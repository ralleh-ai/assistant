use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

use presence_ipc::Command;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use presence_core::render::Renderer;
use presence_core::scene::mode::PresenceMode;
use presence_core::scene::SceneDirector;

#[cfg(feature = "dev")]
use presence_core::scene::SceneRegistry;
#[cfg(feature = "dev")]
use egui_wgpu::ScreenDescriptor;
#[cfg(feature = "dev")]
use crate::ui::{EguiLayer, PanelState};

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
    matches!(
        std::env::var(DROPLET_ENV)
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
            last_frame: Instant::now(),
            sim_accumulator: 0.0,
            fps: 0.0,
            log_fps: std::env::var_os("PRESENCE_LOG_FPS").is_some(),
            fps_log_frames: 0,
            low_fps_seconds: 0.0,
            ipc_commands: crate::ipc_stdin::spawn_if_enabled(),
        }
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

    fn redraw(&mut self) {
        // Apply anything the shell has sent since the last frame before we
        // advance the simulation, so a `SetSignals` that arrives between
        // frames influences *this* frame rather than trailing by one.
        self.drain_pending_commands();
        self.apply_pending_palette();

        let Some(live) = &mut self.live else { return };

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
                return;
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                log::error!("wgpu surface out of memory — exiting");
                return;
            }
            Err(e) => {
                log::warn!("surface error: {e:?}");
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
        // Per-pixel transparency (composite-shader alpha output) is
        // deliberately *not* here — that is a separate change that
        // touches `presence-core::render::post`, and mixing it into
        // the chrome commit would obscure whether a visual regression
        // came from the frame changes or the alpha path.
        let attrs = if droplet_enabled() {
            log::info!(
                "presence-runtime: {DROPLET_ENV}=1 — frameless, always-on-top, \
                 {DROPLET_SIZE_PX:.0}x{DROPLET_SIZE_PX:.0}"
            );
            Window::default_attributes()
                .with_title("Ralleh — Presence")
                .with_decorations(false)
                .with_resizable(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_inner_size(winit::dpi::LogicalSize::new(
                    DROPLET_SIZE_PX,
                    DROPLET_SIZE_PX,
                ))
        } else {
            Window::default_attributes()
                .with_title("Ralleh — Point Cloud Presence (Phase 1 Prototype)")
                .with_inner_size(winit::dpi::LogicalSize::new(960.0, 720.0))
        };
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        let renderer = pollster::block_on(Renderer::new(window.clone()));
        #[cfg(feature = "dev")]
        let ui = EguiLayer::new(&renderer.device, renderer.surface_config.format, &window);

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
                self.redraw();
            }
            _ => {}
        }
    }
}
