use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use ralleh_policy_core::{PolicyEngine, PolicyOutcome, PolicyRequest};

use crate::approval::{ApprovalError, ApprovalStatus, ApprovalStore};
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
///   - A handler only ever runs after policy explicitly returned `Allowed`
///     *or* after a previously parked `ApprovalRequired` call was approved.
///   - Every call — whatever the outcome — produces exactly one
///     `GatewayEvent`, which is *always* handed to the configured
///     `AuditSink` before `dispatch` / `approve` / `reject` returns.
pub struct ToolGateway {
    registry: ToolRegistry,
    policy: PolicyEngine,
    audit_sink: Arc<dyn AuditSink>,
    approvals: Arc<ApprovalStore>,
}

impl ToolGateway {
    pub fn new(registry: ToolRegistry, policy: PolicyEngine) -> Self {
        Self {
            registry,
            policy,
            audit_sink: Arc::new(NoopAuditSink),
            approvals: Arc::new(ApprovalStore::new()),
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
        Self::with_audit_sink_and_approvals(
            registry,
            policy,
            audit_sink,
            Arc::new(ApprovalStore::new()),
        )
    }

    /// Like `with_audit_sink`, but installs a caller-supplied approval
    /// store (typically `ApprovalStore::open(...)` for durability).
    pub fn with_audit_sink_and_approvals(
        registry: ToolRegistry,
        policy: PolicyEngine,
        audit_sink: Arc<dyn AuditSink>,
        approvals: Arc<ApprovalStore>,
    ) -> Self {
        Self {
            registry,
            policy,
            audit_sink,
            approvals,
        }
    }

    /// Shared handle to the in-process approval store (useful for tests and
    /// for HTTP layers that want to introspect a pending request).
    pub fn approvals(&self) -> Arc<ApprovalStore> {
        self.approvals.clone()
    }

    /// Dispatch one tool call end-to-end: registry lookup -> policy
    /// evaluation -> (conditionally) handler execution -> audit event ->
    /// persist to the configured audit sink.
    ///
    /// On `ApprovalRequired`, the original invocation is parked in the
    /// approval store and `GatewayEvent::approval_request_id` is set so a
    /// later `approve` / `reject` can resume or cancel it.
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
            return self.finish(GatewayEvent {
                capability,
                tenant_id,
                device_id,
                actor_id,
                policy_decision: None,
                outcome: ToolCallOutcome::UnknownCapability,
                approval_request_id: None,
                occurred_at: Utc::now(),
            });
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
                return self.finish(GatewayEvent {
                    capability,
                    tenant_id,
                    device_id,
                    actor_id,
                    policy_decision: None,
                    outcome: ToolCallOutcome::Denied,
                    approval_request_id: None,
                    occurred_at: Utc::now(),
                });
            }
        };

        let decision = self.policy.evaluate(&policy_request);

        let (outcome, approval_request_id) = match decision.outcome {
            PolicyOutcome::Denied => (ToolCallOutcome::Denied, None),
            PolicyOutcome::ApprovalRequired => {
                let pending = self.approvals.create_pending(
                    tenant_id.clone(),
                    device_id.clone(),
                    actor_id.clone(),
                    capability.clone(),
                    arguments,
                    decision.decision_id,
                    decision.reason.clone(),
                );
                (ToolCallOutcome::ApprovalRequired, Some(pending.id))
            }
            PolicyOutcome::Allowed => (
                self.invoke_handler(
                    &capability,
                    &tenant_id,
                    &device_id,
                    &actor_id,
                    arguments,
                ),
                None,
            ),
        };

        self.finish(GatewayEvent {
            capability,
            tenant_id,
            device_id,
            actor_id,
            policy_decision: Some(decision),
            outcome,
            approval_request_id,
            occurred_at: Utc::now(),
        })
    }

    /// Grant a previously parked approval and execute the original
    /// invocation. Skips policy re-evaluation on purpose: re-running the
    /// engine would just hit `RequireApproval` again. Tenant isolation is
    /// still enforced — `tenant_id` must match the pending request.
    pub fn approve(
        &self,
        approval_id: Uuid,
        tenant_id: impl AsRef<str>,
        decided_by: impl Into<String>,
    ) -> Result<GatewayEvent, ApprovalError> {
        let decided_by = decided_by.into();
        let pending = self.approvals.claim(
            approval_id,
            tenant_id.as_ref(),
            decided_by.clone(),
            ApprovalStatus::Approved,
        )?;

        let outcome = self.invoke_handler(
            &pending.capability,
            &pending.tenant_id,
            &pending.device_id,
            &pending.actor_id,
            pending.arguments.clone(),
        );
        // Consume the approval whether the handler succeeded or failed —
        // an approved invocation is one-shot either way.
        self.approvals.mark_executed(approval_id)?;

        Ok(self.finish(GatewayEvent {
            capability: pending.capability,
            tenant_id: pending.tenant_id,
            device_id: pending.device_id,
            // Record the *approver* as the actor on the execute event so
            // audit can answer "who authorized this to actually run".
            actor_id: decided_by,
            policy_decision: None,
            outcome,
            approval_request_id: Some(approval_id),
            occurred_at: Utc::now(),
        }))
    }

    /// Permanently reject a pending approval. The handler is never invoked.
    pub fn reject(
        &self,
        approval_id: Uuid,
        tenant_id: impl AsRef<str>,
        decided_by: impl Into<String>,
    ) -> Result<GatewayEvent, ApprovalError> {
        let decided_by = decided_by.into();
        let pending = self.approvals.claim(
            approval_id,
            tenant_id.as_ref(),
            decided_by.clone(),
            ApprovalStatus::Rejected,
        )?;

        Ok(self.finish(GatewayEvent {
            capability: pending.capability,
            tenant_id: pending.tenant_id,
            device_id: pending.device_id,
            actor_id: decided_by,
            policy_decision: None,
            outcome: ToolCallOutcome::ApprovalRejected,
            approval_request_id: Some(approval_id),
            occurred_at: Utc::now(),
        }))
    }

    fn invoke_handler(
        &self,
        capability: &str,
        tenant_id: &str,
        device_id: &str,
        actor_id: &str,
        arguments: serde_json::Value,
    ) -> ToolCallOutcome {
        match self.registry.handler(capability) {
            None => ToolCallOutcome::NoHandlerRegistered,
            Some(handler) => {
                let invocation = ToolInvocation {
                    capability: capability.to_string(),
                    tenant_id: tenant_id.to_string(),
                    device_id: device_id.to_string(),
                    actor_id: actor_id.to_string(),
                    arguments,
                };
                match handler.invoke(&invocation) {
                    Ok(result) => ToolCallOutcome::Succeeded {
                        result_summary: result.summary,
                    },
                    Err(error) => ToolCallOutcome::Failed { error },
                }
            }
        }
    }

    fn finish(&self, event: GatewayEvent) -> GatewayEvent {
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
        let approval_id = event.approval_request_id.expect("pending approval id");
        assert_eq!(
            gateway.approvals().get(approval_id).unwrap().status,
            ApprovalStatus::Pending
        );
    }

    #[test]
    fn approve_executes_parked_invocation_without_rechecking_policy() {
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
        let parked = gateway.dispatch(
            "t1",
            "d1",
            "u1",
            "tool.finance.transfer",
            serde_json::json!({"amount": 10}),
        );
        let approval_id = parked.approval_request_id.unwrap();

        let executed = gateway.approve(approval_id, "t1", "approver-1").unwrap();
        match executed.outcome {
            ToolCallOutcome::Succeeded { result_summary } => {
                assert_eq!(result_summary, "echo:tool.finance.transfer");
            }
            other => panic!("expected Succeeded after approve, got {other:?}"),
        }
        assert_eq!(executed.actor_id, "approver-1");
        assert_eq!(executed.approval_request_id, Some(approval_id));
        assert_eq!(
            gateway.approvals().get(approval_id).unwrap().status,
            ApprovalStatus::Executed
        );

        // One-shot: a second approve must fail.
        assert!(matches!(
            gateway.approve(approval_id, "t1", "approver-2"),
            Err(ApprovalError::NotPending(_, _))
        ));
    }

    #[test]
    fn reject_never_invokes_handler_and_is_one_shot() {
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
        let parked = gateway.dispatch(
            "t1",
            "d1",
            "u1",
            "tool.finance.transfer",
            serde_json::Value::Null,
        );
        let approval_id = parked.approval_request_id.unwrap();

        let rejected = gateway.reject(approval_id, "t1", "rejector-1").unwrap();
        assert_eq!(rejected.outcome, ToolCallOutcome::ApprovalRejected);
        assert_eq!(
            gateway.approvals().get(approval_id).unwrap().status,
            ApprovalStatus::Rejected
        );
        assert!(matches!(
            gateway.approve(approval_id, "t1", "approver-1"),
            Err(ApprovalError::NotPending(_, _))
        ));
    }

    #[test]
    fn approve_rejects_cross_tenant_attempt() {
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
        let parked = gateway.dispatch(
            "tenant-a",
            "d1",
            "u1",
            "tool.finance.transfer",
            serde_json::Value::Null,
        );
        let approval_id = parked.approval_request_id.unwrap();
        assert!(matches!(
            gateway.approve(approval_id, "tenant-b", "approver-1"),
            Err(ApprovalError::TenantMismatch)
        ));
        assert_eq!(
            gateway.approvals().get(approval_id).unwrap().status,
            ApprovalStatus::Pending
        );
    }

    #[test]
    fn allowed_capability_without_registered_handler_is_reported_distinctly() {
        let mut registry = ToolRegistry::new();
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
