use std::sync::{Arc, Mutex};

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
    /// Backend behind an `Arc<Mutex<Arc<..>>>` so the router supports
    /// live swap without breaking in-flight requests. Every request
    /// snapshots the current backend `Arc` under a brief lock, drops
    /// the lock, and then does its work with that snapshot -- a
    /// subsequent `swap_backend` therefore only affects requests
    /// that start after it, not requests already routed through the
    /// old backend. This is what lets the desktop shell's settings
    /// UI reconfigure the completion provider without a restart.
    backend: Arc<Mutex<Arc<dyn CompletionBackend>>>,
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
            backend: Arc::new(Mutex::new(Arc::from(backend))),
            policy: PolicyEngine::new(vec![allow_all]),
        }
    }

    /// Construct a router with an explicit policy engine, so callers (e.g.
    /// `ralleh-mcp-server`) can wire in tenant-scoped completion rules the
    /// same way they wire policy into the tool gateway.
    pub fn with_policy(backend: Box<dyn CompletionBackend>, policy: PolicyEngine) -> Self {
        Self::with_policy_arc(Arc::from(backend), policy)
    }

    /// `Arc`-taking sibling of `with_policy`. Preferred when the
    /// caller already holds the backend behind an `Arc` (e.g. the
    /// shell reuses one Arc across `AssistantState::reconfigure`
    /// and the throwaway "test connection" probe). Skips the
    /// pointless `Box → Arc` round-trip.
    pub fn with_policy_arc(backend: Arc<dyn CompletionBackend>, policy: PolicyEngine) -> Self {
        Self {
            backend: Arc::new(Mutex::new(backend)),
            policy,
        }
    }

    /// `Arc`-taking sibling of `new`. Same permissive default
    /// policy, spared the `Box → Arc` round-trip.
    pub fn with_backend_arc(backend: Arc<dyn CompletionBackend>) -> Self {
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
        Self::with_policy_arc(backend, PolicyEngine::new(vec![allow_all]))
    }

    /// Replace the router's backend at runtime. In-flight requests
    /// keep running against whichever backend they snapshotted at
    /// `route`/`route_stream` entry -- only requests starting after
    /// this call see the new one. Used by the shell's settings UI to
    /// reconfigure the completion provider without a restart.
    ///
    /// A poisoned mutex (unreachable in practice -- the critical
    /// sections are `Arc::clone` and one assignment, neither of which
    /// can panic) is silently no-oped rather than propagated: the
    /// caller has no meaningful recovery, and refusing to swap is
    /// safer than proceeding with an unknown internal state.
    pub fn swap_backend(&self, backend: Arc<dyn CompletionBackend>) {
        if let Ok(mut slot) = self.backend.lock() {
            *slot = backend;
        }
    }

    /// Snapshot of the current backend's `name()` -- stable enough
    /// to surface in UI ("Currently: anthropic") and telemetry
    /// without exposing the underlying `Arc<dyn ..>` to callers.
    /// Returns an empty string if the internal mutex is poisoned;
    /// see `swap_backend` for why we treat that as "unknown" rather
    /// than "propagate".
    pub fn current_backend_name(&self) -> String {
        match self.backend.lock() {
            Ok(g) => g.name().to_string(),
            Err(_) => String::new(),
        }
    }

    /// Snapshot the current backend `Arc`. Cheap `Arc::clone` under a
    /// brief mutex lock. Used inside `route`/`route_stream` so the
    /// rest of the request can proceed without holding the lock, and
    /// exposed for callers that need to run a single request against
    /// the current backend directly (e.g. the shell's "test
    /// connection" command routes through a fresh throwaway router
    /// so a failing test can't leave in-flight state on the
    /// production router).
    fn snapshot_backend(&self) -> Arc<dyn CompletionBackend> {
        // `.expect_or` isn't a thing; `unwrap_or_else` on a poisoned
        // mutex would need a fallback backend, which we don't have
        // here. Poisoned = programmer error, and every write site is
        // a two-line critical section, so `expect` documents the
        // "cannot happen" clearly. If it ever does happen the panic
        // is louder than a silent stall.
        self.backend
            .lock()
            .expect("AiRouter backend mutex poisoned")
            .clone()
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

        let backend = self.snapshot_backend();
        match backend.complete(request).await {
            Ok(response) => CompletionOutcome::Succeeded(response),
            Err(error) => CompletionOutcome::Failed {
                backend: backend.name().to_string(),
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
    /// Chunking is a per-backend concern, delegated to
    /// `CompletionBackend::stream_complete`. The router no longer
    /// splits the completed response server-side: real backends
    /// (`HttpCompletionBackend`) parse Server-Sent Events and emit
    /// deltas as they arrive from the provider, while
    /// backends without native streaming
    /// (fallback via the default trait method) yield the full
    /// response as one chunk. Development backends may add their
    /// own visible pacing (`EchoBackend` splits word-by-word). The
    /// router just forwards whatever chunks the backend produces
    /// and appends the terminal event.
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

        // Allowed. Snapshot the backend once, then spawn the stream
        // + forwarder on a task so the caller gets the receiver back
        // without waiting. Snapshotting here means an in-flight
        // request keeps using the backend it started with even if
        // the router is swapped mid-flight (see `swap_backend`).
        let backend = self.snapshot_backend();
        let request = request.clone();
        tokio::spawn(async move {
            let backend_name = backend.name().to_string();
            let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel::<Result<String, String>>();

            // Drive the backend on a subtask so we can concurrently
            // forward chunks as they arrive rather than waiting for
            // the whole stream to buffer.
            let backend_task = {
                let backend = backend.clone();
                let request = request.clone();
                tokio::spawn(async move {
                    backend.stream_complete(&request, chunk_tx).await;
                })
            };

            let mut fatal: Option<String> = None;
            while let Some(item) = chunk_rx.recv().await {
                match item {
                    Ok(text) => {
                        // Drop empty chunks -- providers occasionally
                        // emit role-only prefix frames with no content,
                        // and the frontend has no use for zero-length
                        // pieces.
                        if text.is_empty() {
                            continue;
                        }
                        if tx
                            .send(CompletionStreamEvent::Chunk {
                                backend: backend_name.clone(),
                                text,
                            })
                            .is_err()
                        {
                            // Downstream dropped the receiver.
                            // Abandon the backend task and stop.
                            backend_task.abort();
                            return;
                        }
                    }
                    Err(error) => {
                        fatal = Some(error);
                        break;
                    }
                }
            }
            // Ensure the backend subtask has exited before we send
            // the terminal event, so `Done` never races with an
            // in-flight chunk.
            let _ = backend_task.await;

            let terminal = match fatal {
                None => CompletionStreamEvent::Done {
                    backend: backend_name,
                },
                Some(error) => CompletionStreamEvent::Failed {
                    backend: backend_name,
                    error,
                },
            };
            let _ = tx.send(terminal);
        });

        rx
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
        assert!(
            rx.recv().await.is_none(),
            "channel must close after terminal event"
        );
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

    #[tokio::test]
    async fn swap_backend_switches_which_backend_serves_new_requests() {
        // Before swap: EchoBackend is in place, response echoes.
        let router = AiRouter::new(Box::new(EchoBackend));
        assert_eq!(router.current_backend_name(), "local-echo");
        let outcome = router.route(&sample_request("hi")).await;
        assert!(matches!(outcome, CompletionOutcome::Succeeded(ref r) if r.text == "echo: hi"));

        // Swap in AlwaysFailBackend. New requests must see the failure.
        router.swap_backend(std::sync::Arc::new(AlwaysFailBackend));
        assert_eq!(router.current_backend_name(), "always-fail");
        let outcome = router.route(&sample_request("hi")).await;
        match outcome {
            CompletionOutcome::Failed { backend, error } => {
                assert_eq!(backend, "always-fail");
                assert_eq!(error, "upstream provider unreachable");
            }
            other => panic!("expected Failed after swap, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn route_stream_after_swap_uses_the_new_backend() {
        let router = AiRouter::new(Box::new(EchoBackend));
        router.swap_backend(std::sync::Arc::new(AlwaysFailBackend));
        let mut rx = router.route_stream(&sample_request("hi"));
        let event = rx.recv().await.expect("terminal event");
        match event {
            CompletionStreamEvent::Failed { backend, .. } => assert_eq!(backend, "always-fail"),
            other => panic!("expected Failed, got {other:?}"),
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
