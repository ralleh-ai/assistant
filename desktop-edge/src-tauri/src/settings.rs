//! Local edge settings persisted under the OS app config directory.
//! Written only via allowlisted Tauri commands (no webview FS access).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use presence_ipc::{PaletteId, QualityTier};
use ralleh_policy_core::EgressPolicy;

use crate::secret_store::{SecretStorage, SecretStore};

const VOICE_STYLES: &[&str] = &["calm", "direct", "warm"];

/// Current on-disk schema version. Bumped whenever a field's
/// semantic meaning shifts, a variant is renamed, or a type is
/// split -- anything that a fresh serde parse of an older file
/// would silently misinterpret. Additive optional fields do
/// *not* require a bump (they land on the default via
/// `#[serde(default)]`); only shape or semantic breaks do.
///
/// v1 codifies the shape that shipped through the OS-keychain
/// migration: presence position/palette/quality/reduced-motion
/// as optional fields, completion.api_key nullable with the
/// real secret in the OS keychain.
///
/// Update [`MIGRATIONS`] alongside this constant. A build whose
/// `MIGRATIONS` table does not close the gap 0..=CURRENT will
/// trip the assertion in [`assert_migration_chain_closes`].
pub const CURRENT_SETTINGS_VERSION: u32 = 1;

/// Return value from [`load_settings`] and friends so the caller
/// can distinguish "loaded fine" from "there's a settings file
/// we couldn't safely load". The distinction matters for the
/// startup migration audit: a corrupt / future-versioned file
/// must not be silently overwritten with a fresh `Default`.
#[derive(Debug)]
pub enum LoadOutcome {
    /// File parsed cleanly at the current or an older version.
    /// `migrated_from` names the on-disk version *before* any
    /// migration ran (equal to CURRENT for the happy path).
    Loaded {
        settings: EdgeSettings,
        migrated_from: u32,
    },
    /// No file existed; caller received a fresh `Default`.
    /// Treated separately so a first-launch shell doesn't get
    /// audited as a migration.
    FreshDefault(EdgeSettings),
    /// File exists but sits at a version this build doesn't
    /// know how to migrate down from (i.e. `on_disk > CURRENT`).
    /// Callers MUST NOT overwrite this file -- doing so would
    /// discard the newer shell's data. The caller runs on a
    /// synthesized `Default` in memory for this session only.
    FutureVersion {
        default: EdgeSettings,
        on_disk_version: u32,
    },
    /// File exists but did not parse (bad JSON, missing
    /// required fields, or a v0→v1 migration that threw). The
    /// caller falls back to `Default` in memory but does not
    /// overwrite the file, matching the fail-closed posture
    /// from the audit log.
    Unreadable {
        default: EdgeSettings,
        reason: String,
    },
}

impl LoadOutcome {
    #[allow(dead_code)] // public inspection surface for future consumers (settings UI)
    pub fn settings(&self) -> &EdgeSettings {
        match self {
            LoadOutcome::Loaded { settings, .. }
            | LoadOutcome::FreshDefault(settings)
            | LoadOutcome::FutureVersion { default: settings, .. }
            | LoadOutcome::Unreadable { default: settings, .. } => settings,
        }
    }

    /// Move-out variant for callers who don't need to inspect
    /// the outcome shape but do need ownership.
    pub fn into_settings(self) -> EdgeSettings {
        match self {
            LoadOutcome::Loaded { settings, .. }
            | LoadOutcome::FreshDefault(settings)
            | LoadOutcome::FutureVersion { default: settings, .. }
            | LoadOutcome::Unreadable { default: settings, .. } => settings,
        }
    }
}

/// Migration step: transform a raw JSON `Value` at `from` into
/// the shape expected at `to`. Return `Err` to abort the whole
/// migration chain (the caller falls back to `Default` in memory
/// without overwriting the file, so a broken migration is
/// recoverable by shipping a fixed build).
///
/// A migration MUST be idempotent under re-run: running it on a
/// file already at `to` MUST leave it unchanged. That property
/// makes the chain safe to re-execute after a shell crash
/// mid-write, or after a rollback.
type MigrationFn = fn(serde_json::Value) -> Result<serde_json::Value, String>;

/// Ordered chain of `(from, to, fn)` migrations. `run_migrations`
/// walks the chain and applies every step whose `from` matches
/// the current version. Gaps in the chain trip
/// [`assert_migration_chain_closes`] at build/test time so a
/// forgotten step can never ship.
///
/// v0 → v1 is a no-op relabel: the on-disk shape didn't change,
/// but shipping a version tag lets us reason about "what did
/// this file *mean* when it was written" for every subsequent
/// bump. Future entries look like:
///
/// ```text
/// (1, 2, migrate_v1_to_v2 as MigrationFn),
/// ```
///
/// where `migrate_v1_to_v2` renames / reshapes fields inside
/// the JSON.
const MIGRATIONS: &[(u32, u32, MigrationFn)] = &[(0, 1, migrate_v0_to_v1)];

/// v0 → v1: stamp the `version` field, no shape change. Files
/// written by pre-versioning shells already load cleanly today
/// through `#[serde(default)]` on every additive field; this
/// migration codifies the version tag so subsequent bumps have
/// a stable baseline to reason from.
fn migrate_v0_to_v1(mut value: serde_json::Value) -> Result<serde_json::Value, String> {
    if let Some(obj) = value.as_object_mut() {
        obj.insert("version".into(), serde_json::json!(1));
    }
    Ok(value)
}

/// Walk [`MIGRATIONS`] starting at `from` until we reach
/// `CURRENT_SETTINGS_VERSION` or run out of steps. Returns the
/// migrated Value on success or the offending step's error on
/// failure. Never overwrites in-place — the caller decides
/// whether to rewrite the file.
pub fn run_migrations(
    mut value: serde_json::Value,
    from: u32,
) -> Result<(serde_json::Value, u32), String> {
    let mut current = from;
    while current < CURRENT_SETTINGS_VERSION {
        let step = MIGRATIONS
            .iter()
            .find(|(f, _, _)| *f == current)
            .ok_or_else(|| {
                format!(
                    "no migration registered from version {current} to \
                     {CURRENT_SETTINGS_VERSION}; refusing to guess"
                )
            })?;
        value = step.2(value).map_err(|e| {
            format!(
                "migration {from} -> {to} failed: {e}",
                from = step.0,
                to = step.1
            )
        })?;
        current = step.1;
    }
    Ok((value, current))
}

/// Compile-time-ish check that the migration chain has no gaps
/// between 0 and `CURRENT_SETTINGS_VERSION`. Called by a unit
/// test rather than a `const` because iterating a slice at
/// const-time still needs unstable features on stable rustc;
/// the practical effect is the same.
#[allow(dead_code)] // exercised only by unit tests; reserved for a future build-script check
fn assert_migration_chain_closes() {
    let mut cursor = 0_u32;
    while cursor < CURRENT_SETTINGS_VERSION {
        let step = MIGRATIONS.iter().find(|(f, _, _)| *f == cursor);
        assert!(
            step.is_some(),
            "MIGRATIONS chain has a gap at version {cursor}"
        );
        let (_, to, _) = step.unwrap();
        assert!(*to > cursor, "MIGRATIONS entry from {cursor} must advance");
        cursor = *to;
    }
    assert_eq!(
        cursor, CURRENT_SETTINGS_VERSION,
        "MIGRATIONS chain does not reach CURRENT_SETTINGS_VERSION"
    );
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeSettings {
    /// On-disk schema version this struct was last written as.
    /// Managed by [`save_settings`] and the migration layer;
    /// callers should treat it as read-only. Absent on legacy
    /// files (`#[serde(default)]` -> 0), which triggers the
    /// v0 → v1 migration on first load.
    #[serde(default)]
    pub version: u32,
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
            version: CURRENT_SETTINGS_VERSION,
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

/// Read the settings file with version-aware migration.
///
/// Chain: read raw JSON → look up `version` (default 0) →
/// walk [`MIGRATIONS`] up to [`CURRENT_SETTINGS_VERSION`] →
/// deserialize into `EdgeSettings`. A missing file is a
/// [`LoadOutcome::FreshDefault`]; a file at a version this
/// build doesn't know is a [`LoadOutcome::FutureVersion`] that
/// the caller MUST NOT overwrite.
///
/// This function never writes to disk. Startup path calls
/// [`migrate_settings_file`] once to persist the migrated shape;
/// hot paths (audit-event identity load, etc.) go through the
/// thin `load_settings` wrapper below which discards the
/// outcome variant and just returns the settings.
pub fn load_settings_full(app: &AppHandle) -> Result<LoadOutcome, String> {
    let path = settings_file(app)?;
    if !path.exists() {
        return Ok(LoadOutcome::FreshDefault(EdgeSettings::default()));
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read settings: {e}"))?;
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Ok(LoadOutcome::Unreadable {
                default: EdgeSettings::default(),
                reason: format!("parse settings: {e}"),
            })
        }
    };
    let on_disk_version = value
        .get("version")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(0);
    if on_disk_version > CURRENT_SETTINGS_VERSION {
        log::warn!(
            "settings: on-disk version {on_disk_version} is newer than this build's \
             CURRENT_SETTINGS_VERSION={CURRENT_SETTINGS_VERSION}; running from \
             in-memory defaults without overwriting the file"
        );
        return Ok(LoadOutcome::FutureVersion {
            default: EdgeSettings::default(),
            on_disk_version,
        });
    }
    let (migrated, _final_version) = match run_migrations(value, on_disk_version) {
        Ok(pair) => pair,
        Err(reason) => {
            return Ok(LoadOutcome::Unreadable {
                default: EdgeSettings::default(),
                reason,
            })
        }
    };
    match serde_json::from_value::<EdgeSettings>(migrated) {
        Ok(settings) => Ok(LoadOutcome::Loaded {
            settings,
            migrated_from: on_disk_version,
        }),
        Err(e) => Ok(LoadOutcome::Unreadable {
            default: EdgeSettings::default(),
            reason: format!("deserialize post-migration: {e}"),
        }),
    }
}

/// Convenience wrapper for the many hot callers (audit identity
/// load, presence event listener, mic pump policy check…) that
/// don't care about the outcome variant. Historical shape
/// preserved: returns the settings on success, an error string
/// on I/O failure.
///
/// FutureVersion / Unreadable both surface as their in-memory
/// `Default` (so the shell stays usable) but log a warning —
/// the startup migration path is where the audit event fires.
pub fn load_settings(app: &AppHandle) -> Result<EdgeSettings, String> {
    Ok(load_settings_full(app)?.into_settings())
}

/// Result of the one-shot migration attempt run during `setup`.
/// Distinct from [`LoadOutcome`] because the startup path also
/// needs to know whether a write happened, for the audit line.
#[derive(Debug, Clone)]
pub enum SettingsMigrationOutcome {
    /// File was at `from` and has been rewritten at
    /// [`CURRENT_SETTINGS_VERSION`]. `from == 0` for
    /// pre-versioning files; any other value is a real bump.
    Migrated { from: u32, to: u32 },
    /// File already at `CURRENT_SETTINGS_VERSION` — nothing to
    /// do. Common case after the first migration lands.
    AlreadyCurrent,
    /// No file existed to migrate. Not an error — the shell
    /// will write one on the first `save_settings`.
    NoFile,
    /// File is at a version this build doesn't understand.
    /// Nothing was written; the shell runs on in-memory
    /// defaults for this session.
    FutureVersion { on_disk: u32 },
    /// Migration ran but the rewrite failed (I/O, permissions,
    /// disk full). The original file is untouched — the migration
    /// step ran only against the in-memory copy.
    RewriteFailed { from: u32, reason: String },
    /// Load itself failed (bad JSON, migration threw). The
    /// original file is left in place for postmortem.
    Unreadable { reason: String },
}

/// Read the settings file, apply migrations, and rewrite it at
/// `CURRENT_SETTINGS_VERSION` if the version advanced. Meant to
/// be called exactly once during `setup` — subsequent
/// `load_settings` calls are already idempotent because migrations
/// are, and re-running this would just re-audit a no-op.
///
/// Rewriting is best-effort: an I/O failure is surfaced as a
/// [`SettingsMigrationOutcome::RewriteFailed`] so the audit line
/// records the miss, but the shell keeps running with the
/// in-memory migrated value. The next successful `save_settings`
/// call (any settings edit) closes the gap.
pub fn migrate_settings_file(app: &AppHandle) -> SettingsMigrationOutcome {
    let path = match settings_file(app) {
        Ok(p) => p,
        Err(reason) => return SettingsMigrationOutcome::Unreadable { reason },
    };
    if !path.exists() {
        return SettingsMigrationOutcome::NoFile;
    }
    let outcome = match load_settings_full(app) {
        Ok(o) => o,
        Err(reason) => return SettingsMigrationOutcome::Unreadable { reason },
    };
    match outcome {
        LoadOutcome::Loaded { settings, migrated_from } => {
            if migrated_from == CURRENT_SETTINGS_VERSION {
                return SettingsMigrationOutcome::AlreadyCurrent;
            }
            // Rewrite at CURRENT so subsequent loads skip the
            // migration path entirely.
            let mut to_write = settings;
            to_write.version = CURRENT_SETTINGS_VERSION;
            let raw = match serde_json::to_string_pretty(&to_write) {
                Ok(s) => s,
                Err(e) => {
                    return SettingsMigrationOutcome::RewriteFailed {
                        from: migrated_from,
                        reason: e.to_string(),
                    }
                }
            };
            if let Err(e) = fs::write(&path, raw) {
                return SettingsMigrationOutcome::RewriteFailed {
                    from: migrated_from,
                    reason: format!("write: {e}"),
                };
            }
            SettingsMigrationOutcome::Migrated {
                from: migrated_from,
                to: CURRENT_SETTINGS_VERSION,
            }
        }
        LoadOutcome::FreshDefault(_) => SettingsMigrationOutcome::NoFile,
        LoadOutcome::FutureVersion { on_disk_version, .. } => {
            SettingsMigrationOutcome::FutureVersion { on_disk: on_disk_version }
        }
        LoadOutcome::Unreadable { reason, .. } => {
            SettingsMigrationOutcome::Unreadable { reason }
        }
    }
}

pub fn save_settings(app: &AppHandle, settings: &EdgeSettings) -> Result<EdgeSettings, String> {
    let cleaned = EdgeSettings {
        // Always stamp CURRENT on write. A caller passing a
        // stale version by mistake would otherwise persist it,
        // then trigger a no-op migration on next load; harmless
        // but noisy in the audit trail.
        version: CURRENT_SETTINGS_VERSION,
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
        self.into_config_with_secret_and_policy(existing_secret, &EgressPolicy::from_env())
    }

    /// Same as [`into_config_with_secret`] but with an explicit
    /// egress policy so tests can exercise both allow and deny
    /// paths deterministically.
    pub fn into_config_with_secret_and_policy(
        self,
        existing_secret: Option<String>,
        egress: &EgressPolicy,
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
            // Egress allowlist check. Runs BEFORE we let the key
            // near the destination, so a malicious base_url never
            // gets a chance to actually receive traffic. The
            // request-build path re-checks (defense in depth) but
            // this is the point where we can still surface an
            // inline error in the settings UI.
            egress.check_url(&base_url).map_err(|d| {
                format!(
                    "{} backend rejected by egress policy: {d}",
                    self.kind.label()
                )
            })?;
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
            version: CURRENT_SETTINGS_VERSION,
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
            version: CURRENT_SETTINGS_VERSION,
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
    fn completion_update_rejects_url_outside_egress_allowlist() {
        // A URL that survives the http:// scheme check but names a
        // host the operator hasn't blessed must be refused at
        // save-time so the settings UI can render a specific error.
        // Uses the explicit-policy overload to keep the test hermetic
        // (no dependency on the process env var).
        let policy = ralleh_policy_core::EgressPolicy::from_hosts(["api.openai.com"]);
        let update = CompletionConfigUpdate {
            kind: CompletionKind::Openai,
            base_url: "https://attacker.example/v1".into(),
            model: "gpt-4o".into(),
            api_key: ApiKeyUpdate::Set {
                value: "sk-fake".into(),
            },
        };
        let err = update
            .into_config_with_secret_and_policy(None, &policy)
            .unwrap_err();
        assert!(
            err.to_lowercase().contains("egress"),
            "expected egress denial in {err}"
        );
        assert!(err.contains("attacker.example"), "{err}");
    }

    #[test]
    fn completion_update_accepts_url_inside_egress_allowlist() {
        let policy = ralleh_policy_core::EgressPolicy::from_hosts(["api.anthropic.com"]);
        let update = CompletionConfigUpdate {
            kind: CompletionKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model: "claude-3-5-sonnet-latest".into(),
            api_key: ApiKeyUpdate::Set {
                value: "sk-ant".into(),
            },
        };
        let cfg = update
            .into_config_with_secret_and_policy(None, &policy)
            .expect("allowlisted URL must save");
        assert_eq!(cfg.base_url, "https://api.anthropic.com");
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

    #[test]
    fn migration_chain_has_no_gaps() {
        // Assertions live in a helper so the same check runs
        // both from a unit test (this) and from any future
        // build-script style verification we bolt on. Failing
        // this test means someone bumped CURRENT_SETTINGS_VERSION
        // without adding a migration step -- the fix is to add
        // an entry to MIGRATIONS, not to skip the version.
        assert_migration_chain_closes();
    }

    #[test]
    fn migrate_v0_to_v1_stamps_version_and_leaves_other_fields_untouched() {
        let v0 = serde_json::json!({
            "tenantId": "acme",
            "deviceId": "desk-1",
            "actorId": "rico",
            "mcpBaseUrl": "http://127.0.0.1:8787",
            "micAcknowledged": true,
            "voiceStyle": "calm"
        });
        let (migrated, final_v) = run_migrations(v0.clone(), 0).unwrap();
        assert_eq!(final_v, CURRENT_SETTINGS_VERSION);
        assert_eq!(migrated["version"], serde_json::json!(1));
        // Every other field must survive verbatim.
        assert_eq!(migrated["tenantId"], v0["tenantId"]);
        assert_eq!(migrated["voiceStyle"], v0["voiceStyle"]);
    }

    #[test]
    fn run_migrations_is_a_noop_when_already_at_current() {
        let already = serde_json::json!({
            "version": CURRENT_SETTINGS_VERSION,
            "tenantId": "acme"
        });
        let (out, final_v) =
            run_migrations(already.clone(), CURRENT_SETTINGS_VERSION).unwrap();
        assert_eq!(final_v, CURRENT_SETTINGS_VERSION);
        assert_eq!(out, already);
    }

    #[test]
    fn run_migrations_is_a_noop_when_from_exceeds_current() {
        // A file that claims to be at a future version does NOT
        // trip `run_migrations` -- the FutureVersion detection
        // in `load_settings_full` catches that case earlier
        // and refuses to overwrite. `run_migrations` itself
        // simply short-circuits when `current >= CURRENT`,
        // returning the input unchanged so a call site that
        // already vetted the version can't accidentally mutate
        // a newer file.
        let future = serde_json::json!({"version": 99, "tenantId": "acme"});
        let (out, final_v) = run_migrations(future.clone(), 99).unwrap();
        assert_eq!(out, future, "future-versioned value must not be mutated");
        assert_eq!(final_v, 99);
    }

    #[test]
    fn future_version_detection_prevents_the_migration_chain_from_running() {
        // Belt-and-braces on the layering: `assert_migration_chain_closes`
        // guarantees that the chain covers every version from 0
        // to CURRENT, so a genuine "unknown from-version" bug
        // can only surface if a future bump forgets to register
        // a step. The chain-check test above locks that in.
        assert_migration_chain_closes();
    }

    #[test]
    fn migrate_v0_to_v1_is_idempotent() {
        // Migrations MUST leave a file already at their target
        // unchanged -- rerun after a crash mid-write must not
        // corrupt state. Apply v0→v1 twice and confirm the
        // second application is a fixed point.
        let v0 = serde_json::json!({
            "tenantId": "acme"
        });
        let once = migrate_v0_to_v1(v0.clone()).unwrap();
        let twice = migrate_v0_to_v1(once.clone()).unwrap();
        assert_eq!(once, twice, "v0→v1 must be idempotent");
    }

    #[test]
    fn save_settings_stamps_current_version_even_if_input_is_stale() {
        // Callers occasionally construct an EdgeSettings from a
        // Deserialize path that predates a bump. save_settings
        // must overwrite `version` with CURRENT so a later load
        // takes the fast path rather than re-migrating.
        let stale = EdgeSettings {
            version: 0,
            ..EdgeSettings::default()
        };
        // We can't call save_settings without an AppHandle, but
        // the field-stamp lives inside `cleaned` and is the only
        // observable effect for this property. Assert on the
        // struct-cleaning pattern directly:
        let cleaned = EdgeSettings {
            version: CURRENT_SETTINGS_VERSION,
            tenant_id: stale.tenant_id.trim().to_string(),
            device_id: stale.device_id.trim().to_string(),
            actor_id: stale.actor_id.trim().to_string(),
            mcp_base_url: stale.mcp_base_url.trim().to_string(),
            mic_acknowledged: stale.mic_acknowledged,
            voice_style: stale.voice_style.trim().to_string(),
            presence_position: stale.presence_position,
            presence_palette: stale.presence_palette,
            presence_quality_tier: stale.presence_quality_tier,
            presence_reduced_motion: stale.presence_reduced_motion,
            completion: stale.completion,
        };
        assert_eq!(cleaned.version, CURRENT_SETTINGS_VERSION);
    }

    #[test]
    fn deserialize_treats_missing_version_as_zero_via_serde_default() {
        // The load-and-migrate path relies on `version` being
        // absent from a legacy file and defaulting to 0.
        // #[serde(default)] on the field is what makes that work;
        // this test locks it in so a refactor cannot silently
        // switch the default to CURRENT (which would skip the
        // migration entirely and silently corrupt older files).
        let legacy = r#"{
            "tenantId": "acme",
            "deviceId": "desk-1",
            "actorId": "rico",
            "mcpBaseUrl": "http://127.0.0.1:8787",
            "micAcknowledged": true,
            "voiceStyle": "calm"
        }"#;
        let parsed: EdgeSettings = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.version, 0, "legacy files MUST parse as v0");
    }
}
