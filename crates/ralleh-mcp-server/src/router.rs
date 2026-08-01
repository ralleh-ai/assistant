use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::post;
use axum::Router;
use serde::{Deserialize, Serialize};

use ralleh_ai_router::CompletionOutcome;
use ralleh_tool_gateway::ToolCallOutcome;

use crate::state::AppState;

/// Request body for `POST /v1/tools/dispatch`.
#[derive(Debug, Deserialize)]
pub struct DispatchRequest {
    pub tenant_id: String,
    pub device_id: String,
    pub actor_id: String,
    pub capability: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// Response body. Deliberately mirrors `GatewayEvent` rather than exposing
/// it directly, so the wire contract can evolve independently of the
/// internal audit record shape.
#[derive(Debug, Serialize)]
pub struct DispatchResponse {
    pub capability: String,
    pub outcome: String,
    pub detail: serde_json::Value,
}

/// Builds the Axum router for the MCP surface. Currently exposes tool
/// dispatch, approval resolve, AI completion routing, and a health check.
/// Routes for approvals match DEVELOPMENT.md §14.1
/// (`POST /v1/approvals/:id/approve|reject`).
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", axum::routing::get(healthz))
        .route("/v1/tools/dispatch", post(dispatch_tool))
        .route("/v1/approvals/:id/approve", post(approve_request))
        .route("/v1/approvals/:id/reject", post(reject_request))
        .route("/v1/completions", post(complete))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

fn outcome_to_response(
    capability: String,
    outcome: &ToolCallOutcome,
    approval_request_id: Option<uuid::Uuid>,
) -> (StatusCode, Json<DispatchResponse>) {
    let (status, outcome_label, mut detail) = match outcome {
        ToolCallOutcome::Succeeded { result_summary } => (
            StatusCode::OK,
            "succeeded",
            serde_json::json!({ "result_summary": result_summary }),
        ),
        ToolCallOutcome::Denied => (
            StatusCode::FORBIDDEN,
            "denied",
            serde_json::json!({ "reason": "policy denied this capability" }),
        ),
        ToolCallOutcome::ApprovalRequired => (
            StatusCode::ACCEPTED,
            "approval_required",
            serde_json::json!({ "reason": "this action requires human approval" }),
        ),
        ToolCallOutcome::ApprovalRejected => (
            StatusCode::FORBIDDEN,
            "approval_rejected",
            serde_json::json!({ "reason": "the pending approval was rejected" }),
        ),
        ToolCallOutcome::Failed { error } => (
            StatusCode::BAD_GATEWAY,
            "failed",
            serde_json::json!({ "error": error }),
        ),
        ToolCallOutcome::NoHandlerRegistered => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "no_handler_registered",
            serde_json::json!({ "reason": "capability is defined but has no handler wired up" }),
        ),
        ToolCallOutcome::UnknownCapability => (
            StatusCode::NOT_FOUND,
            "unknown_capability",
            serde_json::json!({ "reason": "no such capability is registered" }),
        ),
    };

    if let Some(id) = approval_request_id {
        if let Some(obj) = detail.as_object_mut() {
            obj.insert(
                "approval_request_id".to_string(),
                serde_json::json!(id.to_string()),
            );
        }
    }

    (
        status,
        Json(DispatchResponse {
            capability,
            outcome: outcome_label.to_string(),
            detail,
        }),
    )
}

async fn dispatch_tool(
    State(state): State<AppState>,
    Json(req): Json<DispatchRequest>,
) -> impl IntoResponse {
    let event = state.gateway.dispatch(
        req.tenant_id,
        req.device_id,
        req.actor_id,
        req.capability.clone(),
        req.arguments,
    );

    outcome_to_response(event.capability, &event.outcome, event.approval_request_id)
}

/// Body for approve/reject — identifies who is deciding, scoped to a tenant
/// so cross-tenant approval attempts are rejected by the gateway.
#[derive(Debug, Deserialize)]
pub struct ApprovalDecisionRequest {
    pub tenant_id: String,
    pub actor_id: String,
}

async fn approve_request(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Json(req): Json<ApprovalDecisionRequest>,
) -> impl IntoResponse {
    match state.gateway.approve(id, &req.tenant_id, &req.actor_id) {
        Ok(event) => {
            outcome_to_response(event.capability, &event.outcome, event.approval_request_id)
                .into_response()
        }
        Err(err) => approval_error_response(err).into_response(),
    }
}

async fn reject_request(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Json(req): Json<ApprovalDecisionRequest>,
) -> impl IntoResponse {
    match state.gateway.reject(id, &req.tenant_id, &req.actor_id) {
        Ok(event) => {
            outcome_to_response(event.capability, &event.outcome, event.approval_request_id)
                .into_response()
        }
        Err(err) => approval_error_response(err).into_response(),
    }
}

fn approval_error_response(err: ralleh_tool_gateway::ApprovalError) -> impl IntoResponse {
    use ralleh_tool_gateway::ApprovalError;
    let (status, code) = match &err {
        ApprovalError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        ApprovalError::NotPending(_, _) => (StatusCode::CONFLICT, "not_pending"),
        ApprovalError::TenantMismatch => (StatusCode::FORBIDDEN, "tenant_mismatch"),
    };
    (
        status,
        Json(serde_json::json!({
            "error": code,
            "message": err.to_string(),
        })),
    )
}

/// Request body for `POST /v1/completions`.
#[derive(Debug, Deserialize)]
pub struct CompletionHttpRequest {
    pub tenant_id: String,
    pub device_id: String,
    pub actor_id: String,
    #[serde(default)]
    pub model_hint: Option<String>,
    pub prompt: String,
}

#[derive(Debug, Serialize)]
pub struct CompletionHttpResponse {
    pub outcome: String,
    pub detail: serde_json::Value,
}

async fn complete(
    State(state): State<AppState>,
    Json(req): Json<CompletionHttpRequest>,
) -> impl IntoResponse {
    let request = ralleh_ai_router::CompletionRequest {
        tenant_id: req.tenant_id,
        device_id: req.device_id,
        actor_id: req.actor_id,
        model_hint: req.model_hint,
        prompt: req.prompt,
    };

    let outcome = state.ai_router.route(&request).await;

    let (status, outcome_label, detail) = match outcome {
        CompletionOutcome::Succeeded(response) => (
            StatusCode::OK,
            "succeeded",
            serde_json::json!({ "backend": response.backend, "text": response.text }),
        ),
        CompletionOutcome::Failed { backend, error } => (
            StatusCode::BAD_GATEWAY,
            "failed",
            serde_json::json!({ "backend": backend, "error": error }),
        ),
        CompletionOutcome::Denied => (
            StatusCode::FORBIDDEN,
            "denied",
            serde_json::json!({ "reason": "policy denied this completion request" }),
        ),
        CompletionOutcome::ApprovalRequired => (
            StatusCode::ACCEPTED,
            "approval_required",
            serde_json::json!({ "reason": "this completion request requires human approval" }),
        ),
        CompletionOutcome::NoBackendConfigured => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "no_backend_configured",
            serde_json::json!({ "reason": "no completion backend is configured on this server" }),
        ),
    };

    (
        status,
        Json(CompletionHttpResponse {
            outcome: outcome_label.to_string(),
            detail,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use ralleh_policy_core::{PolicyEngine, PolicyRule, RuleEffect};
    use ralleh_tool_gateway::{ToolDefinition, ToolGateway, ToolHandler, ToolInvocation, ToolResult, ToolRegistry};
    use tower::ServiceExt;

    struct EchoHandler;
    impl ToolHandler for EchoHandler {
        fn invoke(&self, invocation: &ToolInvocation) -> Result<ToolResult, String> {
            Ok(ToolResult {
                summary: format!("echo:{}", invocation.capability),
                data: invocation.arguments.clone(),
            })
        }
    }

    fn test_ai_router() -> ralleh_ai_router::AiRouter {
        ralleh_ai_router::AiRouter::new(Box::new(ralleh_ai_router::EchoBackend))
    }

    fn test_state() -> AppState {
        let mut registry = ToolRegistry::new();
        registry.register(
            ToolDefinition {
                capability: "tool.search".to_string(),
                description: "test search".to_string(),
                default_sensitivity: "public".to_string(),
            },
            Box::new(EchoHandler),
        );

        let allow_rule = PolicyRule {
            id: "allow-search".to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.search".to_string()),
            sensitivity: None,
            effect: RuleEffect::Allow,
            reason: "test".to_string(),
        };

        let gateway = ToolGateway::new(registry, PolicyEngine::new(vec![allow_rule]));
        AppState::new(gateway, test_ai_router())
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn completions_endpoint_returns_200_on_success() {
        let app = build_router(test_state());
        let payload = serde_json::json!({
            "tenant_id": "t1",
            "device_id": "d1",
            "actor_id": "u1",
            "prompt": "hello world"
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["outcome"], "succeeded");
        assert_eq!(body["detail"]["text"], "echo: hello world");
    }

    #[tokio::test]
    async fn completions_endpoint_denies_when_policy_denies() {
        let denying_router = AppState::new(
            {
                let mut registry = ToolRegistry::new();
                registry.register(
                    ToolDefinition {
                        capability: "tool.search".to_string(),
                        description: "test search".to_string(),
                        default_sensitivity: "public".to_string(),
                    },
                    Box::new(EchoHandler),
                );
                ToolGateway::new(registry, PolicyEngine::empty())
            },
            ralleh_ai_router::AiRouter::with_policy(
                Box::new(ralleh_ai_router::EchoBackend),
                PolicyEngine::empty(),
            ),
        );
        let app = build_router(denying_router);
        let payload = serde_json::json!({
            "tenant_id": "t1",
            "device_id": "d1",
            "actor_id": "u1",
            "prompt": "hello world"
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = body_json(response).await;
        assert_eq!(body["outcome"], "denied");
    }

    #[tokio::test]
    async fn healthz_reports_ok() {
        let app = build_router(test_state());
        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn dispatch_allowed_capability_returns_200_with_result() {
        let app = build_router(test_state());
        let payload = serde_json::json!({
            "tenant_id": "t1",
            "device_id": "d1",
            "actor_id": "u1",
            "capability": "tool.search",
            "arguments": {"query": "rust"}
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tools/dispatch")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["outcome"], "succeeded");
    }

    #[tokio::test]
    async fn dispatch_unknown_capability_returns_404() {
        let app = build_router(test_state());
        let payload = serde_json::json!({
            "tenant_id": "t1",
            "device_id": "d1",
            "actor_id": "u1",
            "capability": "tool.nonexistent",
            "arguments": {}
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tools/dispatch")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = body_json(response).await;
        assert_eq!(body["outcome"], "unknown_capability");
    }

    #[tokio::test]
    async fn dispatch_denied_capability_returns_403() {
        let payload = serde_json::json!({
            "tenant_id": "t1",
            "device_id": "d1",
            "actor_id": "u1",
            "capability": "tool.search",
            "arguments": {}
        });
        // capability with no matching allow rule for a different tenant id
        // that isn't scoped -- use a capability that IS registered but where
        // we swap in a deny-only engine via a second state.
        let mut registry = ToolRegistry::new();
        registry.register(
            ToolDefinition {
                capability: "tool.search".to_string(),
                description: "test search".to_string(),
                default_sensitivity: "public".to_string(),
            },
            Box::new(EchoHandler),
        );
        let gateway = ToolGateway::new(registry, PolicyEngine::empty());
        let app = build_router(AppState::new(gateway, test_ai_router()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tools/dispatch")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = body_json(response).await;
        assert_eq!(body["outcome"], "denied");
    }

    #[tokio::test]
    async fn dispatch_approval_required_returns_202() {
        let mut registry = ToolRegistry::new();
        registry.register(
            ToolDefinition {
                capability: "tool.finance.transfer".to_string(),
                description: "test".to_string(),
                default_sensitivity: "confidential".to_string(),
            },
            Box::new(EchoHandler),
        );
        let approval_rule = PolicyRule {
            id: "approval-finance".to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.finance".to_string()),
            sensitivity: None,
            effect: RuleEffect::RequireApproval,
            reason: "test".to_string(),
        };
        let gateway = ToolGateway::new(registry, PolicyEngine::new(vec![approval_rule]));
        let app = build_router(AppState::new(gateway, test_ai_router()));

        let payload = serde_json::json!({
            "tenant_id": "t1",
            "device_id": "d1",
            "actor_id": "u1",
            "capability": "tool.finance.transfer",
            "arguments": {}
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tools/dispatch")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = body_json(response).await;
        assert_eq!(body["outcome"], "approval_required");
        assert!(body["detail"]["approval_request_id"].as_str().is_some());
    }

    #[tokio::test]
    async fn approve_then_executes_parked_write_end_to_end() {
        use ralleh_tool_gateway::FsWriteTextHandler;

        let sandbox = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(
            ToolDefinition {
                capability: "tool.fs.write_text".to_string(),
                description: "test write".to_string(),
                default_sensitivity: "internal".to_string(),
            },
            Box::new(FsWriteTextHandler::new(sandbox.path()).unwrap()),
        );
        let approval_rule = PolicyRule {
            id: "approval-fs-write".to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.fs.write_text".to_string()),
            sensitivity: None,
            effect: RuleEffect::RequireApproval,
            reason: "test".to_string(),
        };
        let gateway = ToolGateway::new(registry, PolicyEngine::new(vec![approval_rule]));
        let app = build_router(AppState::new(gateway, test_ai_router()));

        let dispatch_payload = serde_json::json!({
            "tenant_id": "t1",
            "device_id": "d1",
            "actor_id": "u1",
            "capability": "tool.fs.write_text",
            "arguments": {"path": "note.txt", "contents": "approved write"}
        });
        let parked = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tools/dispatch")
                    .header("content-type", "application/json")
                    .body(Body::from(dispatch_payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(parked.status(), StatusCode::ACCEPTED);
        let parked_body = body_json(parked).await;
        let approval_id = parked_body["detail"]["approval_request_id"]
            .as_str()
            .expect("approval id");
        assert!(!sandbox.path().join("note.txt").exists());

        let approve_payload = serde_json::json!({
            "tenant_id": "t1",
            "actor_id": "approver-1"
        });
        let approved = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/approvals/{approval_id}/approve"))
                    .header("content-type", "application/json")
                    .body(Body::from(approve_payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(approved.status(), StatusCode::OK);
        let approved_body = body_json(approved).await;
        assert_eq!(approved_body["outcome"], "succeeded");
        assert_eq!(
            std::fs::read_to_string(sandbox.path().join("note.txt")).unwrap(),
            "approved write"
        );
    }
    #[tokio::test]
    async fn write_text_capability_is_gated_dispatched_and_audited_end_to_end() {
        use ralleh_audit_store::JsonlFileAuditSink;
        use ralleh_tool_gateway::FsWriteTextHandler;
        use std::sync::Arc;

        let sandbox = tempfile::tempdir().unwrap();
        let audit_dir = tempfile::tempdir().unwrap();
        let audit_path = audit_dir.path().join("audit.jsonl");

        let mut registry = ToolRegistry::new();
        registry.register(
            ToolDefinition {
                capability: "tool.fs.write_text".to_string(),
                description: "test write".to_string(),
                default_sensitivity: "internal".to_string(),
            },
            Box::new(FsWriteTextHandler::new(sandbox.path()).unwrap()),
        );

        // Mirrors production wiring: writes require approval, not a bare
        // allow.
        let approval_rule = PolicyRule {
            id: "approval-fs-write".to_string(),
            tenant_id: None,
            device_id: None,
            actor_id: None,
            capability_prefix: Some("tool.fs.write_text".to_string()),
            sensitivity: None,
            effect: RuleEffect::RequireApproval,
            reason: "test".to_string(),
        };

        let audit_sink = Arc::new(JsonlFileAuditSink::open(&audit_path).unwrap());
        let gateway = ToolGateway::with_audit_sink(
            registry,
            PolicyEngine::new(vec![approval_rule]),
            audit_sink,
        );
        let app = build_router(AppState::new(gateway, test_ai_router()));

        let payload = serde_json::json!({
            "tenant_id": "t1",
            "device_id": "d1",
            "actor_id": "u1",
            "capability": "tool.fs.write_text",
            "arguments": {"path": "note.txt", "contents": "hello from e2e test"}
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tools/dispatch")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Policy requires approval, so the handler must never have run:
        // the file must not exist, and the response must reflect that.
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = body_json(response).await;
        assert_eq!(body["outcome"], "approval_required");
        assert!(body["detail"]["approval_request_id"].as_str().is_some());
        assert!(!sandbox.path().join("note.txt").exists());

        // But the attempt itself must still be durably audited -- an
        // approval-required outcome is exactly as auditable as an allowed
        // or denied one.
        let audit_contents = std::fs::read_to_string(&audit_path).unwrap();
        let lines: Vec<&str> = audit_contents.lines().collect();
        assert_eq!(lines.len(), 1);
        let record: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(record["kind"], "tool_dispatch");
        assert_eq!(record["capability"], "tool.fs.write_text");
        assert_eq!(record["outcome"], "ApprovalRequired");
        assert!(record["approval_request_id"].as_str().is_some());
    }
}
