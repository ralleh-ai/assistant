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

/// One event on a streaming completion channel. `Chunk` events
/// arrive in order and their `text` fields concatenate to the full
/// response; exactly one terminal event (`Done`, `Failed`, `Denied`,
/// `ApprovalRequired`, `NoBackendConfigured`) is guaranteed to
/// follow the last `Chunk` — even on cancellation the router
/// closes the channel with a terminal event so the consumer never
/// sees a hang.
///
/// # Backwards compatibility with `CompletionOutcome`
///
/// The terminal variants are shaped so a consumer collecting the
/// full text and the terminal event can reconstruct the exact
/// `CompletionOutcome` a non-streaming `route(&request)` call
/// would have produced. `route` and `route_stream` are semantically
/// identical for correctness — they only differ in when the caller
/// gets to see the text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CompletionStreamEvent {
    /// One incremental piece of response text. Order matters:
    /// concatenating the `text` fields of every `Chunk` in the
    /// order they arrive reproduces the whole response.
    Chunk { backend: String, text: String },
    /// Terminal success. Emitted after the last `Chunk`.
    Done { backend: String },
    /// Terminal handler failure — the backend was invoked and
    /// something went wrong (network, upstream 5xx, parse error).
    Failed { backend: String, error: String },
    /// Terminal policy denial. No `Chunk` events precede this.
    Denied,
    /// Terminal approval-required. No `Chunk` events precede this.
    ApprovalRequired,
    /// Terminal misconfiguration. Router had no usable backend.
    NoBackendConfigured,
}
