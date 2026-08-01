use ralleh_policy_core::{PolicyEngine, PolicyOutcome, PolicyRequest, PolicyRule, RuleEffect};

use crate::backend::CompletionBackend;
#[allow(unused_imports)]
use crate::request::{CompletionOutcome, CompletionRequest, CompletionResponse};

/// Routes completion requests to a configured `CompletionBackend`, gated by
/// `ralleh-policy-core` the same way `ralleh-tool-gateway` gates every tool
/// call. Policy governs *whether* a completion request may be routed at
/// all (e.g. tenant/actor/sensitivity rules); the backend itself has no
/// visibility into that decision and is only ever invoked after policy
/// allows it.
pub struct AiRouter {
    backend: Box<dyn CompletionBackend>,
    policy: PolicyEngine,
}

#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error("no backend is configured for this router")]
    NoBackendConfigured,
}

/// The capability name used for policy evaluation of completion requests.
/// Kept as a single constant so policy rules and router logic can never
/// drift out of sync with each other.
pub const COMPLETION_CAPABILITY: &str = "ai.completion.route";

impl AiRouter {
    /// Construct a router with a permissive default policy (allow all
    /// completion requests). This mirrors the dev-friendly default used
    /// elsewhere (e.g. the mcp-server's default fs-read allow rule) --
    /// production wiring should use `with_policy` with an explicit,
    /// intentional rule set instead of relying on this default.
    pub fn new(backend: Box<dyn CompletionBackend>) -> Self {
        let allow_all = PolicyRule {
            id: "default-allow-completion".to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some(COMPLETION_CAPABILITY.to_string()),
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "default development policy: completion routing is allowed".to_string(),
        };
        Self {
            backend,
            policy: PolicyEngine::new(vec![allow_all]),
        }
    }

    /// Construct a router with an explicit policy engine, so callers (e.g.
    /// `ralleh-mcp-server`) can wire in tenant-scoped completion rules the
    /// same way they wire policy into the tool gateway.
    pub fn with_policy(backend: Box<dyn CompletionBackend>, policy: PolicyEngine) -> Self {
        Self { backend, policy }
    }

    /// Route a request to the configured backend and return a
    /// `CompletionOutcome` -- never a bare panic or unhandled error, so
    /// callers (e.g. `ralleh-mcp-server`'s `/v1/completions` route) can
    /// translate the outcome directly into an HTTP status the same way
    /// `ToolCallOutcome` is translated today.
    ///
    /// Every call is policy-evaluated first. A denied or approval-required
    /// decision short-circuits before the backend is ever invoked, mirroring
    /// `ToolGateway::dispatch`'s "policy decides first" ordering.
    pub async fn route(&self, request: &CompletionRequest) -> CompletionOutcome {
        let policy_request = match PolicyRequest::new(
            request.tenant_id.clone(),
            request.device_id.clone(),
            request.actor_id.clone(),
            COMPLETION_CAPABILITY,
            "internal",
        ) {
            Ok(req) => req,
            Err(_) => return CompletionOutcome::Denied,
        };

        let decision = self.policy.evaluate(&policy_request);
        match decision.outcome {
            PolicyOutcome::Denied => return CompletionOutcome::Denied,
            PolicyOutcome::ApprovalRequired => return CompletionOutcome::ApprovalRequired,
            PolicyOutcome::Allowed => {}
        }

        match self.backend.complete(request).await {
            Ok(response) => CompletionOutcome::Succeeded(response),
            Err(error) => CompletionOutcome::Failed {
                backend: self.backend.name().to_string(),
                error,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::EchoBackend;
    use async_trait::async_trait;

    fn sample_request(prompt: &str) -> CompletionRequest {
        CompletionRequest {
            tenant_id: "t1".to_string(),
            device_id: "d1".to_string(),
            actor_id: "u1".to_string(),
            model_hint: None,
            prompt: prompt.to_string(),
        }
    }

    #[tokio::test]
    async fn routes_to_configured_backend_and_succeeds() {
        let router = AiRouter::new(Box::new(EchoBackend));
        let outcome = router.route(&sample_request("hello")).await;
        match outcome {
            CompletionOutcome::Succeeded(CompletionResponse { backend, text }) => {
                assert_eq!(backend, "local-echo");
                assert_eq!(text, "echo: hello");
            }
            other => panic!("expected Succeeded, got {other:?}"),
        }
    }

    struct AlwaysFailBackend;

    #[async_trait]
    impl CompletionBackend for AlwaysFailBackend {
        fn name(&self) -> &str {
            "always-fail"
        }

        async fn complete(
            &self,
            _request: &CompletionRequest,
        ) -> Result<CompletionResponse, String> {
            Err("upstream provider unreachable".to_string())
        }
    }

    #[tokio::test]
    async fn backend_failure_is_reported_as_failed_outcome_not_a_panic() {
        let router = AiRouter::new(Box::new(AlwaysFailBackend));
        let outcome = router.route(&sample_request("hello")).await;
        match outcome {
            CompletionOutcome::Failed { backend, error } => {
                assert_eq!(backend, "always-fail");
                assert_eq!(error, "upstream provider unreachable");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_prompt_still_routes_without_special_casing() {
        // The router itself has no opinion on prompt validity -- that is a
        // backend concern, not a routing/policy concern. This test
        // documents that boundary explicitly.
        let router = AiRouter::new(Box::new(EchoBackend));
        let outcome = router.route(&sample_request("")).await;
        assert_eq!(
            outcome,
            CompletionOutcome::Succeeded(CompletionResponse {
                backend: "local-echo".to_string(),
                text: "echo: ".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn empty_policy_engine_denies_completion_by_default() {
        // Proves the "policy decides first" ordering: with no allow rule at
        // all, the backend must never be invoked and the outcome must be
        // Denied, not Succeeded/Failed.
        let router = AiRouter::with_policy(Box::new(EchoBackend), PolicyEngine::new(vec![]));
        let outcome = router.route(&sample_request("hello")).await;
        assert_eq!(outcome, CompletionOutcome::Denied);
    }

    #[tokio::test]
    async fn approval_required_rule_short_circuits_before_backend_is_invoked() {
        let approval_rule = PolicyRule {
            id: "approval-completion".to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some(COMPLETION_CAPABILITY.to_string()),
            sensitivity: None,
            effect: RuleEffect::RequireApproval,
            reason: "test".to_string(),
        };
        let router = AiRouter::with_policy(
            Box::new(AlwaysFailBackend),
            PolicyEngine::new(vec![approval_rule]),
        );
        let outcome = router.route(&sample_request("hello")).await;
        // If the backend (AlwaysFailBackend) had been invoked we'd see
        // Failed, not ApprovalRequired -- proves the short-circuit.
        assert_eq!(outcome, CompletionOutcome::ApprovalRequired);
    }

    #[tokio::test]
    async fn tenant_scoped_denial_does_not_leak_to_other_tenants() {
        let scoped_allow = PolicyRule {
            id: "allow-tenant-a-only".to_string(),
            tenant_id: Some("tenant-a".to_string()),
            device_id: None,
            actor_id: None,
            capability_prefix: Some(COMPLETION_CAPABILITY.to_string()),
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "test".to_string(),
        };
        let router =
            AiRouter::with_policy(Box::new(EchoBackend), PolicyEngine::new(vec![scoped_allow]));

        let mut allowed_request = sample_request("hello");
        allowed_request.tenant_id = "tenant-a".to_string();
        let allowed_outcome = router.route(&allowed_request).await;
        assert!(matches!(allowed_outcome, CompletionOutcome::Succeeded(_)));

        let mut denied_request = sample_request("hello");
        denied_request.tenant_id = "tenant-b".to_string();
        let denied_outcome = router.route(&denied_request).await;
        assert_eq!(denied_outcome, CompletionOutcome::Denied);
    }
}
