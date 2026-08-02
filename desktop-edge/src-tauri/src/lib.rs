//! Ralleh desktop edge — Tauri command surface (Phase 1).
//!
//! Keep IPC allowlisted and narrow (threat model T11). No raw FS/net
//! exposure to the webview — settings I/O stays in Rust. OS capabilities
//! go through policy + traits (T13), never raw clipboard/mic APIs from JS.

mod mic;
mod os_caps;
mod presence;
mod presence_mic;
mod settings;

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use mic::{mic_feature_enabled, run_mic_smoke, MicSmokeResult};
use os_caps::{run_clipboard_smoke, ClipboardSmokeResult};
use presence::{EventListener, Presence};
use presence_ipc::{Command as PresenceCommand, Event as PresenceEvent, PaletteId, PresenceMode, QualityTier};
use settings::PresencePosition;
use presence_mic::MicPump;
use ralleh_audio_core::{run_mock_voice_pipeline, MockVoicePipelineResult};
use settings::{load_settings, save_settings, settings_path_display, EdgeSettings};

const EDGE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeSettingsResponse {
    #[serde(flatten)]
    pub settings: EdgeSettings,
    /// Derived gate: critical fields present (not a stored flag).
    pub setup_complete: bool,
}

impl EdgeSettingsResponse {
    fn from_settings(settings: EdgeSettings) -> Self {
        let setup_complete = settings.is_complete();
        Self {
            settings,
            setup_complete,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeFeatures {
    pub mic: bool,
    pub clipboard_os: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreStatus {
    pub product: String,
    pub edge: String,
    pub version: String,
    pub message: String,
    pub features: EdgeFeatures,
}

#[tauri::command]
fn core_ping() -> CoreStatus {
    CoreStatus {
        product: "Ralleh".into(),
        edge: "desktop".into(),
        version: EDGE_VERSION.into(),
        message: "Rust edge core is reachable.".into(),
        features: EdgeFeatures {
            mic: mic_feature_enabled(),
            clipboard_os: cfg!(feature = "clipboard-os"),
        },
    }
}

#[tauri::command]
fn voice_smoke() -> Result<MockVoicePipelineResult, String> {
    run_mock_voice_pipeline()
}

#[tauri::command]
fn clipboard_smoke(app: AppHandle) -> Result<ClipboardSmokeResult, String> {
    let settings = load_settings(&app)?;
    run_clipboard_smoke(&settings)
}

#[tauri::command]
fn mic_smoke(app: AppHandle) -> Result<MicSmokeResult, String> {
    let settings = load_settings(&app)?;
    // ~1s is enough to prove device open + frames without freezing the UI long.
    run_mic_smoke(&settings, 1.0)
}

#[tauri::command]
fn load_edge_settings(app: AppHandle) -> Result<EdgeSettingsResponse, String> {
    Ok(EdgeSettingsResponse::from_settings(load_settings(&app)?))
}

#[tauri::command]
fn save_edge_settings(
    app: AppHandle,
    settings: EdgeSettings,
) -> Result<EdgeSettingsResponse, String> {
    Ok(EdgeSettingsResponse::from_settings(save_settings(
        &app, &settings,
    )?))
}

#[tauri::command]
fn edge_settings_path(app: AppHandle) -> Result<String, String> {
    settings_path_display(&app)
}

// -----------------------------------------------------------------------------
// Presence commands (Phase 2 §3)
//
// Every command below is a thin translation from the JS-friendly argument
// shape Tauri hands us into a `presence_ipc::Command`, then a fire-and-forget
// send. The `Presence` state itself handles the case where the renderer is
// disabled (no `RALLEH_PRESENCE_BIN`) or the child has exited, so these
// commands never surface an error to the UI for a missing presence — that
// would be a startup-time misconfiguration, not a per-invocation failure.
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceStatus {
    pub enabled: bool,
}

#[tauri::command]
fn presence_status(presence: State<'_, Presence>) -> PresenceStatus {
    PresenceStatus {
        enabled: presence.is_enabled(),
    }
}

#[tauri::command]
fn presence_set_mode(
    mode: PresenceMode,
    engaged: bool,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetMode { mode, engaged });
    Ok(())
}

/// Continuous-signal packet. Matches `presence_ipc::Signals` verbatim
/// (serde-side rename_all = "camelCase" to match the pattern the other
/// Tauri commands use for JS args). Kept as a local struct rather than
/// reusing the ipc type directly so a future divergence — for example a
/// clamped `audio_level_smoothed` computed on the shell side — doesn't
/// require a wire-version bump.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceSignalsArgs {
    pub intensity: f32,
    pub audio_level: f32,
    pub progress: f32,
    #[serde(default)]
    pub active_modes: Vec<PresenceMode>,
}

#[tauri::command]
fn presence_set_signals(
    signals: PresenceSignalsArgs,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetSignals(presence_ipc::Signals {
        intensity: signals.intensity,
        audio_level: signals.audio_level,
        progress: signals.progress,
        active_modes: signals.active_modes,
    }));
    Ok(())
}

#[tauri::command]
fn presence_set_reduced_motion(
    enabled: bool,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetReducedMotion { enabled });
    Ok(())
}

#[tauri::command]
fn presence_set_palette(
    palette: PaletteId,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetPalette { palette });
    Ok(())
}

#[tauri::command]
fn presence_set_ring_wanted(
    wanted: bool,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetRingWanted { wanted });
    Ok(())
}

#[tauri::command]
fn presence_set_quality_tier(
    tier: QualityTier,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetQualityTier { tier });
    Ok(())
}

#[tauri::command]
fn presence_set_interactive(
    interactive: bool,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    // Click-through toggle. Only meaningful when the runtime is in
    // transparent mode (see `Presence::spawn` env var), but harmless
    // otherwise — the wire type accepts it either way and the runtime
    // just calls `set_cursor_hittest` on whatever window it has.
    presence.send(PresenceCommand::SetInteractive { interactive });
    Ok(())
}

/// Handle to the active mic pump (if any). Mutex because Tauri hands
/// out shared references to managed state and we need to swap the
/// `Option` in-place from the start/stop commands.
type MicPumpState = Mutex<Option<MicPump>>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceMicStatus {
    pub running: bool,
    pub mic_feature: bool,
}

#[tauri::command]
fn presence_mic_status(pump: State<'_, MicPumpState>) -> PresenceMicStatus {
    PresenceMicStatus {
        running: pump.lock().map(|p| p.is_some()).unwrap_or(false),
        mic_feature: mic_feature_enabled(),
    }
}

#[tauri::command]
fn presence_mic_start(
    app: AppHandle,
    presence: State<'_, Presence>,
    pump: State<'_, MicPumpState>,
) -> Result<PresenceMicStatus, String> {
    // Same clearance gate as `mic_smoke`. Live capture without an
    // explicit acknowledgement is exactly the case T13 (policy +
    // capabilities) rules out.
    let settings = load_settings(&app)?;
    if !settings.mic_acknowledged {
        return Err(
            "mic clearance not stamped — open the station log (Voice) and acknowledge OS mic guidance first"
                .into(),
        );
    }

    let mut guard = pump.lock().map_err(|e| e.to_string())?;
    if guard.is_some() {
        // Already running — idempotent success. Restarting would drop
        // the current stream and open a new one; the UI has no way to
        // know it did that from a click and it would be a spec change.
        return Ok(PresenceMicStatus {
            running: true,
            mic_feature: mic_feature_enabled(),
        });
    }
    let Some(sender) = presence.sender_clone() else {
        return Err(
            "presence renderer is not running (set RALLEH_PRESENCE_BIN before starting the shell)"
                .into(),
        );
    };
    let started = MicPump::start(sender)?;
    *guard = Some(started);
    Ok(PresenceMicStatus {
        running: true,
        mic_feature: mic_feature_enabled(),
    })
}

#[tauri::command]
fn presence_mic_stop(pump: State<'_, MicPumpState>) -> Result<PresenceMicStatus, String> {
    let mut guard = pump.lock().map_err(|e| e.to_string())?;
    if let Some(mut p) = guard.take() {
        p.stop();
    }
    Ok(PresenceMicStatus {
        running: false,
        mic_feature: mic_feature_enabled(),
    })
}

/// Builds the reverse-channel listener that persists window geometry
/// to `EdgeSettings`. Runs on the presence reader thread — kept small
/// so a stream of `Moved` events during a drag does not stall the
/// runtime. Failure to persist is logged; it must never bubble up.
fn presence_event_listener(app: AppHandle) -> EventListener {
    Box::new(move |event| match event {
        // `PresenceEvent` is `#[non_exhaustive]` on the wire crate,
        // so a wildcard is required. Any new event variant added
        // later will fall through until it is explicitly handled —
        // ignoring an unknown event is safer than crashing the shell
        // on a runtime that speaks a newer wire.
        PresenceEvent::Ready { x, y } | PresenceEvent::Moved { x, y } => {
            // (0, 0) is the sentinel the runtime uses to mean "window
            // manager did not tell us the position" (e.g. Wayland).
            // Persisting it would clobber a real value from a previous
            // session, so we skip.
            if x == 0 && y == 0 {
                return;
            }
            let mut settings = match load_settings(&app) {
                Ok(s) => s,
                Err(err) => {
                    log::warn!(
                        "desktop-edge: presence position not persisted \
                         (load_settings failed: {err})"
                    );
                    return;
                }
            };
            let next = PresencePosition { x, y };
            if settings.presence_position == Some(next) {
                // No-op writes are wasteful on a drag stream.
                return;
            }
            settings.presence_position = Some(next);
            if let Err(err) = save_settings(&app, &settings) {
                log::warn!(
                    "desktop-edge: presence position not persisted \
                     (save_settings failed: {err})"
                );
            }
        }
        _ => {
            log::debug!("desktop-edge: ignoring unknown presence event");
        }
    })
}

/// Sends a persisted position (if any) back to the presence right
/// after spawn, so the droplet lands where the user last left it.
/// A missing / partial settings file is a normal first-launch state
/// and yields a no-op.
fn restore_presence_position(app: &AppHandle, presence: &Presence) {
    let Ok(settings) = load_settings(app) else {
        return;
    };
    let Some(pos) = settings.presence_position else {
        return;
    };
    presence.send(PresenceCommand::SetPosition { x: pos.x, y: pos.y });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mic_pump: MicPumpState = Mutex::new(None);

    tauri::Builder::default()
        .manage(mic_pump)
        // Presence is spawned inside `setup` so the reverse-channel
        // listener can capture an `AppHandle` — we need one to load
        // and save `EdgeSettings` from the reader thread. Everything
        // else about the previous lifecycle carries over: on shutdown
        // Tauri drops the managed `Presence`, which closes stdin and
        // kills the child (see `presence::Presence::drop`).
        .setup(|app| {
            let handle = app.handle().clone();
            let presence = Presence::spawn_from_env(presence_event_listener(handle.clone()));
            restore_presence_position(&handle, &presence);
            app.manage(presence);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_ping,
            voice_smoke,
            clipboard_smoke,
            mic_smoke,
            load_edge_settings,
            save_edge_settings,
            edge_settings_path,
            presence_status,
            presence_set_mode,
            presence_set_signals,
            presence_set_reduced_motion,
            presence_set_palette,
            presence_set_ring_wanted,
            presence_set_quality_tier,
            presence_set_interactive,
            presence_mic_status,
            presence_mic_start,
            presence_mic_stop,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
