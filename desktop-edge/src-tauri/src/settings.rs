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
    /// Completion backend configuration owned by the settings UI.
    /// `None` means "follow the `RALLEH_COMPLETION_*` env vars"
    /// (the pre-UI operator config path); `Some` overrides them
    /// entirely. The `api_key` inside is stored in cleartext on
    /// disk under the OS user's config dir — future work moves it
    /// to the OS keychain (Windows Credential Manager, macOS
    /// Keychain, Linux Secret Service), at which point this field
    /// becomes a reference rather than the key itself.
    #[serde(default)]
    pub completion: Option<CompletionConfig>,
}

/// Serialized completion-backend configuration. Stable serde shape
/// so a future keychain migration can add fields without breaking
/// existing settings files. Kept intentionally close to what a real
/// enterprise settings surface exposes: which provider, which
/// model, and how to authenticate.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompletionConfig {
    pub kind: CompletionKind,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    /// Cleartext API key stored locally. `None` when the provider
    /// doesn't require one (local Ollama / LM Studio / vLLM), or
    /// when the operator hasn't entered one yet. This field is
    /// never round-tripped to the frontend: `backend_status`
    /// exposes only `has_api_key: bool`. See `save_settings` for
    /// the "keep existing key" sentinel handling.
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Which completion provider the router should talk to. `Echo` is
/// the always-safe fallback; `Openai` covers OpenAI and every
/// clone that speaks its `/chat/completions` shape; `Anthropic`
/// speaks the messages API directly.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CompletionKind {
    #[default]
    Echo,
    Openai,
    Anthropic,
}

impl CompletionKind {
    /// Stable label surfaced in the settings UI. Prefer this over
    /// `Debug` because `Debug` output is a stability foot-gun.
    pub fn label(self) -> &'static str {
        match self {
            CompletionKind::Echo => "echo",
            CompletionKind::Openai => "openai",
            CompletionKind::Anthropic => "anthropic",
        }
    }
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
            completion: None,
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
        // Completion config is not touched by this save path — it
        // has its own dedicated `write_completion_config` helper so
        // the wizard flow can't overwrite it with `None` on every
        // save, and the settings UI can update the key without
        // resending it (see `ApiKeyUpdate::Keep`).
        completion: settings.completion.clone(),
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

/// Frontend-facing shape of a completion config: identical to
/// `CompletionConfig` except the api_key is replaced by a boolean.
/// This is the ONLY shape that ever leaves the Rust side toward the
/// webview — we never want to hand a raw key back over the IPC
/// bridge. `From<&CompletionConfig>` is the canonical construction
/// path so future fields can't accidentally leak the key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RedactedCompletionConfig {
    pub kind: CompletionKind,
    pub base_url: String,
    pub model: String,
    pub has_api_key: bool,
}

impl From<&CompletionConfig> for RedactedCompletionConfig {
    fn from(c: &CompletionConfig) -> Self {
        Self {
            kind: c.kind,
            base_url: c.base_url.clone(),
            model: c.model.clone(),
            has_api_key: c.api_key.as_ref().is_some_and(|s| !s.is_empty()),
        }
    }
}

/// Instructions the settings UI sends when saving a completion
/// config. Distinguishes "keep the existing key" (user is editing
/// model / base URL without re-entering the key) from "clear the
/// key" (user is switching to a provider that doesn't need one, or
/// intentionally removing it) from "replace with this value".
///
/// This is what lets the API-key field be write-only in the UI:
/// the frontend never learns the current key, but can still edit
/// other fields without wiping the stored one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum ApiKeyUpdate {
    /// Leave the persisted key untouched.
    Keep,
    /// Clear the persisted key.
    Clear,
    /// Replace the persisted key with this cleartext value.
    Set { value: String },
}

impl ApiKeyUpdate {
    /// Apply this update to an existing key, returning the value to
    /// persist. `Keep` returns the input unchanged, `Clear` returns
    /// `None`, `Set` returns `Some(value)`. Empty `Set` values are
    /// coerced to `None` so an accidentally-blank input doesn't
    /// persist as a truthy-but-empty string.
    pub fn apply(self, existing: Option<String>) -> Option<String> {
        match self {
            ApiKeyUpdate::Keep => existing,
            ApiKeyUpdate::Clear => None,
            ApiKeyUpdate::Set { value } if value.is_empty() => None,
            ApiKeyUpdate::Set { value } => Some(value),
        }
    }
}

/// The write-side shape the settings UI sends for a completion
/// config update. Everything but the API key is present in full;
/// the key comes in as an `ApiKeyUpdate` so "keep existing" doesn't
/// require the frontend to have ever seen the key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompletionConfigUpdate {
    pub kind: CompletionKind,
    pub base_url: String,
    pub model: String,
    #[serde(default = "default_keep_key")]
    pub api_key: ApiKeyUpdate,
}

fn default_keep_key() -> ApiKeyUpdate {
    ApiKeyUpdate::Keep
}

impl CompletionConfigUpdate {
    /// Fold this update into an existing `Option<CompletionConfig>`
    /// and produce the new one to persist. Validates that non-echo
    /// kinds carry a non-empty `base_url` and `model`, since those
    /// are unrecoverable at request time -- better to reject at
    /// save so the settings UI can surface the error inline.
    pub fn into_config(self, existing: Option<&CompletionConfig>) -> Result<CompletionConfig, String> {
        let base_url = self.base_url.trim().to_string();
        let model = self.model.trim().to_string();
        if !matches!(self.kind, CompletionKind::Echo) {
            if base_url.is_empty() {
                return Err(format!(
                    "{} backend requires a base URL",
                    self.kind.label()
                ));
            }
            if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
                return Err(format!(
                    "{} backend base URL must start with http:// or https://",
                    self.kind.label()
                ));
            }
            if model.is_empty() {
                return Err(format!(
                    "{} backend requires a model identifier",
                    self.kind.label()
                ));
            }
        }
        let api_key = self.api_key.apply(existing.and_then(|c| c.api_key.clone()));
        // Anthropic without a key is a config error the request
        // path would silently drop to Echo -- catch it here so the
        // UI can prompt for a key rather than the user staring at
        // an "unexpected fallback" toast.
        if matches!(self.kind, CompletionKind::Anthropic)
            && api_key.as_ref().is_none_or(|s| s.is_empty())
        {
            return Err("anthropic backend requires an API key".into());
        }
        Ok(CompletionConfig {
            kind: self.kind,
            base_url,
            model,
            api_key,
        })
    }
}

/// Update or clear the persisted completion config in-place,
/// preserving every other field of `EdgeSettings`. Called by the
/// `assistant_save_backend` Tauri command; kept in this module so
/// the file I/O + validation logic lives next to the schema.
pub fn write_completion_config(
    app: &AppHandle,
    update: Option<CompletionConfigUpdate>,
) -> Result<EdgeSettings, String> {
    let mut current = load_settings(app)?;
    match update {
        None => current.completion = None,
        Some(u) => {
            let merged = u.into_config(current.completion.as_ref())?;
            current.completion = Some(merged);
        }
    }
    let path = settings_file(app)?;
    let raw = serde_json::to_string_pretty(&current).map_err(|e| e.to_string())?;
    fs::write(&path, raw).map_err(|e| format!("write settings: {e}"))?;
    Ok(current)
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
            completion: None,
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
            completion: None,
        };
        assert!(s.is_complete());
    }

    // ---- Completion config ----------------------------------------

    #[test]
    fn redacted_completion_config_never_exposes_the_key() {
        let full = CompletionConfig {
            kind: CompletionKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model: "claude-3-5-sonnet-latest".into(),
            api_key: Some("sk-ant-secret-123".into()),
        };
        let redacted = RedactedCompletionConfig::from(&full);
        let serialized = serde_json::to_string(&redacted).unwrap();
        assert!(
            !serialized.contains("sk-ant-secret-123"),
            "redacted response leaked the api_key: {serialized}"
        );
        assert!(
            serialized.contains("\"hasApiKey\":true"),
            "expected hasApiKey signal, got {serialized}"
        );
    }

    #[test]
    fn redacted_completion_config_reports_missing_key_when_empty() {
        let full = CompletionConfig {
            kind: CompletionKind::Openai,
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3.2:latest".into(),
            api_key: Some(String::new()),
        };
        let redacted = RedactedCompletionConfig::from(&full);
        assert!(!redacted.has_api_key);
    }

    #[test]
    fn api_key_update_keep_preserves_existing() {
        let existing = Some("old".to_string());
        assert_eq!(
            ApiKeyUpdate::Keep.apply(existing.clone()),
            existing,
            "Keep must be a pure identity on the existing value"
        );
    }

    #[test]
    fn api_key_update_clear_removes_the_key() {
        assert_eq!(ApiKeyUpdate::Clear.apply(Some("old".into())), None);
        assert_eq!(ApiKeyUpdate::Clear.apply(None), None);
    }

    #[test]
    fn api_key_update_set_replaces_and_empty_coerces_to_none() {
        assert_eq!(
            ApiKeyUpdate::Set {
                value: "new".into()
            }
            .apply(Some("old".into())),
            Some("new".into()),
        );
        assert_eq!(
            ApiKeyUpdate::Set { value: "".into() }.apply(Some("old".into())),
            None,
            "empty Set must not persist as truthy-but-blank"
        );
    }

    #[test]
    fn completion_update_rejects_anthropic_without_a_key() {
        let update = CompletionConfigUpdate {
            kind: CompletionKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model: "claude-3-5-sonnet-latest".into(),
            api_key: ApiKeyUpdate::Clear,
        };
        let err = update.into_config(None).unwrap_err();
        assert!(err.contains("anthropic"), "{err}");
        assert!(err.to_lowercase().contains("api key"), "{err}");
    }

    #[test]
    fn completion_update_rejects_non_echo_without_a_model() {
        let update = CompletionConfigUpdate {
            kind: CompletionKind::Openai,
            base_url: "http://localhost:11434/v1".into(),
            model: "  ".into(),
            api_key: ApiKeyUpdate::Keep,
        };
        let err = update.into_config(None).unwrap_err();
        assert!(err.contains("model"), "{err}");
    }

    #[test]
    fn completion_update_rejects_non_http_base_url() {
        let update = CompletionConfigUpdate {
            kind: CompletionKind::Openai,
            base_url: "file:///etc/passwd".into(),
            model: "gpt-4o".into(),
            api_key: ApiKeyUpdate::Keep,
        };
        let err = update.into_config(None).unwrap_err();
        assert!(err.to_lowercase().contains("http"), "{err}");
    }

    #[test]
    fn completion_update_allows_echo_with_empty_fields() {
        // Echo has no network side, so demanding a base URL / model
        // would be actively wrong. Confirm the validator special-
        // cases it correctly.
        let update = CompletionConfigUpdate {
            kind: CompletionKind::Echo,
            base_url: "".into(),
            model: "".into(),
            api_key: ApiKeyUpdate::Keep,
        };
        let cfg = update.into_config(None).unwrap();
        assert_eq!(cfg.kind, CompletionKind::Echo);
    }

    #[test]
    fn completion_update_keep_preserves_the_stored_key() {
        let existing = CompletionConfig {
            kind: CompletionKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model: "claude-3-5-sonnet-latest".into(),
            api_key: Some("sk-ant-existing".into()),
        };
        // User edits the model without touching the key.
        let update = CompletionConfigUpdate {
            kind: CompletionKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model: "claude-3-5-haiku-latest".into(),
            api_key: ApiKeyUpdate::Keep,
        };
        let merged = update.into_config(Some(&existing)).unwrap();
        assert_eq!(merged.model, "claude-3-5-haiku-latest");
        assert_eq!(merged.api_key, Some("sk-ant-existing".into()));
    }

    #[test]
    fn older_settings_without_completion_field_still_load() {
        // Any settings file written before this landing must load
        // as `completion: None` -- otherwise an existing user hits
        // a hard error on startup. This is the same contract every
        // other `#[serde(default)]` field on EdgeSettings has.
        let older = r#"{
            "tenantId": "acme",
            "deviceId": "desk-1",
            "actorId": "rico",
            "mcpBaseUrl": "http://127.0.0.1:8787",
            "micAcknowledged": true,
            "voiceStyle": "calm"
        }"#;
        let parsed: EdgeSettings = serde_json::from_str(older).expect("older settings load");
        assert!(parsed.completion.is_none());
    }
}
