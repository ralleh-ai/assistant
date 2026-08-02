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
        }
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
    fn completion_request_carries_the_shell_identity() {
        let req = completion_request("acme", "desk-1", "rico", "hello");
        assert_eq!(req.tenant_id, "acme");
        assert_eq!(req.device_id, "desk-1");
        assert_eq!(req.actor_id, "rico");
        assert_eq!(req.prompt, "hello");
        assert!(req.model_hint.is_none());
    }
}
