use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::record::AuditRecord;

use ralleh_tool_gateway::GatewayEvent;
use ralleh_tool_gateway::gateway::AuditSink as GatewayAuditSink;

/// Errors a sink can produce while persisting an audit record. Kept
/// separate from the record types themselves — persistence failures are an
/// operational concern, not part of the audit data model.
#[derive(Debug, thiserror::Error)]
pub enum AuditSinkError {
    #[error("failed to open audit log at {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write audit record: {source}")]
    Write {
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialize audit record: {source}")]
    Serialize {
        #[source]
        source: serde_json::Error,
    },
}

/// The persistence seam every caller depends on. Never depend on a
/// concrete sink type directly (mirrors `ToolHandler` / `CompletionBackend`
/// elsewhere in the workspace) — this is what lets the storage backend
/// change later (e.g. to a real database) without touching the gateway,
/// router, or HTTP layer that produce these records.
///
/// `record` takes `&self` (not `&mut self`) so sinks can be shared behind
/// an `Arc` across concurrent request handlers the same way `ToolGateway`
/// and `AiRouter` already are; implementations are responsible for their
/// own internal synchronization.
pub trait AuditSink: Send + Sync {
    fn record(&self, record: &AuditRecord) -> Result<(), AuditSinkError>;
}

/// Discards every record. Useful for tests that construct a full
/// gateway/router stack but don't care about audit persistence, so they
/// don't need to manage a temp file just to satisfy the type.
pub struct NullAuditSink;

impl AuditSink for NullAuditSink {
    fn record(&self, _record: &AuditRecord) -> Result<(), AuditSinkError> {
        Ok(())
    }
}

/// Keeps every recorded record in memory, in the order received. Useful
/// for tests that want to assert on exactly what was persisted (record
/// count, specific outcomes, ordering) without touching the filesystem.
#[derive(Default)]
pub struct InMemoryAuditSink {
    records: Mutex<Vec<AuditRecord>>,
}

impl InMemoryAuditSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of everything recorded so far, in insertion order.
    pub fn records(&self) -> Vec<AuditRecord> {
        self.records
            .lock()
            .expect("audit sink mutex poisoned")
            .clone()
    }

    pub fn len(&self) -> usize {
        self.records.lock().expect("audit sink mutex poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl AuditSink for InMemoryAuditSink {
    fn record(&self, record: &AuditRecord) -> Result<(), AuditSinkError> {
        self.records
            .lock()
            .expect("audit sink mutex poisoned")
            .push(record.clone());
        Ok(())
    }
}

// `AuditRecord` needs `Clone` for `InMemoryAuditSink`; derive lives on the
// type itself in record.rs, so nothing extra needed here beyond the trait
// bound already present.

/// Append-only, one-JSON-object-per-line file sink. This is the real
/// (non-mocked) persistence path: every call to `record` opens the file in
/// append mode, writes one JSON line, and flushes before returning —
/// trading a bit of per-call syscall overhead for the property that a
/// crash or kill -9 immediately after a successful `record()` call cannot
/// lose that record. A `Mutex` serializes writes so concurrent callers
/// (e.g. multiple Axum request handlers) can't interleave partial lines.
pub struct JsonlFileAuditSink {
    path: PathBuf,
    file: Mutex<File>,
}

impl JsonlFileAuditSink {
    /// Opens (creating if necessary) the audit log at `path` for
    /// appending. Parent directories are not created automatically —
    /// callers own their deployment layout, mirroring how
    /// `FsReadTextHandler::new` requires its sandbox root to already
    /// exist.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuditSinkError> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| AuditSinkError::Open {
                path: path.clone(),
                source,
            })?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    /// The path this sink writes to, mostly useful for logging/diagnostics
    /// at startup.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AuditSink for JsonlFileAuditSink {
    fn record(&self, record: &AuditRecord) -> Result<(), AuditSinkError> {
        let mut line =
            serde_json::to_vec(record).map_err(|source| AuditSinkError::Serialize { source })?;
        line.push(b'\n');

        let mut file = self.file.lock().expect("audit sink mutex poisoned");
        file.write_all(&line)
            .and_then(|_| file.flush())
            .map_err(|source| AuditSinkError::Write { source })
    }
}

impl GatewayAuditSink for JsonlFileAuditSink {
    fn record(&self, event: &GatewayEvent) {
        let record = AuditRecord::tool_dispatch(event.clone());
        // Best-effort from the gateway's perspective: `AuditSink::record`
        // in `ralleh-tool-gateway` returns nothing to keep `dispatch`
        // infallible for callers, so a write failure here is logged rather
        // than propagated. Losing an audit write is a real operational
        // concern, but it must never take down tool dispatch itself --
        // that would turn an audit sink outage into a full service outage,
        // which is a worse failure mode.
        if let Err(err) = AuditSink::record(self, &record) {
            eprintln!("ralleh-audit-store: failed to persist gateway event: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralleh_ai_router::CompletionOutcome;
    use ralleh_tool_gateway::{GatewayEvent, ToolCallOutcome};

    fn sample_tool_dispatch_record() -> AuditRecord {
        AuditRecord::tool_dispatch(GatewayEvent {
            capability: "tool.search".to_string(),
            tenant_id: "t1".to_string(),
            device_id: "d1".to_string(),
            actor_id: "u1".to_string(),
            policy_decision: None,
            outcome: ToolCallOutcome::UnknownCapability,
            approval_request_id: None,
            occurred_at: chrono::Utc::now(),
        })
    }

    fn sample_completion_record() -> AuditRecord {
        AuditRecord::completion("t1", "d1", "u1", CompletionOutcome::NoBackendConfigured)
    }

    #[test]
    fn null_sink_always_succeeds() {
        let sink = NullAuditSink;
        assert!(AuditSink::record(&sink, &sample_tool_dispatch_record()).is_ok());
    }

    #[test]
    fn in_memory_sink_preserves_insertion_order() {
        let sink = InMemoryAuditSink::new();
        AuditSink::record(&sink, &sample_tool_dispatch_record()).unwrap();
        AuditSink::record(&sink, &sample_completion_record()).unwrap();

        let records = sink.records();
        assert_eq!(records.len(), 2);
        assert!(matches!(
            records[0].kind,
            crate::record::AuditRecordKind::ToolDispatch(_)
        ));
        assert!(matches!(
            records[1].kind,
            crate::record::AuditRecordKind::Completion { .. }
        ));
    }

    #[test]
    fn in_memory_sink_reports_len_and_emptiness() {
        let sink = InMemoryAuditSink::new();
        assert!(sink.is_empty());
        AuditSink::record(&sink, &sample_tool_dispatch_record()).unwrap();
        assert_eq!(sink.len(), 1);
        assert!(!sink.is_empty());
    }

    #[test]
    fn jsonl_sink_creates_file_and_appends_one_line_per_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let sink = JsonlFileAuditSink::open(&path).unwrap();

        AuditSink::record(&sink, &sample_tool_dispatch_record()).unwrap();
        AuditSink::record(&sink, &sample_completion_record()).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);

        // Each line must be independently valid JSON (that's the whole
        // point of JSONL) and round-trip back into an AuditRecord.
        let first: AuditRecord = serde_json::from_str(lines[0]).unwrap();
        let second: AuditRecord = serde_json::from_str(lines[1]).unwrap();
        assert!(matches!(
            first.kind,
            crate::record::AuditRecordKind::ToolDispatch(_)
        ));
        assert!(matches!(
            second.kind,
            crate::record::AuditRecordKind::Completion { .. }
        ));
    }

    #[test]
    fn jsonl_sink_appends_across_multiple_opens_of_the_same_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");

        {
            let sink = JsonlFileAuditSink::open(&path).unwrap();
            AuditSink::record(&sink, &sample_tool_dispatch_record()).unwrap();
        }
        {
            let sink = JsonlFileAuditSink::open(&path).unwrap();
            AuditSink::record(&sink, &sample_completion_record()).unwrap();
        }

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 2);
    }

    #[test]
    fn jsonl_sink_path_returns_the_configured_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let sink = JsonlFileAuditSink::open(&path).unwrap();
        assert_eq!(sink.path(), path.as_path());
    }

    #[test]
    fn concurrent_writes_do_not_interleave_or_lose_records() {
        use std::sync::Arc;
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let sink = Arc::new(JsonlFileAuditSink::open(&path).unwrap());

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let sink = Arc::clone(&sink);
                thread::spawn(move || {
                    for _ in 0..25 {
                        AuditSink::record(sink.as_ref(), &sample_tool_dispatch_record()).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 8 * 25);
        // Every single line must be independently parseable -- if writes
        // had interleaved, some lines would be malformed JSON.
        for line in lines {
            let _: AuditRecord = serde_json::from_str(line).unwrap();
        }
    }
}
