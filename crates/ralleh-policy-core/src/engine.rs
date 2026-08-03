use chrono::Utc;
use uuid::Uuid;

use crate::{PolicyDecision, PolicyOutcome, PolicyRequest, PolicyRule, RuleEffect};

/// The policy evaluation engine: holds an ordered rule set and evaluates
/// requests against it. Deny-by-default — if no rule matches, the outcome
/// is always `Denied`, never `Allowed`.
///
/// This engine performs no I/O and holds no external state beyond the rules
/// it was constructed with, by design: it must be trivially embeddable both
/// on the edge (Tauri/Rust core) and in the control plane (server-side Rust
/// service), sharing the exact same evaluation semantics in both places
/// (see DEVELOPMENT.md ADR-004).
#[derive(Debug, Clone, Default)]
pub struct PolicyEngine {
    rules: Vec<PolicyRule>,
}

impl PolicyEngine {
    /// Construct an engine with an explicit, ordered rule set.
    /// Order matters: first matching rule wins.
    pub fn new(rules: Vec<PolicyRule>) -> Self {
        Self { rules }
    }

    /// An engine with zero rules. Every request will be denied — this is
    /// the safe default state before any policy has been configured.
    pub fn empty() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Evaluate a request against the rule set and produce an auditable
    /// decision. The request is re-validated here (defense in depth — never
    /// trust that a caller validated upstream) and an invalid request is
    /// always denied.
    pub fn evaluate(&self, request: &PolicyRequest) -> PolicyDecision {
        let now = Utc::now();

        if request.validate().is_err() {
            return PolicyDecision {
                decision_id: Uuid::new_v4(),
                tenant_id: request.tenant_id.clone(),
                device_id: request.device_id.clone(),
                actor_id: request.actor_id.clone(),
                capability: request.capability.clone(),
                sensitivity: request.sensitivity.clone(),
                outcome: PolicyOutcome::Denied,
                matched_rule_id: None,
                reason: "request failed schema validation; denied by default".to_string(),
                evaluated_at: now,
            };
        }

        for rule in &self.rules {
            if rule.matches(request) {
                let outcome = match rule.effect {
                    RuleEffect::Allow => PolicyOutcome::Allowed,
                    RuleEffect::Deny => PolicyOutcome::Denied,
                    RuleEffect::RequireApproval => PolicyOutcome::ApprovalRequired,
                };
                return PolicyDecision {
                    decision_id: Uuid::new_v4(),
                    tenant_id: request.tenant_id.clone(),
                    device_id: request.device_id.clone(),
                    actor_id: request.actor_id.clone(),
                    capability: request.capability.clone(),
                    sensitivity: request.sensitivity.clone(),
                    outcome,
                    matched_rule_id: Some(rule.id.clone()),
                    reason: rule.reason.clone(),
                    evaluated_at: now,
                };
            }
        }

        // No rule matched: deny by default. This branch is what makes the
        // engine safe to deploy with an incomplete rule set — silence in
        // policy configuration must never be interpreted as permission.
        PolicyDecision {
            decision_id: Uuid::new_v4(),
            tenant_id: request.tenant_id.clone(),
            device_id: request.device_id.clone(),
            actor_id: request.actor_id.clone(),
            capability: request.capability.clone(),
            sensitivity: request.sensitivity.clone(),
            outcome: PolicyOutcome::Denied,
            matched_rule_id: None,
            reason: "no matching rule; deny by default".to_string(),
            evaluated_at: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow_rule(id: &str, capability_prefix: &str) -> PolicyRule {
        PolicyRule {
            id: id.to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some(capability_prefix.to_string()),
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: format!("allow {capability_prefix}"),
        }
    }

    fn deny_rule(id: &str, capability_prefix: &str) -> PolicyRule {
        PolicyRule {
            id: id.to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some(capability_prefix.to_string()),
            sensitivity: None,
            effect: RuleEffect::Deny,
            reason: format!("deny {capability_prefix}"),
        }
    }

    #[test]
    fn empty_engine_denies_everything() {
        let engine = PolicyEngine::empty();
        let req = PolicyRequest::new("t1", "d1", "u1", "tool.anything", "public").unwrap();
        let decision = engine.evaluate(&req);
        assert_eq!(decision.outcome, PolicyOutcome::Denied);
        assert_eq!(decision.matched_rule_id, None);
    }

    #[test]
    fn matching_allow_rule_permits_request() {
        let engine = PolicyEngine::new(vec![allow_rule("allow-search", "tool.search")]);
        let req = PolicyRequest::new("t1", "d1", "u1", "tool.search", "public").unwrap();
        let decision = engine.evaluate(&req);
        assert_eq!(decision.outcome, PolicyOutcome::Allowed);
        assert_eq!(decision.matched_rule_id, Some("allow-search".to_string()));
    }

    #[test]
    fn non_matching_capability_falls_through_to_deny_by_default() {
        let engine = PolicyEngine::new(vec![allow_rule("allow-search", "tool.search")]);
        let req = PolicyRequest::new("t1", "d1", "u1", "tool.finance", "public").unwrap();
        let decision = engine.evaluate(&req);
        assert_eq!(decision.outcome, PolicyOutcome::Denied);
        assert_eq!(decision.matched_rule_id, None);
    }

    #[test]
    fn first_matching_rule_wins_allow_then_deny() {
        // Broad allow first, narrower deny second — allow should win because
        // it's evaluated first. This documents/enforces the "first match
        // wins" contract explicitly so future edits can't silently change it.
        let engine = PolicyEngine::new(vec![
            allow_rule("allow-all-tools", "tool."),
            deny_rule("deny-finance", "tool.finance"),
        ]);
        let req =
            PolicyRequest::new("t1", "d1", "u1", "tool.finance.transfer", "confidential").unwrap();
        let decision = engine.evaluate(&req);
        assert_eq!(decision.outcome, PolicyOutcome::Allowed);
        assert_eq!(
            decision.matched_rule_id,
            Some("allow-all-tools".to_string())
        );
    }

    #[test]
    fn first_matching_rule_wins_deny_then_allow() {
        // Reversed order: the more specific deny is placed first, so it
        // correctly takes precedence. This is how operators express
        // "deny finance actions, allow everything else" safely.
        let engine = PolicyEngine::new(vec![
            deny_rule("deny-finance", "tool.finance"),
            allow_rule("allow-all-tools", "tool."),
        ]);
        let req =
            PolicyRequest::new("t1", "d1", "u1", "tool.finance.transfer", "confidential").unwrap();
        let decision = engine.evaluate(&req);
        assert_eq!(decision.outcome, PolicyOutcome::Denied);
        assert_eq!(decision.matched_rule_id, Some("deny-finance".to_string()));
    }

    #[test]
    fn require_approval_effect_maps_to_approval_required_outcome() {
        let rule = PolicyRule {
            id: "approval-required".to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.finance.".to_string()),
            sensitivity: None,
            effect: RuleEffect::RequireApproval,
            reason: "financial actions require human approval".to_string(),
        };
        let engine = PolicyEngine::new(vec![rule]);
        let req =
            PolicyRequest::new("t1", "d1", "u1", "tool.finance.transfer", "confidential").unwrap();
        let decision = engine.evaluate(&req);
        assert_eq!(decision.outcome, PolicyOutcome::ApprovalRequired);
    }

    #[test]
    fn tenant_scoped_rule_does_not_leak_to_other_tenants() {
        let rule = PolicyRule {
            id: "tenant-a-only".to_string(),
            tenant_id: Some("tenant-a".to_string()),
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.search".to_string()),
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "tenant-a search access".to_string(),
        };
        let engine = PolicyEngine::new(vec![rule]);

        let req_a = PolicyRequest::new("tenant-a", "d1", "u1", "tool.search", "public").unwrap();
        let req_b = PolicyRequest::new("tenant-b", "d1", "u1", "tool.search", "public").unwrap();

        assert_eq!(engine.evaluate(&req_a).outcome, PolicyOutcome::Allowed);
        // Critical: a different tenant must NOT inherit tenant-a's rule and
        // must fall through to deny-by-default. This is the automated test
        // that directly enforces the "no cross-tenant leakage" invariant.
        assert_eq!(engine.evaluate(&req_b).outcome, PolicyOutcome::Denied);
    }

    #[test]
    fn invalid_request_is_denied_even_with_permissive_rules() {
        // Defense in depth: even an engine configured to allow everything
        // must deny malformed requests, because evaluation always
        // re-validates input rather than trusting the caller.
        let engine = PolicyEngine::new(vec![allow_rule("allow-all", "")]);
        let bad_req = PolicyRequest {
            tenant_id: "".to_string(), // invalid: empty
            device_id: "d1".to_string(),
            actor_id: "u1".to_string(),
            capability: "tool.search".to_string(),
            sensitivity: "public".to_string(),
            context: serde_json::Value::Null,
        };
        let decision = engine.evaluate(&bad_req);
        assert_eq!(decision.outcome, PolicyOutcome::Denied);
        assert!(decision.reason.contains("schema validation"));
    }

    #[test]
    fn decision_carries_full_correlatable_context_for_audit() {
        let engine = PolicyEngine::new(vec![allow_rule("allow-search", "tool.search")]);
        let req = PolicyRequest::new("t1", "d1", "u1", "tool.search", "public").unwrap();
        let decision = engine.evaluate(&req);
        assert_eq!(decision.tenant_id, "t1");
        assert_eq!(decision.device_id, "d1");
        assert_eq!(decision.actor_id, "u1");
        assert_eq!(decision.capability, "tool.search");
        assert_eq!(decision.sensitivity, "public");
    }
}
