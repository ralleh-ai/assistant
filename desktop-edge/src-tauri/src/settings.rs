//! Local edge settings persisted under the OS app config directory.
//! Written only via allowlisted Tauri commands (no webview FS access).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use presence_ipc::{PaletteId, QualityTier};

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
    /// Persisted presence-droplet top-left corner (physical screen
    /// pixels). `None` on first launch, then populated by the
    /// reverse-channel `Event::Ready` / `Event::Moved` from
    /// `presence-runtime`. Written back out on every position change
    /// so a crash keeps at most the last dragged-to position.
    #[serde(default)]
    pub presence_position: Option<PresencePosition>,
    /// Colour scheme applied to the presence on startup. `None` uses
    /// the runtime's compiled-in default (Teal). Wire type mirrors
    /// `presence_ipc::PaletteId` so the same enum flows through
    /// settings, Tauri commands, and the ipc envelope without
    /// re-mapping. Every `presence_set_palette` invocation writes
    /// back here — see `run` in `lib.rs`.
    #[serde(default)]
    pub presence_palette: Option<PaletteId>,
    /// Adaptive-quality tier the shell pins on startup. `None` lets
    /// the runtime's adaptive downshift start from `Balanced`.
    #[serde(default)]
    pub presence_quality_tier: Option<QualityTier>,
    /// Accessibility preset. Persisted as a plain bool because the
    /// only two shell states are on/off; the runtime handles the
    /// crossfade. `#[serde(default)]` keeps older settings files
    /// (before this field existed) loadable as `false`.
    #[serde(default)]
    pub presence_reduced_motion: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PresencePosition {
    pub x: i32,
    pub y: i32,
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
            presence_position: None,
            presence_palette: None,
            presence_quality_tier: None,
            presence_reduced_motion: false,
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
        presence_position: settings.presence_position,
        presence_palette: settings.presence_palette,
        presence_quality_tier: settings.presence_quality_tier,
        presence_reduced_motion: settings.presence_reduced_motion,
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
    fn presence_fields_are_optional_and_default_out_of_older_settings() {
        // A settings file written before the presence fields existed
        // must still load — that is what `#[serde(default)]` on every
        // new field is buying. If this test breaks, an existing user
        // hits a hard error on startup and has to nuke their config.
        let older = r#"{
            "tenantId": "acme",
            "deviceId": "desk-1",
            "actorId": "rico",
            "mcpBaseUrl": "http://127.0.0.1:8787",
            "micAcknowledged": true,
            "voiceStyle": "calm"
        }"#;
        let parsed: EdgeSettings = serde_json::from_str(older).expect("older settings load");
        assert_eq!(parsed.presence_position, None);
        assert_eq!(parsed.presence_palette, None);
        assert_eq!(parsed.presence_quality_tier, None);
        assert!(!parsed.presence_reduced_motion);
    }

    #[test]
    fn presence_fields_round_trip_through_serde() {
        let settings = EdgeSettings {
            tenant_id: "acme".into(),
            device_id: "desk-1".into(),
            actor_id: "rico".into(),
            mcp_base_url: "http://127.0.0.1:8787".into(),
            mic_acknowledged: true,
            voice_style: "calm".into(),
            presence_position: Some(PresencePosition { x: 200, y: 300 }),
            presence_palette: Some(PaletteId::Ember),
            presence_quality_tier: Some(QualityTier::Low),
            presence_reduced_motion: true,
        };
        let encoded = serde_json::to_string(&settings).unwrap();
        let decoded: EdgeSettings = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.presence_position, settings.presence_position);
        assert_eq!(decoded.presence_palette, settings.presence_palette);
        assert_eq!(
            decoded.presence_quality_tier,
            settings.presence_quality_tier
        );
        assert_eq!(
            decoded.presence_reduced_motion,
            settings.presence_reduced_motion
        );
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
            presence_position: None,
            presence_palette: None,
            presence_quality_tier: None,
            presence_reduced_motion: false,
        };
        assert!(s.is_complete());
    }
}
