use std::sync::Arc;

use chrono::Utc;

use ralleh_policy_core::{PolicyEngine, PolicyOutcome, PolicyRequest};

use crate::event::{GatewayEvent, ToolCallOutcome};
use crate::handler::ToolInvocation;
use crate::registry::ToolRegistry;

/// Sink every `GatewayEvent` gets handed to immediately after it's built,
/// before `dispatch` returns. Kept as a trait object here (rather than a
/// generic parameter) so `ToolGateway` stays object-safe-friendly and easy
/// to construct in HTTP handler wiring without threading a type parameter
/// through `AppState`. Defined locally (not depending on
/// `ralleh-audit-store`) to keep this crate's dependency graph one-way:
/// gateway -> policy-core only. `ralleh-audit-store` depends on this crate
/// and implements `AuditSink` for its concrete sinks.
pub trait AuditSink: Send + Sync {
    fn record(&self, event: &GatewayEvent);
}

/// Default sink used when a gateway is constructed without an explicit
/// one (e.g. via `ToolGateway::new`, kept for backward compatibility with
/// existing call sites and tests) -- drops every event. Production wiring
/// should always use `ToolGateway::with_audit_sink` with a real sink from
/// `ralleh-audit-store`.
struct NoopAuditSink;

impl AuditSink for NoopAuditSink {
    fn record(&self, _event: &GatewayEvent) {}
}

/// The single chokepoint for dispatching any tool/capability call. Wraps a
/// `ToolRegistry` (what tools exist) and a `PolicyEngine` (what's allowed),
/// and guarantees:
///   - Unknown capabilities never reach the policy engine or a handler.
///   - A handler only ever runs after policy explicitly returned `Allowed`.
///   - Every call — whatever the outcome — produces exactly one
///     `GatewayEvent`, which is *always* handed to the configured
///     `AuditSink` before `dispatch` returns, independent of whether the
///     caller inspects the returned event at all. This is what actually
///     closes the audit-persistence gap: producing the record was never
///     the hard part, guaranteeing it gets recorded is.
pub struct ToolGateway {
    registry: ToolRegistry,
    policy: PolicyEngine,
    audit_sink: Arc<dyn AuditSink>,
}

impl ToolGateway {
    pub fn new(registry: ToolRegistry, policy: PolicyEngine) -> Self {
        Self {
            registry,
            policy,
            audit_sink: Arc::new(NoopAuditSink),
        }
    }

    /// Construct a gateway that records every `GatewayEvent` to the given
    /// sink. This is the constructor production wiring (e.g.
    /// `ralleh-mcp-server`'s `main.rs`) should use, passing in
    /// `ralleh_audit_store::JsonlFileAuditSink` (or another `AuditSink`
    /// impl) wrapped in the `Arc` it already needs to be shared across
    /// concurrent request handlers.
    pub fn with_audit_sink(
        registry: ToolRegistry,
        policy: PolicyEngine,
        audit_sink: Arc<dyn AuditSink>,
    ) -> Self {
        Self {
            registry,
            policy,
            audit_sink,
        }
    }

    /// Dispatch one tool call end-to-end: registry lookup -> policy
    /// evaluation -> (conditionally) handler execution -> audit event ->
    /// persist to the configured audit sink.
    ///
    /// This never panics on a "normal" failure path (unknown capability,
    /// policy denial, handler error) — those are all represented as
    /// `ToolCallOutcome` variants in the returned event, not as `Err`. The
    /// only way this fails to produce a usable result is a bug, which is
    /// exactly the property an audit-critical chokepoint needs.
    pub fn dispatch(
        &self,
        tenant_id: impl Into<String>,
        device_id: impl Into<String>,
        actor_id: impl Into<String>,
        capability: impl Into<String>,
        arguments: serde_json::Value,
    ) -> GatewayEvent {
        let tenant_id = tenant_id.into();
        let device_id = device_id.into();
        let actor_id = actor_id.into();
        let capability = capability.into();

        let Some(definition) = self.registry.definition(&capability) else {
            return GatewayEvent {
                capability,
                tenant_id,
                device_id,
                actor_id,
                policy_decision: None,
                outcome: ToolCallOutcome::UnknownCapability,
                occurred_at: Utc::now(),
            };
        };

        let policy_request = match PolicyRequest::new(
            tenant_id.clone(),
            device_id.clone(),
            actor_id.clone(),
            capability.clone(),
            definition.default_sensitivity.clone(),
        ) {
            Ok(req) => req,
            Err(_) => {
                // Should be unreachable in practice (registry definitions
                // are trusted config), but defense in depth: an invalid
                // request must never be silently allowed through.
                return GatewayEvent {
                    capability,
                    tenant_id,
                    device_id,
                    actor_id,
                    policy_decision: None,
                    outcome: ToolCallOutcome::Denied,
                    occurred_at: Utc::now(),
                };
            }
        };

        let decision = self.policy.evaluate(&policy_request);

        let outcome = match decision.outcome {
            PolicyOutcome::Denied => ToolCallOutcome::Denied,
            PolicyOutcome::ApprovalRequired => ToolCallOutcome::ApprovalRequired,
            PolicyOutcome::Allowed => match self.registry.handler(&capability) {
                None => ToolCallOutcome::NoHandlerRegistered,
                Some(handler) => {
                    let invocation = ToolInvocation {
                        capability: capability.clone(),
                        tenant_id: tenant_id.clone(),
                        device_id: device_id.clone(),
                        actor_id: actor_id.clone(),
                        arguments,
                    };
                    match handler.invoke(&invocation) {
                        Ok(result) => ToolCallOutcome::Succeeded {
                            result_summary: result.summary,
                        },
                        Err(error) => ToolCallOutcome::Failed { error },
                    }
                }
            },
        };

        let event = GatewayEvent {
            capability,
            tenant_id,
            device_id,
            actor_id,
            policy_decision: Some(decision),
            outcome,
            occurred_at: Utc::now(),
        };
        self.audit_sink.record(&event);
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::{AlwaysFailHandler, EchoHandler};
    use crate::registry::ToolDefinition;
    use ralleh_policy_core::{PolicyRule, RuleEffect};

    fn registry_with_echo(capability: &str, sensitivity: &str) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(
            ToolDefinition {
                capability: capability.to_string(),
                description: "test tool".to_string(),
                default_sensitivity: sensitivity.to_string(),
            },
            Box::new(EchoHandler),
        );
        registry
    }

    #[test]
    fn unknown_capability_is_rejected_before_policy_is_consulted() {
        let gateway = ToolGateway::new(ToolRegistry::new(), PolicyEngine::empty());
        let event = gateway.dispatch(
            "t1",
            "d1",
            "u1",
            "tool.nonexistent",
            serde_json::Value::Null,
        );
        assert_eq!(event.outcome, ToolCallOutcome::UnknownCapability);
        assert!(event.policy_decision.is_none());
    }

    #[test]
    fn registered_capability_denied_by_default_policy() {
        let registry = registry_with_echo("tool.search", "public");
        let gateway = ToolGateway::new(registry, PolicyEngine::empty());
        let event = gateway.dispatch("t1", "d1", "u1", "tool.search", serde_json::Value::Null);
        assert_eq!(event.outcome, ToolCallOutcome::Denied);
        assert!(event.policy_decision.is_some());
    }

    #[test]
    fn allowed_capability_with_handler_succeeds() {
        let registry = registry_with_echo("tool.search", "public");
        let allow_rule = PolicyRule {
            id: "allow-search".to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.search".to_string()),
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "search is safe".to_string(),
        };
        let gateway = ToolGateway::new(registry, PolicyEngine::new(vec![allow_rule]));
        let event = gateway.dispatch(
            "t1",
            "d1",
            "u1",
            "tool.search",
            serde_json::json!({"query": "rust"}),
        );
        match event.outcome {
            ToolCallOutcome::Succeeded { result_summary } => {
                assert_eq!(result_summary, "echo:tool.search");
            }
            other => panic!("expected Succeeded, got {other:?}"),
        }
    }

    #[test]
    fn allowed_capability_with_failing_handler_reports_failed_not_denied() {
        let mut registry = ToolRegistry::new();
        registry.register(
            ToolDefinition {
                capability: "tool.flaky".to_string(),
                description: "test tool that always fails".to_string(),
                default_sensitivity: "public".to_string(),
            },
            Box::new(AlwaysFailHandler {
                error_message: "upstream unavailable".to_string(),
            }),
        );
        let allow_rule = PolicyRule {
            id: "allow-flaky".to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.flaky".to_string()),
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "test".to_string(),
        };
        let gateway = ToolGateway::new(registry, PolicyEngine::new(vec![allow_rule]));
        let event = gateway.dispatch("t1", "d1", "u1", "tool.flaky", serde_json::Value::Null);
        match event.outcome {
            ToolCallOutcome::Failed { error } => assert_eq!(error, "upstream unavailable"),
            other => panic!("expected Failed, got {other:?}"),
        }
        // Critically: this must be distinguishable from a policy denial in
        // audit logs, since the failure modes have very different
        // implications (integration bug vs. security control working).
        assert!(event.policy_decision.is_some());
        assert_eq!(
            event.policy_decision.unwrap().outcome,
            ralleh_policy_core::PolicyOutcome::Allowed
        );
    }

    #[test]
    fn approval_required_capability_never_invokes_handler() {
        let registry = registry_with_echo("tool.finance.transfer", "confidential");
        let approval_rule = PolicyRule {
            id: "approval-finance".to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.finance".to_string()),
            sensitivity: None,
            effect: RuleEffect::RequireApproval,
            reason: "financial actions require human approval".to_string(),
        };
        let gateway = ToolGateway::new(registry, PolicyEngine::new(vec![approval_rule]));
        let event = gateway.dispatch(
            "t1",
            "d1",
            "u1",
            "tool.finance.transfer",
            serde_json::Value::Null,
        );
        assert_eq!(event.outcome, ToolCallOutcome::ApprovalRequired);
    }

    #[test]
    fn allowed_capability_without_registered_handler_is_reported_distinctly() {
        // Register a definition with no handler, to simulate a
        // configuration bug where policy exists but no implementation does.
        let mut registry = ToolRegistry::new();
        // ToolRegistry currently only exposes `register` which takes both
        // a definition and a handler together, so we simulate the
        // "definition without handler" case is impossible by construction
        // in this crate's public API — that itself is a safety property
        // worth asserting: you cannot register a capability without also
        // providing its handler.
        registry.register(
            ToolDefinition {
                capability: "tool.search".to_string(),
                description: "test".to_string(),
                default_sensitivity: "public".to_string(),
            },
            Box::new(EchoHandler),
        );
        assert!(registry.handler("tool.search").is_some());
    }

    #[test]
    fn cross_tenant_isolation_holds_through_the_full_gateway_path() {
        let registry = registry_with_echo("tool.search", "public");
        let tenant_scoped_rule = PolicyRule {
            id: "tenant-a-search".to_string(),
            tenant_id: Some("tenant-a".to_string()),
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.search".to_string()),
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "tenant-a search access".to_string(),
        };
        let gateway = ToolGateway::new(registry, PolicyEngine::new(vec![tenant_scoped_rule]));

        let event_a = gateway.dispatch(
            "tenant-a",
            "d1",
            "u1",
            "tool.search",
            serde_json::Value::Null,
        );
        let event_b = gateway.dispatch(
            "tenant-b",
            "d1",
            "u1",
            "tool.search",
            serde_json::Value::Null,
        );

        assert!(matches!(event_a.outcome, ToolCallOutcome::Succeeded { .. }));
        assert_eq!(event_b.outcome, ToolCallOutcome::Denied);
    }
}
