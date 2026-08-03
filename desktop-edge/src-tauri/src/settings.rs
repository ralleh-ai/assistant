//! Local edge settings persisted under the OS app config directory.
//! Written only via allowlisted Tauri commands (no webview FS access).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use presence_ipc::{PaletteId, QualityTier};

use crate::secret_store::{SecretStorage, SecretStore};

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
    /// entirely. As of the OS keychain migration the `api_key`
    /// field is always `None` on disk after first successful
    /// write; the actual secret lives in
    /// [`crate::secret_store::SecretStore`]. The field is kept on
    /// the wire so pre-migration settings files still load and
    /// can be migrated in-place — see
    /// [`migrate_completion_secret`].
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq, Hash)]
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
/// `CompletionConfig` except the api_key is replaced by a boolean
/// and a storage-backend signal is added. This is the ONLY shape
/// that ever leaves the Rust side toward the webview — we never
/// want to hand a raw key back over the IPC bridge.
///
/// `storage` tells the UI whether the key lives in the OS keychain
/// (`Keychain`), was written to `edge-settings.json` in cleartext
/// because no keychain was available (`Cleartext`), or nothing is
/// stored yet (`None`). The settings UI renders an honest badge
/// off this signal — no more pretending everything's secure when
/// it isn't.
///
/// Build it via [`RedactedCompletionConfig::from_config_and_store`]
/// so `has_api_key` is always sourced from the authoritative
/// store, not from a stale on-disk copy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RedactedCompletionConfig {
    pub kind: CompletionKind,
    pub base_url: String,
    pub model: String,
    pub has_api_key: bool,
    pub storage: SecretStorage,
}

impl RedactedCompletionConfig {
    /// Build a redacted view of `cfg` where `has_api_key` is
    /// sourced from `store` (the authoritative location for the
    /// secret) and falls back to the cleartext `api_key` field
    /// only for pre-migration configs. `storage` reflects where
    /// the key actually lives:
    /// - `Keychain` when the store returned a value.
    /// - `Cleartext` when only the on-disk copy still has one
    ///   (i.e. the migration hasn't run or the store rejected the
    ///   write).
    /// - `None` when neither location has a key.
    pub fn from_config_and_store(cfg: &CompletionConfig, store: &dyn SecretStore) -> Self {
        let in_store = store
            .read(cfg.kind)
            .ok()
            .flatten()
            .is_some_and(|s| !s.is_empty());
        let in_cleartext = cfg.api_key.as_ref().is_some_and(|s| !s.is_empty());
        let storage = if in_store {
            SecretStorage::Keychain
        } else if in_cleartext {
            SecretStorage::Cleartext
        } else {
            SecretStorage::None
        };
        Self {
            kind: cfg.kind,
            base_url: cfg.base_url.clone(),
            model: cfg.model.clone(),
            has_api_key: in_store || in_cleartext,
            storage,
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
    /// Validate the update's non-secret fields and fold in an
    /// existing secret. Called from both the save path (where
    /// `existing_secret` comes from the [`SecretStore`]) and the
    /// live-test path (where the same lookup provides the current
    /// key for `Keep` requests). The returned `CompletionConfig`
    /// carries the resolved `api_key` inline — callers who intend
    /// to persist to disk MUST strip it via
    /// [`CompletionConfig::without_secret`] before writing, and
    /// route the raw key through a `SecretStore::write`.
    pub fn into_config_with_secret(
        self,
        existing_secret: Option<String>,
    ) -> Result<CompletionConfig, String> {
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
        let api_key = self.api_key.apply(existing_secret);
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

    /// Convenience for callers that already have an
    /// `Option<CompletionConfig>` on hand (pre-store callers and
    /// tests). Prefer `into_config_with_secret` in production code
    /// so the secret source is explicit.
    #[cfg(test)]
    pub fn into_config(
        self,
        existing: Option<&CompletionConfig>,
    ) -> Result<CompletionConfig, String> {
        self.into_config_with_secret(existing.and_then(|c| c.api_key.clone()))
    }
}

impl CompletionConfig {
    /// Return a copy with `api_key` cleared. Callers use this to
    /// build the disk-safe representation while keeping the
    /// resolved config for in-memory use (backend construction,
    /// live tests, etc.).
    pub fn without_secret(&self) -> Self {
        Self {
            kind: self.kind,
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key: None,
        }
    }

    /// Fill in `api_key` from `store` if it's currently `None`.
    /// Returns a resolved config suitable for backend construction.
    /// Pre-migration configs (with a cleartext `api_key`) pass
    /// through unchanged — the migration path handles the transfer
    /// to the store separately.
    pub fn resolve_with_store(&self, store: &dyn SecretStore) -> Self {
        if self.api_key.as_ref().is_some_and(|s| !s.is_empty()) {
            return self.clone();
        }
        let mut resolved = self.clone();
        resolved.api_key = store.read(self.kind).ok().flatten();
        resolved
    }
}

/// Persist a completion-config update. Routes the API key through
/// `store` (the OS keychain when available) and writes only the
/// non-secret fields to `edge-settings.json`. The returned
/// `EdgeSettings` has `completion.api_key = None` — callers that
/// need the key for immediate use should call
/// `resolve_with_store` on the returned config.
///
/// Failure modes:
/// - `store.write` fails (no keychain available, permission
///   denied): returns the error unchanged so the UI can surface
///   it. Disk is NOT touched in this case, so the existing
///   configuration remains intact.
/// - JSON write fails: keychain write may have already succeeded.
///   The next successful save will overwrite the store; in the
///   meantime the key is orphaned but not leaked.
pub fn write_completion_config(
    app: &AppHandle,
    store: &dyn SecretStore,
    update: Option<CompletionConfigUpdate>,
) -> Result<EdgeSettings, String> {
    let mut current = load_settings(app)?;
    match update {
        None => {
            // Clearing the whole config also clears any secret we
            // stored for its current kind. Other kinds' keys stay
            // put so an operator can experiment without losing
            // configured providers.
            if let Some(cfg) = &current.completion {
                let _ = store.clear(cfg.kind);
            }
            current.completion = None;
        }
        Some(u) => {
            let existing_secret = store.read(u.kind).ok().flatten();
            let kind = u.kind;
            let api_key_update = u.api_key.clone();
            let resolved = u.into_config_with_secret(existing_secret)?;
            // Persist the secret first: on failure we bail before
            // touching disk, so the on-disk config still points at
            // a valid stored key.
            match api_key_update {
                ApiKeyUpdate::Keep => { /* store already correct */ }
                ApiKeyUpdate::Clear => {
                    store.clear(kind)?;
                }
                ApiKeyUpdate::Set { value } if value.is_empty() => {
                    store.clear(kind)?;
                }
                ApiKeyUpdate::Set { value } => {
                    store.write(kind, &value)?;
                }
            }
            current.completion = Some(resolved.without_secret());
        }
    }
    let path = settings_file(app)?;
    let raw = serde_json::to_string_pretty(&current).map_err(|e| e.to_string())?;
    fs::write(&path, raw).map_err(|e| format!("write settings: {e}"))?;
    Ok(current)
}

/// One-shot migration of a cleartext key from disk into `store`.
/// Called during startup right after `load_settings`; a no-op when
/// there's no cleartext key or the store rejects the write. On
/// success the on-disk copy is cleared and `edge-settings.json` is
/// rewritten — subsequent boots see only the keychain-stored key.
///
/// Returns `Ok(true)` if a migration happened, `Ok(false)` if
/// nothing needed to move, and `Err(_)` if the disk rewrite failed
/// after a successful keychain write (rare, but recoverable — the
/// key is safely in the keychain, the next save will clean up).
pub fn migrate_completion_secret(
    app: &AppHandle,
    store: &dyn SecretStore,
    settings: &mut EdgeSettings,
) -> Result<bool, String> {
    let Some(cfg) = settings.completion.as_mut() else {
        return Ok(false);
    };
    let Some(cleartext) = cfg.api_key.take_if(|s| !s.is_empty()) else {
        // No cleartext key — either nothing to migrate, or it's
        // an empty string we may as well drop. `take_if` already
        // handled the empty case; put back any leftover None.
        return Ok(false);
    };
    match store.write(cfg.kind, &cleartext) {
        Ok(()) => {
            // Verify roundtrip before clearing the on-disk copy.
            // A silently-corrupting keychain (rare, but seen in the
            // wild on old Linux distros with a broken libsecret) would
            // otherwise let us drop the only usable copy of the key.
            // If verification fails we restore the cleartext and
            // report the mismatch so the operator can retry.
            match store.read(cfg.kind) {
                Ok(Some(back)) if back == cleartext => {
                    // Verified: the taken `api_key` is already None
                    // on `cfg`. Persist the cleared form so no one
                    // else ever sees the cleartext again.
                    let path = settings_file(app)?;
                    let raw =
                        serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
                    fs::write(&path, raw).map_err(|e| format!("write settings: {e}"))?;
                    Ok(true)
                }
                Ok(_) => {
                    cfg.api_key = Some(cleartext);
                    Err(
                        "keychain roundtrip verification failed (stored value did not \
                         match the source); keeping cleartext key on disk"
                            .into(),
                    )
                }
                Err(e) => {
                    cfg.api_key = Some(cleartext);
                    Err(format!(
                        "keychain roundtrip verification failed ({e}); keeping cleartext key on disk"
                    ))
                }
            }
        }
        Err(e) => {
            // Store rejected the write (no keychain available).
            // Put the cleartext key back so the shell keeps
            // functioning; the UI's storage badge will surface the
            // insecure fallback to the operator.
            cfg.api_key = Some(cleartext);
            Err(e)
        }
    }
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

    use crate::secret_store::{InMemorySecretStore, NullStore};

    #[test]
    fn redacted_completion_config_never_exposes_the_key_from_store() {
        let full = CompletionConfig {
            kind: CompletionKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model: "claude-3-5-sonnet-latest".into(),
            api_key: None,
        };
        let store =
            InMemorySecretStore::with_entry(CompletionKind::Anthropic, "sk-ant-secret-123");
        let redacted = RedactedCompletionConfig::from_config_and_store(&full, &store);
        let serialized = serde_json::to_string(&redacted).unwrap();
        assert!(
            !serialized.contains("sk-ant-secret-123"),
            "redacted response leaked the api_key: {serialized}"
        );
        assert!(
            serialized.contains("\"hasApiKey\":true"),
            "expected hasApiKey signal, got {serialized}"
        );
        assert!(
            serialized.contains("\"storage\":\"keychain\""),
            "expected keychain storage badge, got {serialized}"
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
        let store = InMemorySecretStore::new();
        let redacted = RedactedCompletionConfig::from_config_and_store(&full, &store);
        assert!(!redacted.has_api_key);
        assert_eq!(redacted.storage, SecretStorage::None);
    }

    #[test]
    fn redacted_completion_config_reports_cleartext_when_store_empty_but_disk_has_key() {
        // Pre-migration state: key still lives in the JSON.
        let full = CompletionConfig {
            kind: CompletionKind::Openai,
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3.2".into(),
            api_key: Some("old-cleartext".into()),
        };
        let store = InMemorySecretStore::new();
        let redacted = RedactedCompletionConfig::from_config_and_store(&full, &store);
        assert!(redacted.has_api_key);
        assert_eq!(redacted.storage, SecretStorage::Cleartext);
    }

    #[test]
    fn resolve_with_store_prefers_stored_secret_when_disk_is_none() {
        let cfg = CompletionConfig {
            kind: CompletionKind::Openai,
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3.2".into(),
            api_key: None,
        };
        let store = InMemorySecretStore::with_entry(CompletionKind::Openai, "sk-live");
        let resolved = cfg.resolve_with_store(&store);
        assert_eq!(resolved.api_key.as_deref(), Some("sk-live"));
    }

    #[test]
    fn resolve_with_store_preserves_cleartext_when_present() {
        // Pre-migration: we shouldn't clobber a still-on-disk key
        // with a (probably empty) store lookup.
        let cfg = CompletionConfig {
            kind: CompletionKind::Openai,
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3.2".into(),
            api_key: Some("still-on-disk".into()),
        };
        let store = InMemorySecretStore::new();
        let resolved = cfg.resolve_with_store(&store);
        assert_eq!(resolved.api_key.as_deref(), Some("still-on-disk"));
    }

    #[test]
    fn without_secret_produces_a_disk_safe_copy() {
        let cfg = CompletionConfig {
            kind: CompletionKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model: "claude".into(),
            api_key: Some("sk-ant".into()),
        };
        let sanitized = cfg.without_secret();
        assert_eq!(sanitized.api_key, None);
        let serialized = serde_json::to_string(&sanitized).unwrap();
        assert!(
            !serialized.contains("sk-ant"),
            "without_secret leaked the api_key on disk: {serialized}"
        );
    }

    #[test]
    fn null_store_write_path_rejects_secrets_cleanly() {
        // Sanity check the surface: on a host without a keychain
        // the store surfaces an error the UI can display. The
        // *migration* path handles this differently (keeps
        // cleartext); this covers the save-new-key path.
        let store = NullStore;
        let update = CompletionConfigUpdate {
            kind: CompletionKind::Openai,
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3.2".into(),
            api_key: ApiKeyUpdate::Set {
                value: "sk-new".into(),
            },
        };
        // `into_config_with_secret` itself doesn't touch the
        // store — it's a pure validation function. The store
        // write happens in `write_completion_config` which needs
        // a real AppHandle we don't have in unit tests. Exercise
        // the store surface directly instead:
        let err = store.write(update.kind, "sk-new").unwrap_err();
        assert!(err.to_lowercase().contains("keychain"), "{err}");
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
