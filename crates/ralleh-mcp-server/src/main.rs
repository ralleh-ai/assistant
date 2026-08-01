use std::sync::Arc;

use ralleh_ai_router::{AiRouter, CompletionBackend, EchoBackend, HttpCompletionBackend};
use ralleh_audit_store::JsonlFileAuditSink;
use ralleh_policy_core::{PolicyEngine, PolicyRule, RuleEffect};
use ralleh_tool_gateway::{
    FsReadTextHandler, FsWriteTextHandler, ToolDefinition, ToolGateway, ToolRegistry,
};
use ralleh_mcp_server::{build_router, AppState};

/// Binary entrypoint for the Rust MCP/tool-gateway HTTP surface.
///
/// This wiring is intentionally minimal and explicit for now: it registers
/// the one real (non-mocked) handler that exists today
/// (`FsReadTextHandler`, sandboxed to a temp directory) behind a
/// conservative allow rule, so the server is genuinely useful to smoke-test
/// end-to-end rather than serving only 404s. Production configuration
/// (loading registry/policy from config files, real handlers, secrets,
/// etc.) is a follow-up once more of the spine is proven.
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr = std::env::var("RALLEH_MCP_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_string());

    let sandbox_dir = std::env::temp_dir().join("ralleh-mcp-server-sandbox");
    std::fs::create_dir_all(&sandbox_dir).expect("failed to create sandbox directory");

    let mut registry = ToolRegistry::new();
    registry.register(
        ToolDefinition {
            capability: "tool.fs.read_text".to_string(),
            description: "Read a UTF-8 text file from the server's sandboxed directory."
                .to_string(),
            default_sensitivity: "internal".to_string(),
        },
        Box::new(
            FsReadTextHandler::new(&sandbox_dir).expect("failed to initialize fs sandbox handler"),
        ),
    );
    registry.register(
        ToolDefinition {
            capability: "tool.fs.write_text".to_string(),
            description: "Write a UTF-8 text file into the server's sandboxed directory."
                .to_string(),
            default_sensitivity: "internal".to_string(),
        },
        Box::new(
            FsWriteTextHandler::new(&sandbox_dir)
                .expect("failed to initialize fs write sandbox handler"),
        ),
    );

    let allow_fs_read = PolicyRule {
        id: "default-allow-fs-read".to_string(),
        tenant_id: None,
        device_id: None,
        actor_id: None,
        capability_prefix: Some("tool.fs.read_text".to_string()),
        sensitivity: None,
        effect: RuleEffect::Allow,
        reason: "default development policy: sandboxed fs reads are allowed".to_string(),
    };

    // Writes are gated separately from reads and, unlike reads, require
    // approval by default: mutating the sandbox is a materially different
    // risk than reading from it, and DEVELOPMENT.md's policy-gating
    // non-negotiable means that distinction has to be expressed as an
    // actual policy rule, not just left to the handler's own defenses
    // (refuse-overwrite, root confinement) to carry alone.
    let require_approval_fs_write = PolicyRule {
        id: "default-approval-fs-write".to_string(),
        tenant_id: None,
        device_id: None,
        actor_id: None,
        capability_prefix: Some("tool.fs.write_text".to_string()),
        sensitivity: None,
        effect: RuleEffect::RequireApproval,
        reason: "default development policy: sandboxed fs writes require approval".to_string(),
    };

    // Every GatewayEvent produced by the gateway (allowed, denied,
    // approval-required, handler failure -- all of it) is appended to this
    // JSONL file before `dispatch` returns, closing the audit-persistence
    // gap called out in DEVELOPMENT.md: policy decisions were being made
    // and *discarded* rather than durably recorded. Path is overridable via
    // `RALLEH_AUDIT_LOG_PATH` for deployments that want the log elsewhere
    // (e.g. a mounted volume) without a code change.
    let audit_log_path = std::env::var("RALLEH_AUDIT_LOG_PATH")
        .unwrap_or_else(|_| std::env::temp_dir().join("ralleh-audit.jsonl").display().to_string());
    let audit_sink = Arc::new(
        JsonlFileAuditSink::open(&audit_log_path).expect("failed to open audit log for writing"),
    );

    let gateway = ToolGateway::with_audit_sink(
        registry,
        PolicyEngine::new(vec![allow_fs_read, require_approval_fs_write]),
        audit_sink.clone(),
    );
    // EchoBackend is the local dev/test completion backend -- no real
    // provider credentials required yet. If RALLEH_AI_BASE_URL is set, we
    // instead wire up a real HttpCompletionBackend speaking the
    // OpenAI-compatible /chat/completions wire format (covers OpenAI
    // itself, and self-hosted vllm/ollama/llama.cpp servers), so this
    // server can be pointed at a real backend without a code change.
    let ai_backend: Box<dyn CompletionBackend> = match std::env::var("RALLEH_AI_BASE_URL") {
        Ok(base_url) => {
            let model = std::env::var("RALLEH_AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
            let api_key = std::env::var("RALLEH_AI_API_KEY").ok();
            let backend_name =
                std::env::var("RALLEH_AI_BACKEND_NAME").unwrap_or_else(|_| "http-backend".to_string());
            tracing::info!(base_url = %base_url, model = %model, "configuring HttpCompletionBackend");
            Box::new(HttpCompletionBackend::new(backend_name, base_url, model, api_key))
        }
        Err(_) => {
            tracing::info!("RALLEH_AI_BASE_URL not set; falling back to local EchoBackend");
            Box::new(EchoBackend)
        }
    };
    let ai_router = AiRouter::new(ai_backend);
    let state = AppState::new(gateway, ai_router);
    let app = build_router(state);

    tracing::info!(%addr, sandbox = %sandbox_dir.display(), audit_log = %audit_sink.path().display(), "starting ralleh-mcp-server");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));

    axum::serve(listener, app)
        .await
        .expect("mcp server exited unexpectedly");
}
