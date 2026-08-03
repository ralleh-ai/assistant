use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use ralleh_ai_router::{
    AiRouter, AnthropicMessagesBackend, CompletionBackend, EchoBackend, HttpCompletionBackend,
};
use ralleh_audit_store::JsonlFileAuditSink;
use ralleh_mcp_server::{
    build_router, resolve_config_path, AppState, ServerConfig, TokenAuthenticator,
};
use ralleh_tool_gateway::{ApprovalStore, ToolGateway};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Reject an unbounded request body: dispatch/completion payloads are small
/// JSON, so anything past this is either a bug or an abuse attempt.
const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024; // 1 MiB
/// Upper bound on wall-clock time any single request may occupy a worker.
const REQUEST_TIMEOUT_SECS: u64 = 30;
/// Ceiling on in-flight requests so a burst can't exhaust memory/FDs.
const MAX_CONCURRENT_REQUESTS: usize = 256;

/// Binary entrypoint for the Rust MCP/tool-gateway HTTP surface.
///
/// Registry + policy rules are loaded from a declarative config file
/// (`config/default.toml` by default, or `RALLEH_CONFIG`), matching
/// DEVELOPMENT.md §8.3. Handler implementations stay in code; the config
/// only chooses which known handlers to wire and under which capability
/// ids / policy effects.
///
/// Bootstrap is fallible-by-`?`: a misconfiguration exits with a non-zero
/// status and a single diagnostic line rather than an unwinding panic +
/// backtrace, which is the behavior an init system / container runtime
/// expects.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let addr = std::env::var("RALLEH_MCP_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_string());

    let config_path =
        resolve_config_path().map_err(|e| format!("failed to resolve server config: {e}"))?;
    tracing::info!(path = %config_path.display(), "loading server config");
    let config = ServerConfig::load_from_path(&config_path).map_err(|e| {
        format!(
            "failed to load server config from {}: {e}",
            config_path.display()
        )
    })?;

    let sandbox_dir = config
        .resolve_sandbox_root()
        .map_err(|e| format!("failed to resolve sandbox root: {e}"))?;
    let registry = config
        .build_registry(&sandbox_dir)
        .map_err(|e| format!("failed to build tool registry from config: {e}"))?;
    let policy = config.build_policy_engine();

    let audit_log_path = std::env::var("RALLEH_AUDIT_LOG_PATH").unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("ralleh-audit.jsonl")
            .display()
            .to_string()
    });
    let audit_sink = Arc::new(
        JsonlFileAuditSink::open(&audit_log_path)
            .map_err(|e| format!("failed to open audit log {audit_log_path} for writing: {e}"))?,
    );

    let approval_store_path = std::env::var("RALLEH_APPROVAL_STORE_PATH").unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("ralleh-approvals.json")
            .display()
            .to_string()
    });
    let approvals = Arc::new(
        ApprovalStore::open(&approval_store_path)
            .map_err(|e| format!("failed to open approval store {approval_store_path}: {e}"))?,
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
                    let api_key = std::env::var("RALLEH_AI_API_KEY").map_err(|_| {
                        "RALLEH_AI_PROVIDER=anthropic requires RALLEH_AI_API_KEY".to_string()
                    })?;
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

    let auth = TokenAuthenticator::from_env()
        .map_err(|e| format!("failed to load API token auth: {e}"))?;
    if auth.is_some() {
        tracing::info!("API token auth enabled (RALLEH_API_TOKENS[_FILE])");
    } else if env_flag("RALLEH_REQUIRE_AUTH") {
        // Fail closed: an operator explicitly demanded auth but none is
        // configured. Starting anyway would silently serve an unauthenticated
        // surface — exactly the outcome the flag exists to prevent.
        return Err(
            "RALLEH_REQUIRE_AUTH is set but no API tokens are configured \
             (set RALLEH_API_TOKENS or RALLEH_API_TOKENS_FILE)"
                .into(),
        );
    } else {
        tracing::warn!(
            "API token auth disabled — tenant/actor claims in request bodies are unauthenticated (threat model T1); set RALLEH_REQUIRE_AUTH=1 to forbid this in production"
        );
    }

    let state = AppState::with_auth(gateway, ai_router, auth);
    // Production hardening layers (tests exercise the bare router):
    //   * cap request bodies so a giant payload can't exhaust memory,
    //   * time-box each request so a slow/stuck handler frees its worker,
    //   * bound total in-flight requests as basic load-shedding,
    //   * emit per-request spans for observability.
    let app = build_router(state)
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(TimeoutLayer::new(Duration::from_secs(REQUEST_TIMEOUT_SECS)))
        .layer(ConcurrencyLimitLayer::new(MAX_CONCURRENT_REQUESTS))
        .layer(TraceLayer::new_for_http());

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
        .map_err(|e| format!("failed to bind {addr}: {e}"))?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| format!("mcp server exited unexpectedly: {e}"))?;

    tracing::info!("ralleh-mcp-server shut down cleanly");
    Ok(())
}

/// Truthy interpretation of a boolean env var (`1`/`true`/`yes`/`on`).
fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

/// Resolve when the process receives Ctrl-C or (on Unix) SIGTERM, so the
/// server drains in-flight requests before exiting instead of dropping them.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => tracing::warn!("failed to install SIGTERM handler: {e}"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received Ctrl-C, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}
