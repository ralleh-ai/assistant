//! In-process approval requests for tool calls that policy gated with
//! `RequireApproval`.
//!
//! This is the minimal spine called for by `docs/NEXT_STEPS.md`: when
//! `ToolGateway::dispatch` stops on `ApprovalRequired`, it parks the
//! original invocation here so a later `approve` / `reject` can either
//! resume execution (without re-hitting the RequireApproval rule) or
//! permanently deny it. Persistence is in-memory — good enough to prove
//! the workflow; a durable store (Postgres / Temporal) is a Phase 2/4
//! concern per DEVELOPMENT.md.

use std::collections::HashMap;
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
///
/// Mirrors DEVELOPMENT.md §13's `ApprovalRequest` entity at the fields
/// this in-process spine actually needs. Extra control-plane fields
/// (workflow linkage, UI deep-links, etc.) can layer on later without
/// changing the gateway's approve/reject contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Thread-safe, process-local store of pending/resolved approvals.
#[derive(Debug, Default)]
pub struct ApprovalStore {
    inner: Mutex<HashMap<Uuid, ApprovalRequest>>,
}

impl ApprovalStore {
    pub fn new() -> Self {
        Self::default()
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
        self.inner
            .lock()
            .expect("approval store mutex poisoned")
            .insert(request.id, request.clone());
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
    /// invoke the handler. Fails if missing, wrong tenant, or not pending.
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
        Ok(snapshot)
    }

    /// Mark an already-Approved request as Executed after the handler ran.
    pub fn mark_executed(&self, id: Uuid) -> Result<(), ApprovalError> {
        let mut map = self.inner.lock().expect("approval store mutex poisoned");
        let entry = map.get_mut(&id).ok_or(ApprovalError::NotFound(id))?;
        if entry.status != ApprovalStatus::Approved {
            return Err(ApprovalError::NotPending(id, entry.status));
        }
        entry.status = ApprovalStatus::Executed;
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
}
