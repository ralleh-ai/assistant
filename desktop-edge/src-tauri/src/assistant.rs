//! In-process router + tool-gateway owned by the shell.
//!
//! Phase 3 §3.2 of `../../docs/PRESENCE_INTEGRATION_PLAN.md`: real
//! `thinking` and `tool_use` engagements need a real work source.
//! Rather than a shell-local stub, this module embeds the two
//! crates that already do the job — `ralleh_ai_router::AiRouter` for
//! completion routing and `ralleh_tool_gateway::ToolGateway` for
//! capability dispatch — so the presence reflects real policy
//! decisions from the start.
//!
//! # Defaults
//!
//! `EchoBackend` is the router's default backend and `EchoHandler`
//! is registered for a single scaffold capability (`assistant.tool.echo`).
//! Neither reaches the network or the filesystem. When Phase 4 wires
//! in a real HTTP completion backend, it slots in behind
//! `CompletionBackend` here without touching the shell's mode-signal
//! plumbing.
//!
//! # Policy
//!
//! Uses the crates' permissive dev defaults (`AiRouter::new` allows
//! all `ai.completion.route`, and the tool registry's policy allows
//! the scaffold capability). Production wiring will pass explicit
//! rule sets in — the shell has no opinion on those rules, it just
//! surfaces the outcome visually.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ralleh_ai_router::{AiRouter, CompletionRequest, EchoBackend};
use ralleh_policy_core::{PolicyEngine, PolicyRule, RuleEffect};
use ralleh_tool_gateway::{
    EchoHandler, ToolDefinition, ToolGateway, ToolRegistry,
};

/// Scaffold capability the dev panel dispatches through the tool
/// gateway. Real capabilities (fs.read.text, http.fetch, etc.) will
/// land as separate registrations once their handlers are wired.
pub const ECHO_CAPABILITY: &str = "assistant.tool.echo";

/// Bundle of Ralleh subsystems the assistant flows through. Cloned
/// aggressively so async Tauri command handlers can move the pieces
/// they need without holding a `State` guard across `.await` points
/// (which would keep the underlying `Mutex` locked longer than
/// necessary). `AiRouter` and `ToolGateway` both hold `Arc`s
/// internally where it matters, so cheap to clone-wrap here.
pub struct AssistantState {
    pub router: Arc<AiRouter>,
    pub gateway: Arc<ToolGateway>,
    /// In-flight assistant operation counter (Phase 3 §3.1). Every
    /// path that engages `thinking` / `tool_use` / `speaking` also
    /// holds a `WorkGuard` from `begin_work`, so `is_idle()` is a
    /// real "no assistant work in flight" answer rather than a
    /// synthetic default. Consumers: the scan-sweep thread that
    /// only fires attention pulses when idle (§3.4), and future
    /// observers (status-line, telemetry).
    in_flight: Arc<AtomicUsize>,
}

/// RAII guard returned by [`AssistantState::begin_work`]. Increments
/// the in-flight counter on construction and decrements on drop.
/// Cheap to clone-hold across `.await` in async command handlers
/// because it's just an `Arc<AtomicUsize>`.
pub struct WorkGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        // `Release` on the decrement pairs with any future `Acquire`
        // load in an observer that wants to see the last work
        // completing before treating the assistant as idle.
        self.counter.fetch_sub(1, Ordering::Release);
    }
}

impl AssistantState {
    /// Construct with dev defaults: `EchoBackend`, echo tool handler,
    /// permissive policy. Called once from Tauri's `.setup()` and
    /// installed as managed state.
    pub fn with_defaults() -> Self {
        let router = AiRouter::new(Box::new(EchoBackend));

        let mut registry = ToolRegistry::new();
        registry.register(
            ToolDefinition {
                capability: ECHO_CAPABILITY.into(),
                description: "scaffold echo capability for the dev panel".into(),
                default_sensitivity: "internal".into(),
            },
            Box::new(EchoHandler),
        );
        // Permissive policy for the scaffold capability. Same shape
        // the router uses internally, kept explicit here so a reader
        // sees the policy that gates this specific capability rather
        // than having to trace defaults.
        let allow_echo = PolicyRule {
            id: "shell-scaffold-allow-echo".into(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some(ECHO_CAPABILITY.into()),
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "dev scaffold: shell-embedded echo tool is allowed".into(),
        };
        let gateway = ToolGateway::new(registry, PolicyEngine::new(vec![allow_echo]));

        Self {
            router: Arc::new(router),
            gateway: Arc::new(gateway),
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Increment the in-flight counter for the lifetime of the
    /// returned guard. Every Tauri command that engages a sustained
    /// presence mode should hold one alongside its `ModeHold` so the
    /// two go in and out of scope together.
    pub fn begin_work(&self) -> WorkGuard {
        // `Acquire` on the increment matches the `Release` on drop —
        // an observer that sees the decrement is guaranteed to see
        // everything the work did before it, which is what makes
        // "idle now" a safe thing to act on (e.g. the scan sweep).
        self.in_flight.fetch_add(1, Ordering::Acquire);
        WorkGuard {
            counter: self.in_flight.clone(),
        }
    }

    /// Cheap `Acquire` read of the in-flight counter. Non-zero means
    /// at least one assistant operation is currently executing.
    ///
    /// `allow(dead_code)` on the two convenience readers below: they
    /// are the public observation surface for future consumers
    /// (status-line, telemetry) and are exercised by the unit tests,
    /// but no non-test call site exists yet — the scan sweep reads
    /// the raw `Arc<AtomicUsize>` from `in_flight_handle` directly.
    #[allow(dead_code)]
    pub fn in_flight_count(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Convenience: `true` when no assistant work is in flight. Used
    /// by the scan-sweep thread (§3.4) to gate sparse attention
    /// pulses so they never compete with real activity.
    #[allow(dead_code)]
    pub fn is_idle(&self) -> bool {
        self.in_flight_count() == 0
    }

    /// Shared handle to the counter so background threads (the scan
    /// sweep, most notably) can observe idleness without holding a
    /// clone of the whole `AssistantState`.
    pub fn in_flight_handle(&self) -> Arc<AtomicUsize> {
        self.in_flight.clone()
    }
}

/// Helper: build a `CompletionRequest` from the shell's identity
/// fields. Kept out of the Tauri handler so it can be unit-tested
/// without a live Tauri context.
pub fn completion_request(
    tenant_id: &str,
    device_id: &str,
    actor_id: &str,
    prompt: &str,
) -> CompletionRequest {
    CompletionRequest {
        tenant_id: tenant_id.into(),
        device_id: device_id.into(),
        actor_id: actor_id.into(),
        model_hint: None,
        prompt: prompt.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_defaults_registers_the_scaffold_echo_capability() {
        let state = AssistantState::with_defaults();
        // Dispatch a benign call and prove it lands on `EchoHandler`
        // (not a policy denial, not "unknown capability"). The
        // handler summary is stable so pinning it here catches a
        // future refactor that silently swaps handlers.
        let event = state.gateway.dispatch(
            "acme",
            "desk-1",
            "rico",
            ECHO_CAPABILITY,
            serde_json::json!({ "ping": true }),
        );
        use ralleh_tool_gateway::ToolCallOutcome;
        match event.outcome {
            ToolCallOutcome::Succeeded { result_summary } => {
                assert_eq!(result_summary, format!("echo:{ECHO_CAPABILITY}"));
            }
            other => panic!("expected Succeeded, got {other:?}"),
        }
    }

    #[test]
    fn begin_work_increments_and_drop_decrements_the_counter() {
        let state = AssistantState::with_defaults();
        assert!(state.is_idle());
        assert_eq!(state.in_flight_count(), 0);

        let g1 = state.begin_work();
        assert_eq!(state.in_flight_count(), 1);
        assert!(!state.is_idle());

        {
            let _g2 = state.begin_work();
            assert_eq!(state.in_flight_count(), 2);
        }
        // g2 dropped: back to 1.
        assert_eq!(state.in_flight_count(), 1);
        assert!(!state.is_idle());

        drop(g1);
        assert_eq!(state.in_flight_count(), 0);
        assert!(state.is_idle());
    }

    #[test]
    fn in_flight_handle_observes_the_same_counter() {
        // The scan-sweep thread holds a raw Arc<AtomicUsize> rather
        // than the whole AssistantState. Confirm the two views agree
        // so a future refactor that swaps the counter type doesn't
        // silently split the observation surface.
        let state = AssistantState::with_defaults();
        let handle = state.in_flight_handle();
        let _guard = state.begin_work();
        assert_eq!(handle.load(std::sync::atomic::Ordering::Acquire), 1);
    }

    #[test]
    fn completion_request_carries_the_shell_identity() {
        let req = completion_request("acme", "desk-1", "rico", "hello");
        assert_eq!(req.tenant_id, "acme");
        assert_eq!(req.device_id, "desk-1");
        assert_eq!(req.actor_id, "rico");
        assert_eq!(req.prompt, "hello");
        assert!(req.model_hint.is_none());
    }
}
