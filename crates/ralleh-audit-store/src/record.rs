use serde::{Deserialize, Serialize};

use ralleh_ai_router::CompletionOutcome;
use ralleh_tool_gateway::GatewayEvent;

/// The two kinds of audit-worthy events produced elsewhere in the
/// workspace. Kept as an enum (rather than two separate sink methods) so
/// a single append-only log — or a single DB table, if a real database
/// backs this later — can interleave both event kinds in true
/// chronological order, which matters for reconstructing "what happened,
/// in what order" during an incident review.
// Both variants are hot-path event carriers. Boxing `ToolDispatch`
// to trim the enum's stack size would add an allocation on every
// tool dispatch (thousands per session in the real product) to
// save a couple hundred bytes on a rare `Completion`. The
// asymmetry isn't worth the churn.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditRecordKind {
    /// A tool/capability dispatch, exactly as produced by
    /// `ralleh_tool_gateway::ToolGateway::dispatch`.
    ToolDispatch(GatewayEvent),
    /// An AI completion routing attempt. `ralleh_ai_router::AiRouter`
    /// doesn't currently produce a single struct the way the tool gateway
    /// does (it returns a bare `CompletionOutcome`), so the caller (the
    /// HTTP layer, today) supplies the request context alongside the
    /// outcome to keep the audit record self-contained the same way
    /// `GatewayEvent` is.
    Completion {
        tenant_id: String,
        device_id: String,
        actor_id: String,
        outcome: CompletionOutcome,
    },
}

/// A single, immutable audit record ready to persist. Every record gets a
/// stable id and timestamp *at persistence time*, independent of whatever
/// timestamp (if any) the underlying event already carries — this is the
/// "when it was recorded" time, which matters for sinks that may buffer or
/// retry writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub record_id: uuid::Uuid,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
    #[serde(flatten)]
    pub kind: AuditRecordKind,
}

impl AuditRecord {
    pub fn new(kind: AuditRecordKind) -> Self {
        Self {
            record_id: uuid::Uuid::new_v4(),
            recorded_at: chrono::Utc::now(),
            kind,
        }
    }

    pub fn tool_dispatch(event: GatewayEvent) -> Self {
        Self::new(AuditRecordKind::ToolDispatch(event))
    }

    pub fn completion(
        tenant_id: impl Into<String>,
        device_id: impl Into<String>,
        actor_id: impl Into<String>,
        outcome: CompletionOutcome,
    ) -> Self {
        Self::new(AuditRecordKind::Completion {
            tenant_id: tenant_id.into(),
            device_id: device_id.into(),
            actor_id: actor_id.into(),
            outcome,
        })
    }
}
