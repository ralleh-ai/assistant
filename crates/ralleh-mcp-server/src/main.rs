use std::sync::Arc;

use ralleh_ai_router::{
    AiRouter, AnthropicMessagesBackend, CompletionBackend, EchoBackend, HttpCompletionBackend,
};
use ralleh_audit_store::JsonlFileAuditSink;
use ralleh_mcp_server::{
    build_router, resolve_config_path, AppState, ServerConfig, TokenAuthenticator,
};
use ralleh_tool_gateway::{ApprovalStore, ToolGateway};

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
        panic!(
            "failed to load server config from {}: {e}",
            config_path.display()
        );
    });

    let sandbox_dir = config.resolve_sandbox_root().unwrap_or_else(|e| {
        panic!("failed to resolve sandbox root: {e}");
    });
    let registry = config.build_registry(&sandbox_dir).unwrap_or_else(|e| {
        panic!("failed to build tool registry from config: {e}");
    });
    let policy = config.build_policy_engine();

    let audit_log_path = std::env::var("RALLEH_AUDIT_LOG_PATH").unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("ralleh-audit.jsonl")
            .display()
            .to_string()
    });
    let audit_sink = Arc::new(
        JsonlFileAuditSink::open(&audit_log_path).expect("failed to open audit log for writing"),
    );

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

    let gateway =
        ToolGateway::with_audit_sink_and_approvals(registry, policy, audit_sink.clone(), approvals);

    let ai_backend: Box<dyn CompletionBackend> = match std::env::var("RALLEH_AI_BASE_URL") {
        Ok(base_url) => {
            let provider = std::env::var("RALLEH_AI_PROVIDER")
                .unwrap_or_else(|_| "openai".to_string())
                .to_ascii_lowercase();
            let backend_name =
                std::env::var("RALLEH_AI_BACKEND_NAME").unwrap_or_else(|_| provider.clone());
            match provider.as_str() {
                "anthropic" => {
                    let model = std::env::var("RALLEH_AI_MODEL")
                        .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());
                    let api_key = std::env::var("RALLEH_AI_API_KEY").unwrap_or_else(|_| {
                        panic!("RALLEH_AI_PROVIDER=anthropic requires RALLEH_AI_API_KEY")
                    });
                    tracing::info!(base_url = %base_url, model = %model, "configuring AnthropicMessagesBackend");
                    Box::new(AnthropicMessagesBackend::new(
                        backend_name,
                        base_url,
                        model,
                        api_key,
                    ))
                }
                _ => {
                    let model = std::env::var("RALLEH_AI_MODEL")
                        .unwrap_or_else(|_| "gpt-4o-mini".to_string());
                    let api_key = std::env::var("RALLEH_AI_API_KEY").ok();
                    tracing::info!(base_url = %base_url, model = %model, "configuring HttpCompletionBackend");
                    Box::new(HttpCompletionBackend::new(
                        backend_name,
                        base_url,
                        model,
                        api_key,
                    ))
                }
            }
        }
        Err(_) => {
            tracing::info!("RALLEH_AI_BASE_URL not set; falling back to local EchoBackend");
            Box::new(EchoBackend)
        }
    };
    let ai_router = AiRouter::new(ai_backend);

    let auth = TokenAuthenticator::from_env().unwrap_or_else(|e| {
        panic!("failed to load API token authenticator: {e}");
    });
    if auth.is_some() {
        tracing::info!("API token auth enabled (RALLEH_API_TOKENS[_FILE])");
    } else {
        tracing::warn!(
            "API token auth disabled — tenant/actor claims in request bodies are unauthenticated (threat model T1)"
        );
    }

    let state = AppState::with_auth(gateway, ai_router, auth);
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
