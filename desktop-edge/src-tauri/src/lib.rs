//! Ralleh desktop edge — Tauri command surface (Phase 1).
//!
//! Keep IPC allowlisted and narrow (threat model T11). No raw FS/net
//! exposure to the webview.

use serde::Serialize;

use ralleh_audio_core::{run_mock_voice_pipeline, MockVoicePipelineResult};

const EDGE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreStatus {
    pub product: String,
    pub edge: String,
    pub version: String,
    pub message: String,
}

/// Prove UI → Rust IPC.
#[tauri::command]
fn core_ping() -> CoreStatus {
    CoreStatus {
        product: "Ralleh".into(),
        edge: "desktop".into(),
        version: EDGE_VERSION.into(),
        message: "Rust edge core is reachable.".into(),
    }
}

/// Mock mic → VAD → STT → TTS via `ralleh-audio-core` (no hardware).
#[tauri::command]
fn voice_smoke() -> Result<MockVoicePipelineResult, String> {
    run_mock_voice_pipeline()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![core_ping, voice_smoke])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
