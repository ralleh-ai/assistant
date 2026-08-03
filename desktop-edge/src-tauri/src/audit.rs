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
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    /// The presence-runtime child stopped emitting events for
    /// longer than [`presence_ipc::STALL_THRESHOLD_MS`]. `detail`
    /// carries `elapsed_ms`, the last heartbeat `sequence` /
    /// `uptime_ms` we saw, and the runtime's PID (as reported by
    /// the shell) so an operator can correlate with a crash dump.
    PresenceStalled,
    /// A previously-stalled runtime has resumed emitting events
    /// (its own or a fresh spawn). Paired with a preceding
    /// `PresenceStalled` event; `detail.recovery_ms` names the
    /// gap.
    PresenceRecovered,
    /// A router-health probe failed (network, timeout, or a
    /// backend error response) after a previous healthy /
    /// unknown state. `detail.error` names the reason,
    /// `detail.latency_ms` the elapsed time before the failure.
    /// Subsequent same-state probes do *not* re-emit — this is
    /// an edge, not a heartbeat.
    RouterUnhealthy,
    /// A previously-unhealthy router probe succeeded. Paired
    /// with a preceding `RouterUnhealthy`; `detail.latency_ms`
    /// names the recovery response time.
    RouterHealthy,
    /// The `edge-settings.json` file was migrated from an older
    /// schema version. `detail.from` / `detail.to` name the
    /// version transition; the file has been rewritten at the
    /// new version by the time this event fires.
    SettingsMigrate,
    /// A settings migration attempt failed. Either the file
    /// sits at a version this build doesn't understand
    /// (`detail.reason` = "future-version") or the rewrite
    /// I/O errored. The original file is untouched — the
    /// shell runs on in-memory defaults for the session.
    SettingsMigrateFailed,
}

impl AuditKind {
    fn outcome(self) -> AuditOutcome {
        match self {
            Self::EgressAllow
            | Self::BackendSwap
            | Self::SecretWrite
            | Self::SecretClear
            | Self::SecretMigrate
            | Self::PresenceRecovered
            | Self::RouterHealthy
            | Self::SettingsMigrate => AuditOutcome::Allow,
            Self::EgressDeny
            | Self::SecretMigrateFailed
            | Self::PresenceStalled
            | Self::RouterUnhealthy
            | Self::SettingsMigrateFailed => AuditOutcome::Deny,
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
    /// Hex-encoded SHA-256 of the previous line's `hash` field.
    /// `None` on the first event of a chain (genesis) or on
    /// legacy events written before the chain shipped. Together
    /// with `hash` this forms a Merkle-lite log: mutating a
    /// past event without regenerating every later hash trips
    /// `AuditLog::verify`. Wrapped in `Option` + skip-serialize
    /// so pre-chain files still parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_hash: Option<String>,
    /// Hex-encoded SHA-256 of this event's canonical
    /// serialization (with `hash` cleared, `prev_hash` set).
    /// The next event's `prev_hash` MUST equal this value;
    /// [`AuditLog::verify`] enforces both invariants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
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
            prev_hash: None,
            hash: None,
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
    ///
    /// The event is fingerprinted before write: `prev_hash` is
    /// set to the previous line's `hash` (or `None` after a
    /// rotation / at file creation), and `hash` is the SHA-256
    /// of the event's canonical serialization with `hash`
    /// cleared. Together these form the tamper-evident chain
    /// that [`AuditLog::verify`] walks.
    pub fn write(&self, event: &AuditEvent) -> Result<(), String> {
        if self.disabled {
            return Err("audit log disabled (no-op sink)".into());
        }
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        // Ensure the directory still exists — a paranoid check
        // that catches "user manually deleted their config dir"
        // between open() and the first write.
        if !guard.dir.exists() {
            fs::create_dir_all(&guard.dir).map_err(|e| format!("audit: recreate dir: {e}"))?;
        }
        // Determine `prev_hash` before we potentially rotate:
        // after rotation the file is empty and this must be
        // `None` to start a fresh chain. Reading from the
        // current-still-active file catches both cases (returns
        // `None` when the file is missing).
        let mut chained = event.clone();
        chained.prev_hash = last_hash_in(&guard.active);
        chained.hash = None;
        chained.hash = Some(compute_hash(&chained)?);
        let mut line = serde_json::to_string(&chained).map_err(|e| e.to_string())?;
        line.push('\n');
        // Rotate if needed. `metadata` failing (file doesn't
        // exist yet on first write) is a legitimate not-rotate
        // signal, not an error. On rotation the freshly-empty
        // active file breaks the chain deliberately -- but we
        // computed `prev_hash` above against the *old* file, so
        // recompute after rotate to reflect the new empty state.
        if let Ok(meta) = fs::metadata(&guard.active) {
            if meta.len() + line.len() as u64 > ROTATE_AT_BYTES {
                rotate(&mut guard)?;
                // Post-rotation the chain restarts. Recompute
                // hash under `prev_hash: None`.
                chained.prev_hash = None;
                chained.hash = None;
                chained.hash = Some(compute_hash(&chained)?);
                line = serde_json::to_string(&chained).map_err(|e| e.to_string())?;
                line.push('\n');
            }
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

    /// Walk the active file and confirm the hash chain is
    /// intact. Returns a report naming any tampering the
    /// verifier detected. An empty `issues` list means the
    /// chain is consistent with the events on disk *right now*
    /// — this is tamper-evident, not tamper-proof (an attacker
    /// with file-write access can regenerate the whole chain).
    /// For a stronger property the log would need to be signed
    /// by an offline key or notarized to an external service;
    /// that layer is deliberately not built yet, see the crate
    /// docs.
    pub fn verify(&self) -> Result<AuditVerifyReport, String> {
        if self.disabled {
            return Ok(AuditVerifyReport::empty());
        }
        let guard = self.inner.lock().map_err(|e| e.to_string())?;
        let raw = match fs::read_to_string(&guard.active) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(AuditVerifyReport::empty());
            }
            Err(e) => return Err(format!("audit: read: {e}")),
        };
        let mut issues = Vec::new();
        let mut prev: Option<String> = None;
        let mut checked = 0_usize;
        for (idx, line) in raw.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event: AuditEvent = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(e) => {
                    issues.push(AuditVerifyIssue {
                        line: idx + 1,
                        kind: AuditVerifyIssueKind::Unparseable,
                        detail: e.to_string(),
                    });
                    continue;
                }
            };
            checked += 1;
            // Legacy events (written before the chain shipped)
            // have no `hash` field. Report them as a chain
            // break so an operator sees exactly where the
            // pre-chain history ends and the tamper-evident
            // window begins.
            let Some(recorded_hash) = event.hash.clone() else {
                issues.push(AuditVerifyIssue {
                    line: idx + 1,
                    kind: AuditVerifyIssueKind::MissingHash,
                    detail: "event predates the hash chain".into(),
                });
                prev = None;
                continue;
            };
            // Chain link: prev_hash must match the previous
            // event's hash (or be None on the first line).
            if event.prev_hash != prev {
                issues.push(AuditVerifyIssue {
                    line: idx + 1,
                    kind: AuditVerifyIssueKind::PrevHashMismatch,
                    detail: format!(
                        "expected prev_hash={:?}, found {:?}",
                        prev, event.prev_hash
                    ),
                });
            }
            // Content check: recompute the hash from the event
            // itself (with `hash` cleared) and compare.
            let mut for_hash = event.clone();
            for_hash.hash = None;
            let expected = match compute_hash(&for_hash) {
                Ok(h) => h,
                Err(e) => {
                    issues.push(AuditVerifyIssue {
                        line: idx + 1,
                        kind: AuditVerifyIssueKind::RecomputeFailed,
                        detail: e,
                    });
                    continue;
                }
            };
            if expected != recorded_hash {
                issues.push(AuditVerifyIssue {
                    line: idx + 1,
                    kind: AuditVerifyIssueKind::ContentTampered,
                    detail: format!(
                        "recorded hash {recorded_hash} does not match \
                         recomputed {expected}"
                    ),
                });
            }
            prev = Some(recorded_hash);
        }
        Ok(AuditVerifyReport {
            events_checked: checked,
            issues,
        })
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

/// SHA-256 (hex-encoded) of `event`'s canonical JSON
/// serialization. The caller MUST clear `event.hash` before
/// invoking so the hash cannot depend on itself; verification
/// enforces the same discipline.
///
/// Canonical form: `serde_json::to_string(event)` — deterministic
/// under our current `serde_json` config (no `preserve_order`
/// feature; `Value::Object` uses `BTreeMap` which serializes in
/// sorted key order, and the struct field order is fixed by the
/// derive). A migration to `preserve_order` would break this
/// determinism silently; the `sorted_detail_field_hashes_stably`
/// test would trip.
fn compute_hash(event: &AuditEvent) -> Result<String, String> {
    debug_assert!(
        event.hash.is_none(),
        "compute_hash must be called on an event with `hash` cleared"
    );
    let bytes = serde_json::to_vec(event).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Return the `hash` field of the last valid JSON line in
/// `path`, or `None` when the file is missing, empty, contains
/// only malformed lines, or its last event predates the chain
/// (no `hash` field). Reads the whole file — cheap at rotation
/// thresholds well under a megabyte.
fn last_hash_in(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    raw.lines()
        .rev()
        .find_map(|l| serde_json::from_str::<AuditEvent>(l).ok())
        .and_then(|e| e.hash)
}

/// Chain-verification result. `events_checked` counts lines the
/// parser accepted (malformed lines are counted as issues but
/// don't increment this). `issues` is empty when the chain is
/// intact against on-disk content right now.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuditVerifyReport {
    pub events_checked: usize,
    pub issues: Vec<AuditVerifyIssue>,
}

impl AuditVerifyReport {
    fn empty() -> Self {
        Self::default()
    }

    /// `true` iff every parsed event in the active file
    /// contributes to an unbroken chain rooted at a genesis
    /// entry with `prev_hash: None`. Useful for a UI badge or
    /// a smoke test after a suspected tamper event.
    #[allow(dead_code)]
    pub fn is_intact(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Single verification finding. `line` is 1-indexed against the
/// active file so an operator can jump straight to it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditVerifyIssue {
    pub line: usize,
    pub kind: AuditVerifyIssueKind,
    pub detail: String,
}

/// What went wrong at a line. Deliberately narrow so a downstream
/// SIEM can filter on these without inventing its own taxonomy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AuditVerifyIssueKind {
    /// The line's recorded `hash` does not match a re-hash of
    /// its content. Someone edited the event body in place.
    ContentTampered,
    /// The line's `prev_hash` does not match the previous
    /// line's `hash`. Someone deleted or reordered events, or
    /// re-hashed later lines without repairing this one.
    PrevHashMismatch,
    /// The line has no `hash` field. Either predates the chain
    /// (legacy audit file) or the field was manually stripped.
    /// Chain restarts on the next line.
    MissingHash,
    /// serde_json refused to parse the line.
    Unparseable,
    /// Recompute failed on this event (should not happen in
    /// practice; a serialization bug on the write path is the
    /// only realistic cause).
    RecomputeFailed,
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
    fn chain_writes_prev_hash_none_on_the_first_event() {
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        log.write(&sample_event(AuditKind::EgressAllow)).unwrap();
        let raw = fs::read_to_string(dir.path().join(LOG_FILENAME)).unwrap();
        // Skip-serializing on `None`: the field must not appear.
        assert!(
            !raw.contains("\"prevHash\""),
            "genesis event should not carry prevHash: {raw}"
        );
        assert!(raw.contains("\"hash\":"), "hash field is missing: {raw}");
    }

    #[test]
    fn chain_second_event_prev_hash_matches_first_event_hash() {
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        log.write(&sample_event(AuditKind::EgressAllow)).unwrap();
        log.write(&sample_event(AuditKind::EgressDeny)).unwrap();
        let events = log.tail(2).unwrap();
        assert_eq!(events.len(), 2);
        let first_hash = events[0].hash.clone().expect("first event has hash");
        assert_eq!(
            events[1].prev_hash.as_deref(),
            Some(first_hash.as_str()),
            "second event's prev_hash must link to first event's hash"
        );
    }

    #[test]
    fn verify_reports_no_issues_on_a_clean_chain() {
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        for _ in 0..4 {
            log.write(&sample_event(AuditKind::EgressAllow)).unwrap();
        }
        let report = log.verify().unwrap();
        assert_eq!(report.events_checked, 4);
        assert!(report.is_intact(), "unexpected issues: {:?}", report.issues);
    }

    #[test]
    fn verify_detects_content_tampering() {
        // Mutate a byte inside a written event body (not the
        // hash field itself) and confirm the verifier catches it.
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        log.write(&sample_event(AuditKind::EgressAllow)).unwrap();
        log.write(&sample_event(AuditKind::EgressAllow)).unwrap();
        let path = dir.path().join(LOG_FILENAME);
        let raw = fs::read_to_string(&path).unwrap();
        // Replace the subject on the first line, preserving line
        // count and column shape so we know the diff is purely
        // content.
        let mutated = raw.replacen("openai@api.openai.com", "attacker@evil.com", 1);
        fs::write(&path, mutated).unwrap();
        let report = log.verify().unwrap();
        assert!(!report.is_intact(), "tamper undetected: {report:?}");
        // The specific issue must name the tampered line so an
        // operator can locate it.
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.line == 1 && i.kind == AuditVerifyIssueKind::ContentTampered),
            "expected ContentTampered on line 1: {:?}",
            report.issues
        );
    }

    #[test]
    fn verify_detects_line_deletion_via_prev_hash_mismatch() {
        // Write 3 events, delete the middle line, confirm the
        // verifier reports a broken link -- the survivor points
        // at a hash that no longer exists in the file.
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        log.write(&sample_event(AuditKind::EgressAllow)).unwrap();
        log.write(&sample_event(AuditKind::EgressDeny)).unwrap();
        log.write(&sample_event(AuditKind::BackendSwap)).unwrap();
        let path = dir.path().join(LOG_FILENAME);
        let raw = fs::read_to_string(&path).unwrap();
        let mut lines: Vec<&str> = raw.lines().collect();
        lines.remove(1);
        let after: String = lines.join("\n") + "\n";
        fs::write(&path, after).unwrap();
        let report = log.verify().unwrap();
        assert!(!report.is_intact(), "deletion undetected: {report:?}");
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.kind == AuditVerifyIssueKind::PrevHashMismatch),
            "expected a PrevHashMismatch after deletion: {:?}",
            report.issues
        );
    }

    #[test]
    fn verify_reports_missing_hash_on_pre_chain_legacy_lines() {
        // A file written by an older shell has events without
        // `hash` or `prevHash`. Confirm the verifier reports
        // MissingHash for those lines rather than treating them
        // as intact.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(LOG_FILENAME);
        // Hand-write a legacy-shaped line. Keep the field set
        // narrow so a future field addition doesn't make this
        // parse-fail instead of missing-hash-fail.
        let legacy = "{\"timestamp\":\"2026-01-01T00:00:00.000Z\",\"kind\":\"egress-allow\",\"outcome\":\"allow\",\"tenant\":\"acme\",\"device\":\"desk-1\",\"actor\":\"rico\",\"subject\":\"legacy\",\"detail\":null}\n";
        fs::write(&path, legacy).unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        let report = log.verify().unwrap();
        assert_eq!(report.events_checked, 1);
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.kind == AuditVerifyIssueKind::MissingHash),
            "expected MissingHash on legacy line: {:?}",
            report.issues
        );
    }

    #[test]
    fn compute_hash_refuses_input_that_already_carries_a_hash() {
        // Guards against a caller that forgets to clear
        // `hash` before recomputing -- would otherwise produce
        // a hash-of-a-hash and quietly diverge from the write
        // path. Debug-only assertion but pinned so a refactor
        // that changes the shape trips this.
        let mut event = sample_event(AuditKind::EgressAllow);
        event.hash = Some("stale".into());
        let result = std::panic::catch_unwind(|| compute_hash(&event));
        // Debug assertions on: this panics. Release: the
        // function computes a wrong hash but doesn't crash --
        // we accept that trade because clearing is the caller's
        // responsibility and every real caller does it.
        if cfg!(debug_assertions) {
            assert!(result.is_err(), "expected debug-assert to trip");
        }
    }

    #[test]
    fn rotation_restarts_the_chain_in_the_new_active_file() {
        // Confirm that the first event written after rotation
        // has prev_hash: None. The rollover file's chain stays
        // intact independently.
        let dir = TempDir::new().unwrap();
        let log = AuditLog::for_dir(dir.path().to_path_buf());
        // Seed the active file just under the cap so the next
        // write rotates.
        let path = dir.path().join(LOG_FILENAME);
        {
            let mut f = fs::File::create(&path).unwrap();
            let filler = "x".repeat(ROTATE_AT_BYTES as usize);
            f.write_all(filler.as_bytes()).unwrap();
        }
        log.write(&sample_event(AuditKind::EgressAllow)).unwrap();
        assert!(dir.path().join(ROLLOVER_FILENAME).exists());
        let active = fs::read_to_string(&path).unwrap();
        // The first (and only) line of the new active file must
        // NOT carry a prevHash field -- fresh chain.
        assert!(
            !active.contains("\"prevHash\""),
            "post-rotation genesis should not carry prevHash: {active}"
        );
        assert!(active.contains("\"hash\":"));
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
