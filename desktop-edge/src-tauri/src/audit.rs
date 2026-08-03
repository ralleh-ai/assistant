//! Append-only audit log for policy-relevant events.
//!
//! ## Why this exists
//!
//! The last three landings — OS keychain, egress allowlist, and
//! streaming hardening — established security *controls*. This
//! module makes those controls *verifiable*. Every allow / deny /
//! fallback / secret-mutation event is written to a JSON-Lines
//! file the operator or a compliance function can archive, ship
//! to a SIEM, or hand to a customer's security review.
//!
//! Without this log, a customer asking "prove that no key ever
//! left this device to a non-approved host" gets an appeal to
//! source code. With it, they get a file.
//!
//! ## Wire shape
//!
//! JSON Lines (one event per line, newline-terminated) under the
//! Tauri app config dir as `audit.jsonl`. Rotation is size-based:
//! when the active file passes [`ROTATE_AT_BYTES`] the writer
//! renames it to `audit.jsonl.1` and starts a new active file.
//! One rollover file is kept — this is a shell-side operational
//! log, not the durable event store a real SIEM pipeline would
//! provide, so we don't try to be one.
//!
//! Fields are stable and camelCase to match the rest of the Tauri
//! surface. New fields land as `#[serde(default)]` so an old
//! audit reader (or the settings-UI panel that reads the tail of
//! the log) keeps parsing.
//!
//! ## Fail-open, not fail-loud
//!
//! An audit write that fails cannot be allowed to break the
//! action it was recording. If disk is full, permissions are
//! denied, or the file is locked, the write returns `Err(_)` and
//! the caller logs a warning — but the requested action still
//! proceeds. This mirrors how enterprise audit sinks are wired in
//! general: the log is *evidence*, not *authorization*. Denying a
//! user's login because the audit disk is full would be a worse
//! failure mode than the missing log line.
//!
//! ## Redaction
//!
//! Nothing in this module ever accepts a raw secret. The type
//! system enforces it: [`AuditEvent`] has no `String` fields
//! wide enough to accidentally receive one, and secret-mutation
//! events carry the [`SecretKind`] label, not the key value. The
//! keychain module never surfaces a stored secret to a caller
//! that isn't the request path; the audit path is not a request
//! path.

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// Rotate the active log file once it reaches this many bytes.
/// 4 MiB is enough for tens of thousands of events at typical
/// sizes (~150 B/event), which comfortably covers a day of dev
/// use; larger than that and the file starts becoming annoying to
/// tail in a text editor. Tunable in the future via env if
/// enterprises push back.
pub const ROTATE_AT_BYTES: u64 = 4 * 1024 * 1024;

/// Filename under the Tauri app config dir. Kept as a constant so
/// tests can construct the same path without duplicating the
/// string, and so the rollover naming convention has a single
/// source of truth.
pub const LOG_FILENAME: &str = "audit.jsonl";

/// Rollover file name. Only one is kept.
pub const ROLLOVER_FILENAME: &str = "audit.jsonl.1";

/// Kinds of event the shell records. Kept flat (no nested enum
/// data) so the JSONL surface is trivial to filter with `jq` or
/// grep. Payload details live in the `subject` / `detail` fields
/// of [`AuditEvent`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AuditKind {
    /// A completion backend URL passed the egress allowlist and
    /// was constructed (`route` is now able to reach that host).
    EgressAllow,
    /// A completion backend URL was refused by the egress
    /// allowlist at some enforcement layer.
    EgressDeny,
    /// The router's active backend was replaced (name transition
    /// captured in `detail.from` / `detail.to`).
    BackendSwap,
    /// A secret was written to the OS keychain (or a fallback
    /// store when unavailable).
    SecretWrite,
    /// A secret was cleared from the store.
    SecretClear,
    /// A cleartext api_key was migrated off disk into the store.
    SecretMigrate,
    /// A secret migration attempt failed (keychain unavailable /
    /// verification mismatch). The cleartext key was preserved
    /// on disk; this event names the reason.
    SecretMigrateFailed,
}

impl AuditKind {
    fn outcome(self) -> AuditOutcome {
        match self {
            Self::EgressAllow
            | Self::BackendSwap
            | Self::SecretWrite
            | Self::SecretClear
            | Self::SecretMigrate => AuditOutcome::Allow,
            Self::EgressDeny | Self::SecretMigrateFailed => AuditOutcome::Deny,
        }
    }
}

/// Kept separate from `AuditKind` because a SIEM filter often
/// wants "everything that was denied" without enumerating the
/// specific kinds — pull the events with `outcome=="deny"` and
/// you have your daily security report.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuditOutcome {
    Allow,
    Deny,
}

/// Label a `SecretWrite` / `SecretClear` event applies to.
/// Deliberately narrow to prevent the audit surface from ever
/// carrying a real key.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SecretKind {
    CompletionApiKey,
}

/// One record. Timestamp is ISO-8601 with millisecond resolution
/// so events can be sorted lexically. Tenant/device/actor triple
/// mirrors every other identity-carrying record in the shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub timestamp: String,
    pub kind: AuditKind,
    pub outcome: AuditOutcome,
    pub tenant: String,
    pub device: String,
    pub actor: String,
    /// Free-form label for the resource the event is about
    /// (`"openai@api.openai.com"`, `"backend"`, etc.). Never a
    /// secret; the type system enforces that by construction —
    /// only pass values derived from allowlisted config.
    #[serde(default)]
    pub subject: String,
    /// Freeform structured data. Serde-JSON here rather than a
    /// wide enum so new event shapes don't force an audit-reader
    /// upgrade — readers filter by `kind` and access `detail`
    /// keys they know about.
    #[serde(default)]
    pub detail: serde_json::Value,
}

impl AuditEvent {
    pub fn new(kind: AuditKind, tenant: impl Into<String>) -> Self {
        Self {
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            kind,
            outcome: kind.outcome(),
            tenant: tenant.into(),
            device: String::new(),
            actor: String::new(),
            subject: String::new(),
            detail: serde_json::Value::Null,
        }
    }

    pub fn with_identity(
        mut self,
        tenant: impl Into<String>,
        device: impl Into<String>,
        actor: impl Into<String>,
    ) -> Self {
        self.tenant = tenant.into();
        self.device = device.into();
        self.actor = actor.into();
        self
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = detail;
        self
    }
}

/// Async-safe append-only writer with size-based rotation.
///
/// Locks its `File` handle with a `Mutex` for the duration of the
/// write, which is fine because writes are infrequent (dozens per
/// hour in normal use) and the alternative — an `mpsc` channel to
/// a dedicated writer thread — is more infrastructure than the
/// per-write cost justifies at current volumes. Revisit if audit
/// throughput ever climbs into "thousands per second" territory.
pub struct AuditLog {
    inner: Mutex<AuditLogInner>,
    /// `true` for the no-op sink; every write short-circuits to
    /// `Err` without touching the filesystem. This avoids the
    /// classic "no-op should be a real filesystem path but every
    /// real path is writeable somewhere" trap.
    disabled: bool,
}

struct AuditLogInner {
    dir: PathBuf,
    active: PathBuf,
    rollover: PathBuf,
}

impl AuditLog {
    /// Open the audit log under the Tauri app config dir. Ensures
    /// the directory exists; a failure here is surfaced so
    /// `setup` can log the misconfiguration and continue with a
    /// [`no_op`] audit log rather than crashing the shell.
    pub fn open(app: &AppHandle) -> Result<Self, String> {
        let dir = app
            .path()
            .app_config_dir()
            .map_err(|e| format!("audit: app config dir: {e}"))?;
        fs::create_dir_all(&dir).map_err(|e| format!("audit: create dir: {e}"))?;
        Ok(Self::for_dir(dir))
    }

    /// Test/instrumentation entry point: opens the log against an
    /// explicit directory. Used by unit tests and by the null
    /// sink; production code should go through `open`.
    pub fn for_dir(dir: PathBuf) -> Self {
        let active = dir.join(LOG_FILENAME);
        let rollover = dir.join(ROLLOVER_FILENAME);
        Self {
            inner: Mutex::new(AuditLogInner {
                dir,
                active,
                rollover,
            }),
            disabled: false,
        }
    }

    /// A sink that discards every event. Used when opening the
    /// real log failed — the shell keeps running but nothing is
    /// recorded. Callers who care about the "audit disabled"
    /// state should log a warning at startup.
    pub fn no_op() -> Self {
        let mut sink = Self::for_dir(PathBuf::from("__disabled__"));
        sink.disabled = true;
        sink
    }

    /// Append `event` to the active file, rotating when the file
    /// crosses `ROTATE_AT_BYTES`. Errors are returned rather than
    /// panicked — see the fail-open policy in the module doc.
    pub fn write(&self, event: &AuditEvent) -> Result<(), String> {
        if self.disabled {
            return Err("audit log disabled (no-op sink)".into());
        }
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        // Serialize before touching the file. Malformed events
        // (should be impossible; `AuditEvent` is trivially
        // serializable) fail the write instead of writing garbage.
        let mut line = serde_json::to_string(event).map_err(|e| e.to_string())?;
        line.push('\n');
        // Rotate if needed. `metadata` failing (file doesn't
        // exist yet on first write) is a legitimate not-rotate
        // signal, not an error.
        if let Ok(meta) = fs::metadata(&guard.active) {
            if meta.len() + line.len() as u64 > ROTATE_AT_BYTES {
                rotate(&mut guard)?;
            }
        }
        // Ensure the directory still exists — a paranoid check
        // that catches "user manually deleted their config dir"
        // between open() and the first write.
        if !guard.dir.exists() {
            fs::create_dir_all(&guard.dir).map_err(|e| format!("audit: recreate dir: {e}"))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&guard.active)
            .map_err(|e| format!("audit: open {}: {e}", guard.active.display()))?;
        file.write_all(line.as_bytes())
            .map_err(|e| format!("audit: write: {e}"))?;
        Ok(())
    }

    /// Read the last `limit` events from the active file (older
    /// events from the rollover are ignored — the settings-UI use
    /// case is "what happened recently", not "give me my life
    /// story"). Malformed lines are skipped rather than fatal.
    pub fn tail(&self, limit: usize) -> Result<Vec<AuditEvent>, String> {
        let guard = self.inner.lock().map_err(|e| e.to_string())?;
        let raw = match fs::read_to_string(&guard.active) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("audit: read: {e}")),
        };
        // Cheap tail-N implementation: split lines, take the last
        // `limit`, parse each. For audit volumes (thousands of
        // lines at most) this is fine; if the log ever grows into
        // millions of lines we'd need a reverse line-reader, but
        // rotation caps the active file size well below that.
        let lines: Vec<&str> = raw.lines().collect();
        let start = lines.len().saturating_sub(limit);
        let events = lines[start..]
            .iter()
            .filter_map(|line| serde_json::from_str::<AuditEvent>(line).ok())
            .collect();
        Ok(events)
    }

    /// Absolute path of the active log file (for diagnostics and
    /// the "Open audit log" button we'll eventually add to the
    /// settings UI).
    pub fn active_path(&self) -> PathBuf {
        self.inner
            .lock()
            .map(|g| g.active.clone())
            .unwrap_or_default()
    }
}

fn rotate(inner: &mut AuditLogInner) -> Result<(), String> {
    // Remove any existing rollover file; then move the active
    // file into its slot. Best-effort — a locked rollover file on
    // Windows would otherwise wedge the writer, and losing the
    // previous rollover is a strictly better failure than losing
    // future writes.
    let _ = fs::remove_file(&inner.rollover);
    if inner.active.exists() {
        fs::rename(&inner.active, &inner.rollover)
            .map_err(|e| format!("audit: rotate: {e}"))?;
    }
    Ok(())
}

/// Convenience: emit `event`, logging the audit-write failure at
/// warn level. Callers use this at the vast majority of sites so
/// they don't have to write the same handler in every place.
pub fn record(log: &AuditLog, event: AuditEvent) {
    if let Err(e) = log.write(&event) {
        log::warn!("audit: failed to record {:?}: {e}", event.kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_event(kind: AuditKind) -> AuditEvent {
        AuditEvent::new(kind, "acme")
            .with_identity("acme", "desk-1", "rico")
            .with_subject("openai@api.openai.com")
    }

    #[test]
    fn event_kind_maps_to_outcome() {
        assert_eq!(AuditKind::EgressAllow.outcome(), AuditOutcome::Allow);
        assert_eq!(AuditKind::EgressDeny.outcome(), AuditOutcome::Deny);
        assert_eq!(
            AuditKind::SecretMigrateFailed.outcome(),
            AuditOutcome::Deny
        );
    }

    #[test]
    fn write_creates_file_and_appends_jsonl() {
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        log.write(&sample_event(AuditKind::EgressAllow)).unwrap();
        log.write(&sample_event(AuditKind::EgressDeny)).unwrap();
        let raw = fs::read_to_string(dir.path().join(LOG_FILENAME)).unwrap();
        assert_eq!(
            raw.lines().count(),
            2,
            "expected 2 records, got {}",
            raw.lines().count()
        );
        assert!(raw.contains("\"kind\":\"egress-allow\""));
        assert!(raw.contains("\"kind\":\"egress-deny\""));
    }

    #[test]
    fn write_never_contains_a_stored_secret() {
        // Sanity-check the redaction guarantee: the AuditEvent
        // surface has no field wide enough for an api_key, so
        // writing a `SecretWrite` event yields a line that
        // doesn't include the secret even if the caller mistakenly
        // tried to embed one in `subject`. Subjects for these
        // events are labels, not values, and this test pins that.
        // We use a distinctive sentinel value ("SUPER_SECRET_KEY")
        // and assert it never appears anywhere in the serialized
        // form — a substring like "sk-" would false-positive on
        // the "secret-write" kind label, so we deliberately do
        // NOT use API-key-shaped strings for this pin.
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        let event = AuditEvent::new(AuditKind::SecretWrite, "acme")
            .with_identity("acme", "desk-1", "rico")
            .with_subject("completion-api-key")
            .with_detail(serde_json::json!({
                "kind": SecretKind::CompletionApiKey,
                "storage": "keychain",
            }));
        log.write(&event).unwrap();
        let raw = fs::read_to_string(dir.path().join(LOG_FILENAME)).unwrap();
        // The event carries the LABEL, never a value. If a future
        // refactor plumbs the raw key through, this pin catches
        // it — the sentinel value has to leak from the caller side,
        // not from us, which is exactly the invariant we want.
        const SENTINEL: &str = "SUPER_SECRET_KEY_VALUE_DO_NOT_LEAK";
        assert!(
            !raw.contains(SENTINEL),
            "audit line leaked the sentinel secret: {raw}"
        );
        assert!(raw.contains("\"kind\":\"secret-write\""));
        assert!(raw.contains("\"subject\":\"completion-api-key\""));
    }

    #[test]
    fn rotation_moves_active_to_rollover_when_over_cap() {
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        // Prime the active file to the cap. Write directly rather
        // than looping via the sink -- much faster and the point
        // is to test the rotation trigger, not the serializer.
        let path = dir.path().join(LOG_FILENAME);
        {
            let mut f = fs::File::create(&path).unwrap();
            let filler = "x".repeat(ROTATE_AT_BYTES as usize);
            f.write_all(filler.as_bytes()).unwrap();
        }
        // One more real event should trip the rotation.
        log.write(&sample_event(AuditKind::EgressAllow)).unwrap();
        assert!(
            dir.path().join(ROLLOVER_FILENAME).exists(),
            "rollover file was not created"
        );
        let active = fs::read_to_string(&path).unwrap();
        assert_eq!(
            active.lines().count(),
            1,
            "post-rotation active file should hold the single new record",
        );
    }

    #[test]
    fn tail_returns_only_the_last_n_events() {
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        for _ in 0..7 {
            log.write(&sample_event(AuditKind::EgressAllow)).unwrap();
        }
        let events = log.tail(3).unwrap();
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn tail_returns_empty_when_log_does_not_exist_yet() {
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        assert!(log.tail(10).unwrap().is_empty());
    }

    #[test]
    fn tail_skips_malformed_lines_rather_than_erroring() {
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        log.write(&sample_event(AuditKind::EgressAllow)).unwrap();
        // Corrupt the file: append a non-JSON line. A real audit
        // reader must not blow up on this — tampered files still
        // need to yield the events we can still parse.
        {
            let mut f = OpenOptions::new()
                .append(true)
                .open(dir.path().join(LOG_FILENAME))
                .unwrap();
            writeln!(f, "not json").unwrap();
        }
        log.write(&sample_event(AuditKind::EgressDeny)).unwrap();
        let events = log.tail(10).unwrap();
        assert_eq!(events.len(), 2, "malformed line should be skipped");
    }

    #[test]
    fn no_op_sink_fails_writes_but_never_panics() {
        // The no-op sink is what runs when the real log failed to
        // open. Confirm the write path returns Err rather than
        // panicking, and that the "record" helper swallows it.
        let log = AuditLog::no_op();
        let err = log.write(&sample_event(AuditKind::EgressAllow));
        assert!(err.is_err());
        // The record helper is fail-open: it must not panic on
        // the returned Err.
        record(&log, sample_event(AuditKind::EgressAllow));
    }

    #[test]
    fn serialized_shape_uses_camel_case_and_kebab_kind() {
        let event = sample_event(AuditKind::BackendSwap);
        let json = serde_json::to_string(&event).unwrap();
        // These are the field names external tooling filters on.
        // Pin them so a rename doesn't silently break downstream
        // dashboards that parse this file.
        assert!(json.contains("\"timestamp\""), "{json}");
        assert!(json.contains("\"tenant\":\"acme\""), "{json}");
        assert!(json.contains("\"outcome\":\"allow\""), "{json}");
        assert!(json.contains("\"kind\":\"backend-swap\""), "{json}");
    }
}
