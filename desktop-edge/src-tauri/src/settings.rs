//! Local edge settings persisted under the OS app config directory.
//! Written only via allowlisted Tauri commands (no webview FS access).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

const VOICE_STYLES: &[&str] = &["calm", "direct", "warm"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeSettings {
    pub tenant_id: String,
    pub device_id: String,
    pub actor_id: String,
    /// Base URL for the local mcp-server (e.g. http://127.0.0.1:8787).
    pub mcp_base_url: String,
    /// Operator acknowledged Windows/macOS mic permission guidance.
    pub mic_acknowledged: bool,
    /// Preferred speaking style for future TTS / persona (`calm` | `direct` | `warm`).
    #[serde(default)]
    pub voice_style: String,
}

impl Default for EdgeSettings {
    fn default() -> Self {
        Self {
            tenant_id: "local".into(),
            device_id: "desktop-1".into(),
            actor_id: "operator".into(),
            mcp_base_url: "http://127.0.0.1:8787".into(),
            mic_acknowledged: false,
            voice_style: String::new(),
        }
    }
}

impl EdgeSettings {
    /// Critical fields required before the core shell may open.
    pub fn is_complete(&self) -> bool {
        !self.tenant_id.trim().is_empty()
            && !self.device_id.trim().is_empty()
            && !self.actor_id.trim().is_empty()
            && (self.mcp_base_url.starts_with("http://")
                || self.mcp_base_url.starts_with("https://"))
            && self.mic_acknowledged
            && VOICE_STYLES.contains(&self.voice_style.as_str())
    }
}

fn settings_file(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app config dir: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("create config dir: {e}"))?;
    Ok(dir.join("edge-settings.json"))
}

pub fn load_settings(app: &AppHandle) -> Result<EdgeSettings, String> {
    let path = settings_file(app)?;
    if !path.exists() {
        return Ok(EdgeSettings::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read settings: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse settings: {e}"))
}

pub fn save_settings(app: &AppHandle, settings: &EdgeSettings) -> Result<EdgeSettings, String> {
    let cleaned = EdgeSettings {
        tenant_id: settings.tenant_id.trim().to_string(),
        device_id: settings.device_id.trim().to_string(),
        actor_id: settings.actor_id.trim().to_string(),
        mcp_base_url: settings.mcp_base_url.trim().to_string(),
        mic_acknowledged: settings.mic_acknowledged,
        voice_style: settings.voice_style.trim().to_string(),
    };
    if cleaned.tenant_id.is_empty() || cleaned.device_id.is_empty() || cleaned.actor_id.is_empty()
    {
        return Err("tenant, device, and actor labels cannot be empty".into());
    }
    if !(cleaned.mcp_base_url.starts_with("http://")
        || cleaned.mcp_base_url.starts_with("https://"))
    {
        return Err("mcp base URL must start with http:// or https://".into());
    }
    if !cleaned.voice_style.is_empty() && !VOICE_STYLES.contains(&cleaned.voice_style.as_str()) {
        return Err("voice style must be one of: calm, direct, warm".into());
    }
    let path = settings_file(app)?;
    let raw = serde_json::to_string_pretty(&cleaned).map_err(|e| e.to_string())?;
    fs::write(&path, raw).map_err(|e| format!("write settings: {e}"))?;
    Ok(cleaned)
}

pub fn settings_path_display(app: &AppHandle) -> Result<String, String> {
    Ok(settings_file(app)?.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_by_default() {
        assert!(!EdgeSettings::default().is_complete());
    }

    #[test]
    fn complete_when_all_critical_set() {
        let s = EdgeSettings {
            tenant_id: "acme".into(),
            device_id: "desk-1".into(),
            actor_id: "rico".into(),
            mcp_base_url: "http://127.0.0.1:8787".into(),
            mic_acknowledged: true,
            voice_style: "calm".into(),
        };
        assert!(s.is_complete());
    }
}
