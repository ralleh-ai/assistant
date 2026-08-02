//! Ralleh desktop edge — Tauri command surface (Phase 1).
//!
//! Keep IPC allowlisted and narrow (threat model T11). No raw FS/net
//! exposure to the webview — settings I/O stays in Rust.

mod settings;

use serde::Serialize;
use tauri::AppHandle;

use ralleh_audio_core::{run_mock_voice_pipeline, MockVoicePipelineResult};
use settings::{load_settings, save_settings, settings_path_display, EdgeSettings};

const EDGE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreStatus {
    pub product: String,
    pub edge: String,
    pub version: String,
    pub message: String,
}

#[tauri::command]
fn core_ping() -> CoreStatus {
    CoreStatus {
        product: "Ralleh".into(),
        edge: "desktop".into(),
        version: EDGE_VERSION.into(),
        message: "Rust edge core is reachable.".into(),
    }
}

#[tauri::command]
fn voice_smoke() -> Result<MockVoicePipelineResult, String> {
    run_mock_voice_pipeline()
}

#[tauri::command]
fn load_edge_settings(app: AppHandle) -> Result<EdgeSettings, String> {
    load_settings(&app)
}

#[tauri::command]
fn save_edge_settings(app: AppHandle, settings: EdgeSettings) -> Result<EdgeSettings, String> {
    save_settings(&app, &settings)
}

#[tauri::command]
fn edge_settings_path(app: AppHandle) -> Result<String, String> {
    settings_path_display(&app)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            core_ping,
            voice_smoke,
            load_edge_settings,
            save_edge_settings,
            edge_settings_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
