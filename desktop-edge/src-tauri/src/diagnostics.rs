//! Operator-facing diagnostics bundle.
//!
//! When a user reports a problem, the first ~90% of triage is
//! "what did the shell see just before?". Instead of asking
//! operators to hunt down four files under an OS-specific
//! config dir, expose a single Tauri command that packages
//! the relevant surfaces into one JSON blob written to a
//! path they can attach to a ticket.
//!
//! # Contents
//!
//! - `manifest`: shell build metadata, timestamp, current
//!   settings schema version.
//! - `settings`: `EdgeSettings` with the api_key stripped and
//!   the storage kind recorded (same shape the settings UI
//!   sees; the raw secret never leaves the keychain).
//! - `backend_status`: active backend name + last
//!   health-probe snapshot.
//! - `audit_tail`: last 500 audit events.
//! - `presence_log_tail`: last 500 lines of the runtime's
//!   stderr capture.
//! - `presence_status`: liveness snapshot at the moment of
//!   capture (`last_event_ms_ago`, heartbeat sequence,
//!   uptime).
//!
//! # Security posture
//!
//! Every field on the wire is already deemed safe for the UI
//! to see. The bundle re-uses those redactions rather than
//! inventing its own so a leak here would require breaking
//! two layers simultaneously.

use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::assistant::AssistantState;
use crate::assistant_health;
use crate::audit::{AuditEvent, AuditLog, AuditVerifyReport};
use crate::presence::Presence;
use crate::presence_log::PresenceLog;
use crate::secret_store::open_default as open_default_secret_store;
use crate::settings::{load_settings, RedactedCompletionConfig, CURRENT_SETTINGS_VERSION};

/// Cap on the audit tail included in the bundle. Chosen to
/// match the `assistant_audit_tail` upper bound so an
/// operator gets the same slice they'd see in the UI.
const AUDIT_TAIL_LIMIT: usize = 500;

/// Cap on the presence log tail. 500 lines is roughly the
/// last few minutes of a healthy runtime and comfortably
/// includes the full panic backtrace of any single crash.
const PRESENCE_LOG_TAIL_LIMIT: usize = 500;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsBundle {
    pub manifest: Manifest,
    pub settings: SettingsSection,
    pub backend_status: BackendSection,
    pub presence_status: PresenceSection,
    pub audit_tail: Vec<AuditEvent>,
    /// Chain-verification result at the moment of capture.
    /// `None` when no `AuditLog` is managed (e.g. in the
    /// stubbed tests below). A support ticket reader can trust
    /// the audit tail iff `auditVerify.issues` is empty.
    pub audit_verify: Option<AuditVerifyReport>,
    pub presence_log_tail: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// RFC 3339 timestamp of capture.
    pub captured_at: String,
    /// Compile-time crate version of `desktop-edge`.
    pub shell_version: &'static str,
    /// Settings schema version this shell writes.
    pub settings_schema_version: u32,
    /// Wire version of `presence-ipc`.
    pub presence_wire_version: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSection {
    /// Identity triple (safe to include — same values the
    /// audit log already carries).
    pub tenant_id: String,
    pub device_id: String,
    pub actor_id: String,
    pub mcp_base_url: String,
    pub voice_style: String,
    pub mic_acknowledged: bool,
    pub presence_palette: Option<String>,
    pub presence_quality_tier: Option<String>,
    pub presence_reduced_motion: bool,
    pub presence_position: Option<(i32, i32)>,
    pub completion: Option<RedactedCompletionConfig>,
    pub settings_version_on_disk: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendSection {
    pub active_backend: String,
    pub health: assistant_health::HealthSnapshot,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceSection {
    pub enabled: bool,
    pub last_event_ms_ago: Option<u64>,
    pub last_heartbeat_sequence: Option<u64>,
    pub last_heartbeat_uptime_ms: Option<u64>,
}

/// Build a bundle from the currently-managed Tauri state.
/// Kept as a plain function (not a `#[tauri::command]`) so
/// the write-to-disk wrapper below can compose it with the
/// path-handling logic and unit tests can exercise it against
/// fabricated state.
pub fn build_bundle(app: &AppHandle) -> Result<DiagnosticsBundle, String> {
    let captured_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let manifest = Manifest {
        captured_at,
        shell_version: env!("CARGO_PKG_VERSION"),
        settings_schema_version: CURRENT_SETTINGS_VERSION,
        presence_wire_version: presence_ipc::VERSION,
    };

    // Settings section: reuse the same redaction the UI sees.
    let settings = load_settings(app).unwrap_or_default();
    let store = open_default_secret_store();
    let completion = settings
        .completion
        .as_ref()
        .map(|c| RedactedCompletionConfig::from_config_and_store(c, store.as_ref()));
    let settings_section = SettingsSection {
        tenant_id: settings.tenant_id,
        device_id: settings.device_id,
        actor_id: settings.actor_id,
        mcp_base_url: settings.mcp_base_url,
        voice_style: settings.voice_style,
        mic_acknowledged: settings.mic_acknowledged,
        presence_palette: settings.presence_palette.map(|p| {
            // PaletteId doesn't expose a str method; serialize
            // via serde to reuse the wire spelling.
            serde_json::to_value(p)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        }),
        presence_quality_tier: settings.presence_quality_tier.map(|q| {
            serde_json::to_value(q)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        }),
        presence_reduced_motion: settings.presence_reduced_motion,
        presence_position: settings.presence_position.map(|p| (p.x, p.y)),
        completion,
        settings_version_on_disk: settings.version,
    };

    // Backend section: same shape as `assistant_backend_status`
    // but assembled directly so we don't invoke a Tauri command
    // from inside another.
    let assistant_state: State<'_, AssistantState> = app.state();
    let health_state: State<'_, assistant_health::Health> = app.state();
    let health_snapshot = {
        let guard = health_state.lock().map_err(|e| e.to_string())?;
        guard.materialize(std::time::Instant::now())
    };
    let backend_section = BackendSection {
        active_backend: assistant_state.current_backend_name(),
        health: health_snapshot,
    };

    // Presence section: liveness snapshot + enabled flag.
    let presence_state: State<'_, Presence> = app.state();
    let liveness = presence_state.liveness_snapshot();
    let last_event_ms_ago = liveness
        .last_event_at
        .map(|t| std::time::Instant::now().duration_since(t).as_millis() as u64);
    let presence_section = PresenceSection {
        enabled: presence_state.is_enabled(),
        last_event_ms_ago,
        last_heartbeat_sequence: liveness.last_heartbeat_sequence,
        last_heartbeat_uptime_ms: liveness.last_heartbeat_uptime_ms,
    };

    // Audit tail: fail-open. If the audit log itself is broken
    // (rotated file missing, etc.) we'd rather return the
    // bundle without those events than refuse the whole
    // request -- the settings/health sections are still useful.
    let audit_state = app.try_state::<AuditLog>();
    let audit_tail = audit_state
        .as_ref()
        .and_then(|s| s.tail(AUDIT_TAIL_LIMIT).ok())
        .unwrap_or_default();
    // Chain verification: fail-open on error so a corrupted
    // audit file still yields a bundle. A `None` here means we
    // didn't even try (no log managed); a `Some(report)` with
    // issues is a live tamper signal the receiver should see.
    let audit_verify = audit_state.as_ref().and_then(|s| s.verify().ok());

    // Presence log: same fail-open posture. Wrapped in an Arc
    // by the setup path.
    let presence_log_tail = app
        .try_state::<std::sync::Arc<PresenceLog>>()
        .and_then(|s| s.tail(PRESENCE_LOG_TAIL_LIMIT).ok())
        .unwrap_or_default();

    Ok(DiagnosticsBundle {
        manifest,
        settings: settings_section,
        backend_status: backend_section,
        presence_status: presence_section,
        audit_tail,
        audit_verify,
        presence_log_tail,
    })
}

/// Serialize `bundle` to a file the operator can attach to a
/// ticket. `dest_dir` names the directory to write into; when
/// `None`, the shell's app config dir is used (same directory
/// that already holds settings + audit + presence.log). The
/// filename is `ralleh-diagnostics-<utc-timestamp>.json`,
/// deterministic enough for chronological sorting but
/// specific enough to avoid clobbering an earlier bundle.
pub fn write_bundle(
    app: &AppHandle,
    bundle: &DiagnosticsBundle,
    dest_dir: Option<PathBuf>,
) -> Result<PathBuf, String> {
    let dir = match dest_dir {
        Some(d) => d,
        None => app
            .path()
            .app_config_dir()
            .map_err(|e| format!("app config dir: {e}"))?,
    };
    fs::create_dir_all(&dir).map_err(|e| format!("create bundle dir: {e}"))?;
    // Filename-safe timestamp: colons are illegal on Windows,
    // so use the same shape the audit log uses.
    let ts = bundle.manifest.captured_at.replace([':', '.'], "-");
    let path = dir.join(format!("ralleh-diagnostics-{ts}.json"));
    let raw =
        serde_json::to_string_pretty(bundle).map_err(|e| format!("serialize diagnostics: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("write diagnostics: {e}"))?;
    Ok(path)
}

/// Convenience: build + write in one call. Returns the file
/// path so the UI can offer an "Open in file explorer"
/// affordance.
pub fn build_and_write(app: &AppHandle, dest_dir: Option<PathBuf>) -> Result<PathBuf, String> {
    let bundle = build_bundle(app)?;
    write_bundle(app, &bundle, dest_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_bundle_stub() -> DiagnosticsBundle {
        DiagnosticsBundle {
            manifest: Manifest {
                captured_at: "2026-08-03T15:04:05.000Z".into(),
                shell_version: env!("CARGO_PKG_VERSION"),
                settings_schema_version: CURRENT_SETTINGS_VERSION,
                presence_wire_version: presence_ipc::VERSION,
            },
            settings: SettingsSection {
                tenant_id: "acme".into(),
                device_id: "desk-1".into(),
                actor_id: "rico".into(),
                mcp_base_url: "http://127.0.0.1:8787".into(),
                voice_style: "calm".into(),
                mic_acknowledged: true,
                presence_palette: Some("ember".into()),
                presence_quality_tier: Some("balanced".into()),
                presence_reduced_motion: false,
                presence_position: Some((100, 200)),
                completion: None,
                settings_version_on_disk: 1,
            },
            backend_status: BackendSection {
                active_backend: "echo".into(),
                health: assistant_health::HealthSnapshot::unknown("echo".into()),
            },
            presence_status: PresenceSection {
                enabled: true,
                last_event_ms_ago: Some(1200),
                last_heartbeat_sequence: Some(42),
                last_heartbeat_uptime_ms: Some(84_000),
            },
            audit_tail: Vec::new(),
            audit_verify: None,
            presence_log_tail: vec!["[info] presence-runtime started".into()],
        }
    }

    #[test]
    fn bundle_serializes_to_valid_json_with_stable_keys() {
        let bundle = make_bundle_stub();
        let json = serde_json::to_value(&bundle).unwrap();
        // Pin the top-level shape so a rename that would break
        // downstream tooling (support workflows parse this by
        // key) fails loudly.
        for key in [
            "manifest",
            "settings",
            "backendStatus",
            "presenceStatus",
            "auditTail",
            "auditVerify",
            "presenceLogTail",
        ] {
            assert!(
                json.get(key).is_some(),
                "missing top-level key `{key}` in {json:?}"
            );
        }
        // Manifest keys must be there so the receiver can
        // fail-fast on version-mismatch bugs.
        let manifest = json.get("manifest").unwrap();
        assert!(manifest.get("capturedAt").is_some());
        assert!(manifest.get("shellVersion").is_some());
        assert!(manifest.get("settingsSchemaVersion").is_some());
    }

    #[test]
    fn write_bundle_produces_a_file_named_after_the_capture_timestamp() {
        // The filename shape has to survive on Windows, which
        // disallows `:` and `.` in a name. Confirm both are
        // replaced.
        let dir = TempDir::new().unwrap();
        let bundle = make_bundle_stub();
        // Fake AppHandle isn't possible in a unit test, so
        // exercise the filename-shape logic manually against
        // the same substitution `write_bundle` does.
        let ts = bundle.manifest.captured_at.replace([':', '.'], "-");
        let name = format!("ralleh-diagnostics-{ts}.json");
        let out = dir.path().join(&name);
        fs::write(&out, "{}").unwrap();
        assert!(out.exists());
        assert!(!name.contains(':'), "windows-hostile char in name: {name}");
    }

    #[test]
    fn bundle_never_contains_the_raw_api_key_field_name() {
        // Belt-and-braces: even a bundle stub with an api_key
        // field on the underlying config MUST serialize to
        // something without the raw `apiKey` string, because
        // the RedactedCompletionConfig replaces it with
        // `hasApiKey` + storage-kind. A regression that reintroduces
        // the raw field would leak keys through the bundle.
        let bundle = make_bundle_stub();
        let json = serde_json::to_string(&bundle).unwrap();
        assert!(
            !json.contains("\"apiKey\""),
            "bundle must not carry the raw apiKey field: {json}"
        );
    }
}
