//! Ralleh desktop edge — Tauri command surface (Phase 1).
//!
//! Keep IPC allowlisted and narrow (threat model T11). No raw FS/net
//! exposure to the webview — settings I/O stays in Rust. OS capabilities
//! go through policy + traits (T13), never raw clipboard/mic APIs from JS.

mod mic;
mod os_caps;
mod presence;
mod settings;

use serde::Serialize;
use tauri::{AppHandle, State};

use mic::{mic_feature_enabled, run_mic_smoke, MicSmokeResult};
use os_caps::{run_clipboard_smoke, ClipboardSmokeResult};
use presence::Presence;
use presence_ipc::{Command as PresenceCommand, PaletteId, PresenceMode, QualityTier};
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Presence is spawned once at startup and installed as managed state.
    // Every Tauri command that wants to nudge the visuals extracts it via
    // `State<'_, Presence>`; on shutdown Tauri drops the state, which
    // closes stdin and kills the child (see `presence::Presence::drop`).
    let presence = Presence::spawn_from_env();

    tauri::Builder::default()
        .manage(presence)
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
            presence_set_reduced_motion,
            presence_set_palette,
            presence_set_ring_wanted,
            presence_set_quality_tier,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
