use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The final outcome of evaluating a request against the rule set.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PolicyOutcome {
    Allowed,
    Denied,
    ApprovalRequired,
}

/// An immutable, auditable record of a policy evaluation. Callers persist
/// this to their audit sink; this crate never performs I/O so evaluation
/// stays fast, pure, and trivial to unit test.
///
/// This directly satisfies DEVELOPMENT.md non-negotiable: "Do not add
/// integrations without audit events" and INVARIANT-A-style requirements —
/// every decision carries enough context to answer "who did what, when, and
/// why was it allowed/denied" without needing to reconstruct state later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub decision_id: Uuid,
    pub tenant_id: String,
    pub device_id: String,
    pub actor_id: String,
    pub capability: String,
    pub sensitivity: String,
    pub outcome: PolicyOutcome,
    /// The id of the rule that produced this outcome, or `None` if no rule
    /// matched (deny-by-default path).
    pub matched_rule_id: Option<String>,
    /// Human-readable reason, copied from the matched rule, or a fixed
    /// "no matching rule; deny by default" message.
    pub reason: String,
    pub evaluated_at: chrono::DateTime<chrono::Utc>,
}
