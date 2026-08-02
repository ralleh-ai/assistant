use std::sync::Arc;

use ralleh_policy_core::{PolicyEngine, PolicyOutcome, PolicyRequest, PolicyRule, RuleEffect};
use tokio::sync::mpsc;

use crate::backend::CompletionBackend;
#[allow(unused_imports)]
use crate::request::{
    CompletionOutcome, CompletionRequest, CompletionResponse, CompletionStreamEvent,
};

/// Routes completion requests to a configured `CompletionBackend`, gated by
/// `ralleh-policy-core` the same way `ralleh-tool-gateway` gates every tool
/// call. Policy governs *whether* a completion request may be routed at
/// all (e.g. tenant/actor/sensitivity rules); the backend itself has no
/// visibility into that decision and is only ever invoked after policy
/// allows it.
pub struct AiRouter {
    backend: Arc<dyn CompletionBackend>,
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
            backend: Arc::from(backend),
            policy: PolicyEngine::new(vec![allow_all]),
        }
    }

    /// Construct a router with an explicit policy engine, so callers (e.g.
    /// `ralleh-mcp-server`) can wire in tenant-scoped completion rules the
    /// same way they wire policy into the tool gateway.
    pub fn with_policy(backend: Box<dyn CompletionBackend>, policy: PolicyEngine) -> Self {
        Self {
            backend: Arc::from(backend),
            policy,
        }
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

    /// Streaming counterpart to [`Self::route`]. Returns an unbounded
    /// receiver of [`CompletionStreamEvent`]s; the sender side is
    /// driven by a detached tokio task that runs the same policy →
    /// complete path as `route`.
    ///
    /// # Semantics
    ///
    /// - Zero or more `Chunk` events are emitted in order. Their
    ///   `text` fields concatenate to the same string a
    ///   non-streaming `route(&request)` call would have returned
    ///   in `CompletionResponse::text`.
    /// - Exactly one terminal event follows: `Done` on success,
    ///   `Failed` / `Denied` / `ApprovalRequired` /
    ///   `NoBackendConfigured` on the corresponding non-success
    ///   paths. Terminal events on non-success paths are emitted
    ///   *without* any preceding `Chunk`s.
    /// - The channel is closed by the sender task after the
    ///   terminal event, so callers can iterate to `None` without
    ///   coordinating cancellation.
    ///
    /// # Chunking policy
    ///
    /// This landing chunks server-side by splitting the completed
    /// response on whitespace boundaries and pacing them at
    /// ~10 ms per chunk. That gives the frontend a real streaming
    /// UX today without waiting on per-backend SSE parsing (the
    /// natural follow-up: `CompletionBackend::stream_complete`
    /// with a default impl equivalent to what happens here, and
    /// per-backend overrides for OpenAI / Anthropic that consume
    /// their native event-stream formats). When those overrides
    /// land, this method's public contract does not change.
    pub fn route_stream(
        &self,
        request: &CompletionRequest,
    ) -> mpsc::UnboundedReceiver<CompletionStreamEvent> {
        let (tx, rx) = mpsc::unbounded_channel();

        // Policy check happens synchronously so denied / approval
        // paths emit their terminal event without needing to spawn
        // a task at all. This mirrors `route`'s short-circuit
        // ordering: policy decides first, backend never sees a
        // denied request.
        let policy_request = match PolicyRequest::new(
            request.tenant_id.clone(),
            request.device_id.clone(),
            request.actor_id.clone(),
            COMPLETION_CAPABILITY,
            "internal",
        ) {
            Ok(req) => req,
            Err(_) => {
                let _ = tx.send(CompletionStreamEvent::Denied);
                return rx;
            }
        };
        let decision = self.policy.evaluate(&policy_request);
        match decision.outcome {
            PolicyOutcome::Denied => {
                let _ = tx.send(CompletionStreamEvent::Denied);
                return rx;
            }
            PolicyOutcome::ApprovalRequired => {
                let _ = tx.send(CompletionStreamEvent::ApprovalRequired);
                return rx;
            }
            PolicyOutcome::Allowed => {}
        }

        // Allowed. Spawn the backend call + chunking on a task so
        // the caller gets the receiver back without waiting. `Arc`
        // clone rather than borrowing self through the task, since
        // the caller may drop the router reference before the task
        // finishes on a very short-lived query.
        let backend = self.backend.clone();
        let request = request.clone();
        tokio::spawn(async move {
            let backend_name = backend.name().to_string();
            match backend.complete(&request).await {
                Ok(response) => {
                    emit_chunks(&tx, &backend_name, &response.text).await;
                    let _ = tx.send(CompletionStreamEvent::Done {
                        backend: backend_name,
                    });
                }
                Err(error) => {
                    let _ = tx.send(CompletionStreamEvent::Failed {
                        backend: backend_name,
                        error,
                    });
                }
            }
        });

        rx
    }
}

/// Splits `text` at whitespace-preserving boundaries and pushes
/// each piece as its own `Chunk` event, with a small pacing delay
/// so the frontend sees the response arrive progressively rather
/// than in one burst.
///
/// The whitespace-preserving split is important: if we stripped
/// whitespace the reconstructed text would lose its word spacing.
/// Instead we split so each chunk is either a word or the
/// whitespace that follows it, and concatenation reproduces the
/// original exactly.
async fn emit_chunks(
    tx: &mpsc::UnboundedSender<CompletionStreamEvent>,
    backend_name: &str,
    text: &str,
) {
    // ~10 ms per chunk feels responsive without flooding the
    // frontend on a long response. Tuned against a 100-word Echo
    // reply — the eye reads it as it arrives, not as a wall.
    const PACE_MS: u64 = 10;

    for piece in split_preserving_whitespace(text) {
        if tx
            .send(CompletionStreamEvent::Chunk {
                backend: backend_name.to_string(),
                text: piece.to_string(),
            })
            .is_err()
        {
            // Receiver dropped — caller lost interest. Stop
            // spending CPU on chunks nobody will read.
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(PACE_MS)).await;
    }
}

/// Splits `text` into alternating word / whitespace pieces such
/// that concatenating all returned slices reproduces `text`
/// byte-for-byte. Handles empty input, all-whitespace input, and
/// unicode word characters correctly.
fn split_preserving_whitespace(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut current_start = 0;
    let mut in_whitespace = None;
    for (idx, ch) in text.char_indices() {
        let is_ws = ch.is_whitespace();
        match in_whitespace {
            None => in_whitespace = Some(is_ws),
            Some(prev) if prev != is_ws => {
                out.push(&text[current_start..idx]);
                current_start = idx;
                in_whitespace = Some(is_ws);
            }
            _ => {}
        }
    }
    if current_start < text.len() {
        out.push(&text[current_start..]);
    }
    out
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
    async fn route_stream_yields_chunks_that_reconstruct_the_full_response() {
        let router = AiRouter::new(Box::new(EchoBackend));
        let mut rx = router.route_stream(&sample_request("hello world"));
        let mut collected = String::new();
        let mut terminal: Option<CompletionStreamEvent> = None;
        while let Some(event) = rx.recv().await {
            match &event {
                CompletionStreamEvent::Chunk { text, .. } => collected.push_str(text),
                _ => {
                    terminal = Some(event);
                    break;
                }
            }
        }
        assert_eq!(collected, "echo: hello world");
        assert!(
            matches!(
                terminal,
                Some(CompletionStreamEvent::Done { ref backend }) if backend == "local-echo"
            ),
            "expected terminal Done, got {terminal:?}"
        );
    }

    #[tokio::test]
    async fn route_stream_denied_short_circuits_without_emitting_chunks() {
        let router = AiRouter::with_policy(Box::new(EchoBackend), PolicyEngine::new(vec![]));
        let mut rx = router.route_stream(&sample_request("hello"));
        let first = rx.recv().await.expect("terminal event");
        assert_eq!(first, CompletionStreamEvent::Denied);
        // Channel must close after the terminal event so callers
        // can loop to None without special handling.
        assert!(rx.recv().await.is_none(), "channel must close after terminal event");
    }

    #[tokio::test]
    async fn route_stream_backend_failure_is_reported_as_failed_not_a_panic() {
        let router = AiRouter::new(Box::new(AlwaysFailBackend));
        let mut rx = router.route_stream(&sample_request("hello"));
        let event = rx.recv().await.expect("terminal event");
        match event {
            CompletionStreamEvent::Failed { backend, error } => {
                assert_eq!(backend, "always-fail");
                assert_eq!(error, "upstream provider unreachable");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn split_preserving_whitespace_round_trips_arbitrary_text() {
        for input in ["", " ", "hello", "hello world", "  a  b  c  ", "多bytechars 🚀 ok"] {
            let joined: String = split_preserving_whitespace(input).concat();
            assert_eq!(joined, input, "round-trip failed for {input:?}");
        }
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
