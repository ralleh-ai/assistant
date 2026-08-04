use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

use presence_ipc::Command;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use presence_core::render::{Renderer, RendererOptions};
use presence_core::scene::mode::PresenceMode;
use presence_core::scene::{SceneDirector, ViewportExtent};

#[cfg(feature = "dev")]
use crate::ui::{EguiLayer, FormPanel, PanelState, SceneSelector};
#[cfg(feature = "dev")]
use egui_wgpu::ScreenDescriptor;
#[cfg(feature = "dev")]
use presence_core::scene::SceneRegistry;

/// Simulation runs on a fixed step so motion timing is identical at any
/// frame rate — the behaviors are spring integrators, and letting `dt`
/// float means the same state looks different on a 30 Hz vs 144 Hz display.
const SIM_STEP_SECONDS: f32 = 1.0 / 60.0;
/// Cap on catch-up steps per frame. Past this the backlog is dropped rather
/// than simulated, trading a visible time skip for never entering a
/// death-spiral where catching up costs more than the frame it's chasing.
const MAX_SIM_STEPS_PER_FRAME: u32 = 4;

struct Live {
    window: Arc<Window>,
    renderer: Renderer,
    #[cfg(feature = "dev")]
    ui: EguiLayer,
}

pub struct App {
    live: Option<Live>,
    director: SceneDirector,
    /// Only read by the debug panel today, so it lives with the `dev`
    /// feature. When the shell owns a real registry (Phase 2 §2/§3) this
    /// will move into `presence-core`'s public surface.
    #[cfg(feature = "dev")]
    registry: SceneRegistry,
    /// Debug-panel scene selector state (which scene / anchor / disposition /
    /// scale to present). Dev-only, like the panel that drives it.
    #[cfg(feature = "dev")]
    scene_sel: SceneSelector,
    /// Debug-panel form state — the transition duration the panel morphs at.
    #[cfg(feature = "dev")]
    form_panel: FormPanel,
    last_frame: Instant,
    sim_accumulator: f32,
    fps: f32,
    /// `PRESENCE_LOG_FPS` — periodically logs the frame rate, for judging
    /// performance while the debug panel is collapsed. Read once rather than
    /// per frame.
    log_fps: bool,
    fps_log_frames: u64,
    /// Seconds the smoothed FPS has stayed below `ADAPTIVE_DOWNSHIFT_FPS`.
    /// Once it crosses `ADAPTIVE_DOWNSHIFT_HOLD_SECONDS` the quality tier is
    /// stepped down (once). Never stepped back up automatically — a lift is
    /// a decision that has to survive the transient it produces, and the
    /// point at which one is safe is much easier for the user to judge than
    /// for an autoshift to.
    low_fps_seconds: f32,
    /// Optional stdin transport (see [`crate::ipc_stdin`]). `None` when
    /// `PRESENCE_STDIN_IPC` is not set to a truthy value — that is the
    /// default and keeps the runtime a stand-alone dev harness.
    ipc_commands: Option<Receiver<Command>>,
    /// Reverse-channel sink (see [`crate::ipc_stdout`]). Emits an
    /// [`Event::Ready`] once when the window opens and a throttled
    /// stream of [`Event::Moved`] on drag. No-op when
    /// `PRESENCE_STDOUT_IPC` is unset.
    ipc_events: crate::ipc_stdout::EventSink,
    /// Physical-pixel top-left corner most recently reported to the
    /// shell. Used to suppress noise (winit fires `Moved` on every
    /// pixel of a drag) and to skip apply-echoes (a `SetPosition`
    /// from the shell arrives, we move the window, winit fires
    /// `Moved`, we would otherwise send it right back).
    last_reported_position: Option<(i32, i32)>,
    /// Monotonic clock last time a `Moved` was emitted. Combined with
    /// `MOVE_EMIT_INTERVAL` to rate-limit the drag stream to
    /// something a settings writer can keep up with.
    last_move_emit: Instant,
    /// Runtime process start. Feeds `uptime_ms` on every emitted
    /// [`Event::Heartbeat`] so the shell (and the audit log) can
    /// correlate stalls with startup phase or long-lived process
    /// health.
    started_at: Instant,
    /// Monotonically-increasing counter attached to each heartbeat.
    /// A gap in the sequence tells the shell the runtime restarted
    /// itself internally (a future panic-recovery path may do this);
    /// today it's an ever-increasing tally.
    heartbeat_sequence: u64,
    /// Wall-clock of the last heartbeat we emitted. Combined with
    /// [`presence_ipc::HEARTBEAT_INTERVAL_MS`] to fire the next one
    /// on schedule from the redraw loop — no dedicated timer thread
    /// needed, and the emit is skipped cleanly when the runtime is
    /// paused (window minimized, backgrounded on macOS, etc.),
    /// which the shell reads as "runtime intentionally quiet".
    last_heartbeat_emit: Instant,
}

/// Smoothed FPS at or below this figure counts as under-budget.
///
/// 45 rather than 60: the target is 60, but the smoothed value flickers at
/// the target on healthy hardware, and downshifting on those flickers would
/// make the entity rebuild its point set for reasons a user could not see.
const ADAPTIVE_DOWNSHIFT_FPS: f32 = 45.0;

/// Consecutive seconds of under-budget FPS before adaptive downshift fires.
/// Long enough that a startup transient or a brief tab-swap does not lower
/// the tier, short enough that a genuinely slow machine gets its help before
/// the user has decided to close the window.
const ADAPTIVE_DOWNSHIFT_HOLD_SECONDS: f32 = 3.0;

/// Minimum interval between `Event::Moved` emissions. winit fires a
/// `WindowEvent::Moved` on every screen-pixel step of a drag; without
/// throttling that would flood stdout at hundreds of events per
/// second and the shell would spend real work persisting each one.
/// 100 ms feels responsive to the user (the settings writer sees the
/// final position within a frame of drag-end) and keeps the pipe
/// depth trivial.
const MOVE_EMIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Environment variable that flips the runtime into "droplet" chrome —
/// frameless and always-on-top, sized like an indicator rather than a
/// window. Off by default because the dev harness needs decorations to
/// exercise the resize path and needs standard z-order to read the
/// debug panel alongside a browser or terminal.
///
/// See ADR-013 for the shipping product's window model. This env var is
/// the near-term way to opt in from the same binary; Phase 4 replaces
/// it with a persistent setting.
const DROPLET_ENV: &str = "PRESENCE_DROPLET";

/// Inner size when droplet chrome is active. Sized to be *legible*
/// rather than *maximal* — a 320-pixel square is roughly the smallest
/// the shell + halo read as a coherent surface rather than as a blob;
/// dropping below that flattens the fold silhouette that carries most
/// of the identity.
const DROPLET_SIZE_PX: f64 = 320.0;

fn droplet_enabled() -> bool {
    truthy_env(DROPLET_ENV)
}

/// Per-pixel alpha + click-through opt-in. Implies droplet chrome
/// because a full-window transparent 960×720 dev harness would be
/// worse than either mode on its own — the debug panel would float
/// over a huge invisible rectangle. Off by default; the shell sets
/// this env var when it spawns the runtime for real use (see
/// `desktop-edge/src-tauri/src/presence.rs`).
const TRANSPARENT_ENV: &str = "PRESENCE_TRANSPARENT";

fn transparent_enabled() -> bool {
    truthy_env(TRANSPARENT_ENV)
}

fn truthy_env(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

impl App {
    pub fn new() -> Self {
        Self {
            live: None,
            director: SceneDirector::new(),
            #[cfg(feature = "dev")]
            registry: SceneRegistry::with_builtin_scenes(),
            #[cfg(feature = "dev")]
            scene_sel: SceneSelector::default(),
            #[cfg(feature = "dev")]
            form_panel: FormPanel::default(),
            last_frame: Instant::now(),
            sim_accumulator: 0.0,
            fps: 0.0,
            log_fps: std::env::var_os("PRESENCE_LOG_FPS").is_some(),
            fps_log_frames: 0,
            low_fps_seconds: 0.0,
            ipc_commands: crate::ipc_stdin::spawn_if_enabled(),
            ipc_events: crate::ipc_stdout::EventSink::spawn_if_enabled(),
            last_reported_position: None,
            last_move_emit: Instant::now(),
            started_at: Instant::now(),
            heartbeat_sequence: 0,
            last_heartbeat_emit: Instant::now(),
        }
    }

    /// Emit a [`presence_ipc::Event::Heartbeat`] if the cadence
    /// interval has elapsed since the last one. Called from the
    /// redraw loop so the runtime's own "am I painting frames?"
    /// state is what drives the beat — a wedged event loop stops
    /// beating on its own, which is exactly the failure mode the
    /// shell's stall detector was built for.
    fn maybe_emit_heartbeat(&mut self) {
        let now = Instant::now();
        let elapsed_ms = now.duration_since(self.last_heartbeat_emit).as_millis() as u64;
        if elapsed_ms < presence_ipc::HEARTBEAT_INTERVAL_MS {
            return;
        }
        self.last_heartbeat_emit = now;
        let uptime_ms = now.duration_since(self.started_at).as_millis() as u64;
        self.ipc_events.send(presence_ipc::Event::Heartbeat {
            sequence: self.heartbeat_sequence,
            uptime_ms,
        });
        self.heartbeat_sequence = self.heartbeat_sequence.saturating_add(1);
    }

    /// Drains everything the stdin transport has queued since the last
    /// frame and forwards each command to the director. No-op when the
    /// transport is disabled.
    fn drain_pending_commands(&mut self) {
        let Some(rx) = &self.ipc_commands else {
            return;
        };
        for cmd in crate::ipc_stdin::drain(rx) {
            self.director.apply_command(cmd);
        }
    }

    /// If the shell has requested a palette change via ipc, apply it to
    /// the renderer. Called once per frame after `drain_pending_commands`
    /// so the update lands on the same frame the command arrived.
    fn apply_pending_palette(&mut self) {
        let Some(live) = &mut self.live else { return };
        if let Some(id) = self.director.take_pending_palette() {
            live.renderer.palette = id.palette();
        }
    }

    /// If the shell has requested an interactivity change via ipc,
    /// flip the window's hittest flag. Same one-shot semantics as
    /// `apply_pending_palette` — the director returns `None` once
    /// the request has been consumed, so a runtime restart or a
    /// missed frame does not double-apply.
    /// If the shell has requested a window-position change via ipc,
    /// move the outer window. Coordinates are physical screen pixels —
    /// same units both `WindowEvent::Moved` reports and `Command::SetPosition`
    /// accepts, so a shell that echoes a stored value back gets a no-op
    /// or a corrective move as appropriate.
    ///
    /// After applying, `last_reported_position` is set to the new
    /// value so the resulting winit `Moved` event does not bounce
    /// straight back to the shell as a fresh "user drag" — that
    /// would create an event loop between the two sides.
    fn apply_pending_position(&mut self) {
        let Some(live) = &mut self.live else { return };
        if let Some((x, y)) = self.director.take_pending_position() {
            live.window
                .set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
            self.last_reported_position = Some((x, y));
        }
    }

    fn apply_pending_hittest(&mut self) {
        let Some(live) = &mut self.live else { return };
        if let Some(interactive) = self.director.take_pending_hittest() {
            // `hittest=true` means "receive clicks" — the wire
            // command's `interactive` flag maps 1:1. On error we
            // log rather than panic: a droplet that could not
            // flip is still visually correct, and yelling louder
            // than the log tells us nothing new.
            if let Err(err) = live.window.set_cursor_hittest(interactive) {
                log::warn!("presence-runtime: set_cursor_hittest({interactive}) failed ({err})");
            } else {
                log::info!("presence-runtime: cursor hittest -> {interactive}");
            }
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        // Apply anything the shell has sent since the last frame before we
        // advance the simulation, so a `SetSignals` that arrives between
        // frames influences *this* frame rather than trailing by one.
        self.drain_pending_commands();
        self.apply_pending_palette();
        self.apply_pending_position();
        self.apply_pending_hittest();
        // Heartbeat before the frame goes out. A wedged renderer stops
        // reaching this line, and the shell's stall detector picks it
        // up — the interval-gate inside makes this cheap in the
        // happy case (a compare and return, no I/O).
        self.maybe_emit_heartbeat();

        let Some(live) = &mut self.live else { return };

        let size = live.window.inner_size();
        self.director
            .set_viewport_extent(ViewportExtent::from_pixels(size.width, size.height));

        let now = Instant::now();
        let frame_dt = (now - self.last_frame).as_secs_f32().min(0.25);
        self.last_frame = now;
        if frame_dt > 0.0 {
            let instant_fps = 1.0 / frame_dt.max(1e-6);
            self.fps += (instant_fps - self.fps) * 0.1;
        }

        self.sim_accumulator += frame_dt;
        let mut steps = 0;
        while self.sim_accumulator >= SIM_STEP_SECONDS && steps < MAX_SIM_STEPS_PER_FRAME {
            self.director.tick(SIM_STEP_SECONDS);
            self.sim_accumulator -= SIM_STEP_SECONDS;
            steps += 1;
        }
        if steps == MAX_SIM_STEPS_PER_FRAME {
            self.sim_accumulator = 0.0;
        }

        // Adaptive downshift — see the constants above for the thresholds.
        // Waits for the smoothed FPS to warm up before counting, since the
        // very first few frames of a run are not representative of anything.
        if self.fps > 5.0 {
            if self.fps < ADAPTIVE_DOWNSHIFT_FPS {
                self.low_fps_seconds += frame_dt;
                if self.low_fps_seconds >= ADAPTIVE_DOWNSHIFT_HOLD_SECONDS {
                    if let Some(next) = self.director.tier().lower() {
                        log::info!(
                            "adaptive downshift: {:.1} FPS held under {} for {:.1}s — moving to {}",
                            self.fps,
                            ADAPTIVE_DOWNSHIFT_FPS,
                            self.low_fps_seconds,
                            next.label(),
                        );
                        self.director.set_quality_tier(next);
                    }
                    self.low_fps_seconds = 0.0;
                }
            } else {
                // A single healthy frame resets the timer. That is more
                // lenient than requiring a healthy *streak* to clear it,
                // and the leniency is deliberate: we would rather miss a
                // downshift by a fraction of a second than fire one on a
                // machine that is actually fine and just hit a hitch.
                self.low_fps_seconds = 0.0;
            }
        }

        live.renderer.animate_camera(frame_dt);
        if self.log_fps {
            self.fps_log_frames += 1;
            if self.fps_log_frames.is_multiple_of(180) {
                log::info!("fps {:.1}", self.fps);
            }
        }

        let entities = self.director.entities();
        let entity_particles: Vec<(&[_], f32)> = entities
            .iter()
            .map(|e| (e.particles.as_slice(), e.presence))
            .collect();

        #[allow(unused_mut)]
        let mut frame = match live.renderer.begin_frame(&entity_particles) {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                let size = live.window.inner_size();
                live.renderer.resize(size.width, size.height);
                // Re-arm the frame pump. Every early return from the redraw
                // path MUST request another redraw, or the loop goes idle and
                // the shell's heartbeat watcher reports a false `presence
                // stalled` even though the renderer merely skipped one frame.
                live.window.request_redraw();
                return;
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                // OOM is genuinely unrecoverable — actually exit rather than
                // logging "exiting" and then spinning forever.
                log::error!("wgpu surface out of memory — exiting");
                event_loop.exit();
                return;
            }
            Err(e) => {
                log::warn!("surface error: {e:?}");
                live.window.request_redraw();
                return;
            }
        };

        // Debug overlay is a `dev` feature — compiled out of shipping builds
        // so the runtime does not pull `egui` or its transitive graph.
        #[cfg(feature = "dev")]
        {
            let screen_descriptor = ScreenDescriptor {
                size_in_pixels: [
                    live.renderer.surface_config.width,
                    live.renderer.surface_config.height,
                ],
                pixels_per_point: live.window.scale_factor() as f32,
            };
            live.ui.draw(
                &live.window,
                &live.renderer.device,
                &live.renderer.queue,
                &mut frame,
                &screen_descriptor,
                &mut PanelState {
                    director: &mut self.director,
                    registry: &self.registry,
                    selector: &mut self.scene_sel,
                    form: &mut self.form_panel,
                    material: &mut live.renderer.material,
                    post: &mut live.renderer.post.settings,
                    palette: &mut live.renderer.palette,
                    fps: self.fps,
                },
            );
        }

        frame.finish(&live.renderer.queue);
        live.window.request_redraw();
    }

    fn handle_key(&mut self, key_event: &KeyEvent, event_loop: &ActiveEventLoop) {
        if key_event.state != ElementState::Pressed {
            return;
        }
        match &key_event.logical_key {
            Key::Character(c) if c.eq_ignore_ascii_case("l") => self.director.toggle_ring(),
            Key::Character(c) if c.eq_ignore_ascii_case("r") => {
                self.director.reduced_motion = !self.director.reduced_motion;
            }
            Key::Character(c) if c.eq_ignore_ascii_case("q") => {
                // Cycles: the alternative would be two separate hotkeys, and
                // the tier list is short enough that a single-key cycle is
                // faster to use in the dev harness than picking a direction.
                use presence_core::scene::QualityTier;
                let current = self.director.tier();
                let idx = QualityTier::ALL
                    .iter()
                    .position(|t| *t == current)
                    .unwrap_or(0);
                let next = QualityTier::ALL[(idx + 1) % QualityTier::ALL.len()];
                self.director.set_quality_tier(next);
            }
            Key::Named(NamedKey::Escape) => event_loop.exit(),
            // Toggles rather than a selection, so overlapping modes can be
            // exercised from the keyboard — the composition is the thing that
            // needs looking at, and it is the thing a radio group hides.
            Key::Character(c) => {
                if let Some(mode) = PresenceMode::ALL
                    .into_iter()
                    .find(|m| c.eq_ignore_ascii_case(&m.key().to_string()))
                {
                    self.director.toggle_mode(mode);
                }
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_some() {
            return;
        }

        // Two chrome profiles from the same binary: a resizable dev
        // harness by default, and a small always-on-top droplet under
        // PRESENCE_DROPLET=1. The droplet path is the shape ADR-013
        // commits to for the shipping product; the env var is the
        // near-term opt-in until Phase 4 wires this to a persisted
        // setting.
        //
        // Transparency implies droplet — see the note on
        // `TRANSPARENT_ENV`. Combining the two gives the shape
        // ADR-013 commits to: a frameless, always-on-top, per-pixel
        // alpha droplet that ignores clicks by default.
        let transparent = transparent_enabled();
        let droplet = droplet_enabled() || transparent;

        let attrs = if droplet {
            log::info!(
                "presence-runtime: droplet mode ({DROPLET_ENV}/{TRANSPARENT_ENV}) — \
                 frameless, always-on-top, {DROPLET_SIZE_PX:.0}x{DROPLET_SIZE_PX:.0}, \
                 transparent={transparent}"
            );
            let mut a = Window::default_attributes()
                .with_title("Ralleh — Presence")
                .with_decorations(false)
                .with_resizable(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_inner_size(winit::dpi::LogicalSize::new(
                    DROPLET_SIZE_PX,
                    DROPLET_SIZE_PX,
                ));
            if transparent {
                // Ask the OS compositor to composite this window with
                // per-pixel alpha. Without this the swapchain can still
                // be configured PreMultiplied but the window itself
                // shows an opaque black background, which defeats the
                // whole point.
                a = a.with_transparent(true);
            }
            a
        } else {
            Window::default_attributes()
                .with_title("Ralleh — Point Cloud Presence (Phase 1 Prototype)")
                .with_inner_size(winit::dpi::LogicalSize::new(960.0, 720.0))
        };
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                // A window we cannot create is fatal, but exit the loop
                // cleanly rather than unwinding a panic across the winit
                // callback boundary (which aborts with a far less useful
                // message and no chance to flush logs).
                log::error!("presence-runtime: failed to create window: {err}");
                event_loop.exit();
                return;
            }
        };

        // Click-through: the droplet must not eat mouse events meant
        // for the app underneath it, or the user will find their own
        // desktop half-usable whenever the presence is on. `false`
        // means "cursor events pass through to whatever is under
        // this window"; winit routes to WS_EX_TRANSPARENT on Windows
        // and the equivalents on macOS/Linux. Non-fatal on error —
        // some platforms report `NotSupported`, and a droplet that
        // grabs clicks is worse than no droplet only for the click,
        // not for the visuals.
        if transparent {
            if let Err(err) = window.set_cursor_hittest(false) {
                log::warn!(
                    "presence-runtime: set_cursor_hittest(false) failed ({err}); \
                     clicks on the droplet will not pass through to windows behind it"
                );
            }
        }

        let renderer = pollster::block_on(Renderer::new_with_options(
            window.clone(),
            RendererOptions { transparent },
        ));
        #[cfg(feature = "dev")]
        let ui = EguiLayer::new(&renderer.device, renderer.surface_config.format, &window);

        // Report the initial position over the reverse channel so a
        // fresh shell that has never seen this presence has a value
        // to persist immediately. Reading `outer_position` at this
        // point picks up whatever the window manager placed us at —
        // subsequent moves (either from the user dragging or from a
        // `SetPosition` command) travel through `Moved` events.
        if let Ok(pos) = window.outer_position() {
            self.last_reported_position = Some((pos.x, pos.y));
            self.ipc_events
                .send(presence_ipc::Event::Ready { x: pos.x, y: pos.y });
        } else {
            // Some window managers (Wayland notably) don't expose an
            // outer position. Still emit a Ready so the shell knows the
            // runtime is alive; use (0, 0) as a sentinel that means
            // "we don't know, don't persist this value".
            self.ipc_events
                .send(presence_ipc::Event::Ready { x: 0, y: 0 });
        }

        window.request_redraw();
        self.last_frame = Instant::now();
        self.live = Some(Live {
            window,
            renderer,
            #[cfg(feature = "dev")]
            ui,
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        // Without the `dev` feature the overlay does not exist, so no window
        // event is ever consumed by UI and the whole check collapses to
        // `false` at compile time.
        #[cfg(feature = "dev")]
        let consumed_by_ui = if let Some(live) = &mut self.live {
            if live.window.id() != window_id {
                false
            } else {
                live.ui.handle_event(&live.window, &event)
            }
        } else {
            false
        };
        #[cfg(not(feature = "dev"))]
        let consumed_by_ui = false;
        #[cfg(not(feature = "dev"))]
        let _ = window_id;

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(live) = &mut self.live {
                    live.renderer.resize(size.width, size.height);
                    self.director
                        .set_viewport_extent(ViewportExtent::from_pixels(size.width, size.height));
                }
            }
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if !consumed_by_ui {
                    self.handle_key(&key_event, event_loop);
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
            }
            WindowEvent::Moved(pos) => {
                self.on_window_moved(pos.x, pos.y);
            }
            _ => {}
        }
    }
}

impl App {
    /// Handles a `WindowEvent::Moved`. Rate-limited to
    /// `MOVE_EMIT_INTERVAL` and suppressed when the current position
    /// equals the last one we told the shell about — which is what
    /// prevents an echo loop for `Command::SetPosition` (shell sends
    /// position, we move, winit fires `Moved`, we would otherwise
    /// send the same coords back).
    fn on_window_moved(&mut self, x: i32, y: i32) {
        if self.last_reported_position == Some((x, y)) {
            return;
        }
        let now = Instant::now();
        if now.duration_since(self.last_move_emit) < MOVE_EMIT_INTERVAL {
            // Drop, but keep the last-reported position stale so the
            // final resting frame of a drag still fires when the
            // interval elapses. Winit issues `Moved` on drag-end too,
            // so the settled coordinate reliably lands.
            return;
        }
        self.last_reported_position = Some((x, y));
        self.last_move_emit = now;
        self.ipc_events.send(presence_ipc::Event::Moved { x, y });
    }
}
