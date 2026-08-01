use std::sync::Arc;

use ralleh_ai_router::{AiRouter, CompletionBackend, EchoBackend, HttpCompletionBackend};
use ralleh_audit_store::JsonlFileAuditSink;
use ralleh_tool_gateway::{ApprovalStore, ToolGateway};
use ralleh_mcp_server::{build_router, resolve_config_path, AppState, ServerConfig};

/// Binary entrypoint for the Rust MCP/tool-gateway HTTP surface.
///
/// Registry + policy rules are loaded from a declarative config file
/// (`config/default.toml` by default, or `RALLEH_CONFIG`), matching
/// DEVELOPMENT.md §8.3. Handler implementations stay in code; the config
/// only chooses which known handlers to wire and under which capability
/// ids / policy effects.
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr = std::env::var("RALLEH_MCP_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_string());

    let config_path = resolve_config_path().unwrap_or_else(|e| {
        panic!("failed to resolve server config: {e}");
    });
    tracing::info!(path = %config_path.display(), "loading server config");
    let config = ServerConfig::load_from_path(&config_path).unwrap_or_else(|e| {
        panic!("failed to load server config from {}: {e}", config_path.display());
    });

    let sandbox_dir = config.resolve_sandbox_root().unwrap_or_else(|e| {
        panic!("failed to resolve sandbox root: {e}");
    });
    let registry = config.build_registry(&sandbox_dir).unwrap_or_else(|e| {
        panic!("failed to build tool registry from config: {e}");
    });
    let policy = config.build_policy_engine();

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

    // Pending approvals survive process restarts when backed by this JSON
    // snapshot (override with RALLEH_APPROVAL_STORE_PATH).
    let approval_store_path = std::env::var("RALLEH_APPROVAL_STORE_PATH").unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("ralleh-approvals.json")
            .display()
            .to_string()
    });
    let approvals = Arc::new(
        ApprovalStore::open(&approval_store_path)
            .unwrap_or_else(|e| panic!("failed to open approval store {approval_store_path}: {e}")),
    );
    tracing::info!(path = %approval_store_path, "approval store ready");

    let gateway = ToolGateway::with_audit_sink_and_approvals(
        registry,
        policy,
        audit_sink.clone(),
        approvals,
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

    tracing::info!(
        %addr,
        sandbox = %sandbox_dir.display(),
        audit_log = %audit_sink.path().display(),
        approval_store = %approval_store_path,
        config = %config_path.display(),
        "starting ralleh-mcp-server"
    );

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));

    axum::serve(listener, app)
        .await
        .expect("mcp server exited unexpectedly");
}
