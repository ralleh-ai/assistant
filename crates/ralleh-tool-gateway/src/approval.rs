//! Approval requests for tool calls gated with `RequireApproval`.
//!
//! When `ToolGateway::dispatch` stops on `ApprovalRequired`, it parks the
//! original invocation here so a later `approve` / `reject` can resume or
//! deny it. The store is optionally durable: `ApprovalStore::open(path)`
//! snapshots the full map to a JSON file after every mutation so pending
//! approvals survive process restarts (Postgres/Temporal remain Phase 2/4).

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Lifecycle of one approval request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    /// Approved and the underlying handler has already been invoked
    /// (success or failure — the approval itself was consumed either way).
    Executed,
}

/// A parked tool invocation waiting on human confirmation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovalRequest {
    pub id: Uuid,
    pub tenant_id: String,
    pub device_id: String,
    /// Original requester (the actor whose call was gated).
    pub actor_id: String,
    pub capability: String,
    pub arguments: serde_json::Value,
    pub policy_decision_id: Uuid,
    pub reason: String,
    pub status: ApprovalStatus,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    /// Actor who approved or rejected. `None` while still pending.
    pub decided_by: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApprovalError {
    #[error("no approval request found for id {0}")]
    NotFound(Uuid),
    #[error("approval request {0} is not pending (status={1:?})")]
    NotPending(Uuid, ApprovalStatus),
    #[error("approval request belongs to a different tenant")]
    TenantMismatch,
}

/// Thread-safe store of pending/resolved approvals.
///
/// Construct with [`ApprovalStore::new`] (memory-only) or
/// [`ApprovalStore::open`] (load + persist a JSON snapshot on every change).
#[derive(Debug)]
pub struct ApprovalStore {
    inner: Mutex<HashMap<Uuid, ApprovalRequest>>,
    /// When set, every mutating operation atomically rewrites this path.
    path: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct ApprovalSnapshot {
    requests: Vec<ApprovalRequest>,
}

impl Default for ApprovalStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ApprovalStore {
    /// In-memory only — nothing survives process exit.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            path: None,
        }
    }

    /// Open (or create) a durable store at `path`. Existing file contents
    /// are loaded; missing file starts empty. Subsequent mutations rewrite
    /// the file atomically (`*.tmp` + rename).
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let map = if path.is_file() {
            let raw = fs::read_to_string(&path)?;
            if raw.trim().is_empty() {
                HashMap::new()
            } else {
                let snap: ApprovalSnapshot = serde_json::from_str(&raw).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("corrupt approval store {}: {e}", path.display()),
                    )
                })?;
                snap.requests.into_iter().map(|r| (r.id, r)).collect()
            }
        } else {
            HashMap::new()
        };

        let store = Self {
            inner: Mutex::new(map),
            path: Some(path.clone()),
        };
        // Ensure the file exists even when empty so operators can see it.
        store.persist()?;
        Ok(store)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("approval store mutex poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn persist(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let map = self.inner.lock().expect("approval store mutex poisoned");
        self.persist_map(path, &map)
    }

    fn persist_map(&self, path: &Path, map: &HashMap<Uuid, ApprovalRequest>) -> io::Result<()> {
        let snap = ApprovalSnapshot {
            requests: map.values().cloned().collect(),
        };
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            serde_json::to_writer_pretty(&mut f, &snap)?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    fn persist_or_expect(&self) {
        if let Err(e) = self.persist() {
            panic!(
                "failed to persist approval store to {}: {e}",
                self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
            );
        }
    }

    /// Park a new pending approval for a gated invocation.
    pub fn create_pending(
        &self,
        tenant_id: impl Into<String>,
        device_id: impl Into<String>,
        actor_id: impl Into<String>,
        capability: impl Into<String>,
        arguments: serde_json::Value,
        policy_decision_id: Uuid,
        reason: impl Into<String>,
    ) -> ApprovalRequest {
        let request = ApprovalRequest {
            id: Uuid::new_v4(),
            tenant_id: tenant_id.into(),
            device_id: device_id.into(),
            actor_id: actor_id.into(),
            capability: capability.into(),
            arguments,
            policy_decision_id,
            reason: reason.into(),
            status: ApprovalStatus::Pending,
            created_at: Utc::now(),
            decided_at: None,
            decided_by: None,
        };
        {
            let mut map = self.inner.lock().expect("approval store mutex poisoned");
            map.insert(request.id, request.clone());
        }
        self.persist_or_expect();
        request
    }

    pub fn get(&self, id: Uuid) -> Option<ApprovalRequest> {
        self.inner
            .lock()
            .expect("approval store mutex poisoned")
            .get(&id)
            .cloned()
    }

    /// Atomically claim a pending approval for the given tenant, marking it
    /// Approved (or Rejected). Returns the request as it stood *before* the
    /// status flip so the caller can still read the original arguments and
    /// invoke the handler.
    pub fn claim(
        &self,
        id: Uuid,
        tenant_id: &str,
        decided_by: impl Into<String>,
        next: ApprovalStatus,
    ) -> Result<ApprovalRequest, ApprovalError> {
        assert!(
            matches!(next, ApprovalStatus::Approved | ApprovalStatus::Rejected),
            "claim() only accepts Approved or Rejected as the next status"
        );
        let snapshot = {
            let mut map = self.inner.lock().expect("approval store mutex poisoned");
            let entry = map.get_mut(&id).ok_or(ApprovalError::NotFound(id))?;
            if entry.tenant_id != tenant_id {
                return Err(ApprovalError::TenantMismatch);
            }
            if entry.status != ApprovalStatus::Pending {
                return Err(ApprovalError::NotPending(id, entry.status));
            }
            let snapshot = entry.clone();
            entry.status = next;
            entry.decided_at = Some(Utc::now());
            entry.decided_by = Some(decided_by.into());
            snapshot
        };
        self.persist_or_expect();
        Ok(snapshot)
    }

    /// Mark an already-Approved request as Executed after the handler ran.
    pub fn mark_executed(&self, id: Uuid) -> Result<(), ApprovalError> {
        {
            let mut map = self.inner.lock().expect("approval store mutex poisoned");
            let entry = map.get_mut(&id).ok_or(ApprovalError::NotFound(id))?;
            if entry.status != ApprovalStatus::Approved {
                return Err(ApprovalError::NotPending(id, entry.status));
            }
            entry.status = ApprovalStatus::Executed;
        }
        self.persist_or_expect();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(store: &ApprovalStore) -> ApprovalRequest {
        store.create_pending(
            "tenant-a",
            "device-1",
            "user-1",
            "tool.fs.write_text",
            serde_json::json!({"path": "a.txt", "contents": "hi"}),
            Uuid::new_v4(),
            "writes require approval",
        )
    }

    #[test]
    fn create_pending_is_retrievable() {
        let store = ApprovalStore::new();
        let created = seed(&store);
        let got = store.get(created.id).unwrap();
        assert_eq!(got.status, ApprovalStatus::Pending);
        assert_eq!(got.capability, "tool.fs.write_text");
    }

    #[test]
    fn claim_approve_then_second_claim_fails() {
        let store = ApprovalStore::new();
        let created = seed(&store);
        let first = store
            .claim(
                created.id,
                "tenant-a",
                "approver-1",
                ApprovalStatus::Approved,
            )
            .unwrap();
        assert_eq!(first.status, ApprovalStatus::Pending);
        assert_eq!(
            store.get(created.id).unwrap().status,
            ApprovalStatus::Approved
        );
        let err = store
            .claim(
                created.id,
                "tenant-a",
                "approver-2",
                ApprovalStatus::Approved,
            )
            .unwrap_err();
        assert!(matches!(err, ApprovalError::NotPending(_, ApprovalStatus::Approved)));
    }

    #[test]
    fn claim_rejects_cross_tenant() {
        let store = ApprovalStore::new();
        let created = seed(&store);
        let err = store
            .claim(
                created.id,
                "tenant-b",
                "approver-1",
                ApprovalStatus::Approved,
            )
            .unwrap_err();
        assert_eq!(err, ApprovalError::TenantMismatch);
        assert_eq!(
            store.get(created.id).unwrap().status,
            ApprovalStatus::Pending
        );
    }

    #[test]
    fn durable_store_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("approvals.json");

        let id = {
            let store = ApprovalStore::open(&path).unwrap();
            let created = seed(&store);
            assert!(path.is_file());
            created.id
        };

        let reopened = ApprovalStore::open(&path).unwrap();
        let got = reopened.get(id).unwrap();
        assert_eq!(got.status, ApprovalStatus::Pending);
        assert_eq!(got.capability, "tool.fs.write_text");
        assert_eq!(got.tenant_id, "tenant-a");
    }

    #[test]
    fn durable_store_persists_status_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("approvals.json");
        let store = ApprovalStore::open(&path).unwrap();
        let created = seed(&store);
        store
            .claim(
                created.id,
                "tenant-a",
                "approver-1",
                ApprovalStatus::Rejected,
            )
            .unwrap();

        let reopened = ApprovalStore::open(&path).unwrap();
        assert_eq!(
            reopened.get(created.id).unwrap().status,
            ApprovalStatus::Rejected
        );
        assert_eq!(
            reopened.get(created.id).unwrap().decided_by.as_deref(),
            Some("approver-1")
        );
    }
}
