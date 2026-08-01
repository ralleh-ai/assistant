use serde::{Deserialize, Serialize};

/// A request to route to some completion backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub tenant_id: String,
    pub device_id: String,
    pub actor_id: String,
    /// Optional caller hint about which backend/model family to prefer.
    /// The router treats this as advisory, not authoritative -- routing
    /// policy always has final say.
    pub model_hint: Option<String>,
    pub prompt: String,
}

/// A successful completion result from whichever backend served it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub backend: String,
    pub text: String,
}

/// Mirrors `ralleh_tool_gateway::ToolCallOutcome` in spirit: every routing
/// attempt produces exactly one outcome, so routing is auditable the same
/// way tool dispatch is, without callers having to inspect a raw
/// `Result<_, _>` to figure out what happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CompletionOutcome {
    Succeeded(CompletionResponse),
    Failed { backend: String, error: String },
    Denied,
    ApprovalRequired,
    NoBackendConfigured,
}
