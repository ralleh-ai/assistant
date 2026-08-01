use serde::{Deserialize, Serialize};

use ralleh_policy_core::PolicyDecision;

/// Outcome of a tool call, independent of the policy decision that gated it.
/// A call can be `Allowed` by policy and still fail at execution time (e.g.
/// the underlying integration errored) — that's a separate axis from
/// whether it was authorized to run at all, and both are recorded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ToolCallOutcome {
    /// Policy denied the call; the handler was never invoked.
    Denied,
    /// Policy requires human approval; the handler was never invoked.
    /// The matching pending request id (if any) is on `GatewayEvent::
    /// approval_request_id`.
    ApprovalRequired,
    /// A previously pending approval was explicitly rejected; the handler
    /// was never invoked.
    ApprovalRejected,
    /// Policy allowed the call and the handler executed successfully.
    Succeeded { result_summary: String },
    /// Policy allowed the call but the handler itself returned an error.
    Failed { error: String },
    /// Policy allowed the call, but no handler is registered for this
    /// capability — a configuration bug, not a policy or handler failure.
    NoHandlerRegistered,
    /// The capability was not found in the tool registry at all — rejected
    /// before policy was even consulted.
    UnknownCapability,
}

/// A single, immutable, audit-ready record of one gateway dispatch. Every
/// call through `ToolGateway::dispatch` produces exactly one of these,
/// regardless of outcome. This is the artifact that satisfies "audit events
/// for every privileged action" from DEVELOPMENT.md's non-negotiables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayEvent {
    pub capability: String,
    pub tenant_id: String,
    pub device_id: String,
    pub actor_id: String,
    /// `None` when the capability was unknown and policy was never consulted.
    pub policy_decision: Option<PolicyDecision>,
    pub outcome: ToolCallOutcome,
    /// Set when this event created or resolved an `ApprovalRequest`
    /// (parked on `ApprovalRequired`, or carried through on a later
    /// approve/reject/execute). `#[serde(default)]` keeps older JSONL
    /// audit lines that predate this field deserializable.
    #[serde(default)]
    pub approval_request_id: Option<uuid::Uuid>,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}
