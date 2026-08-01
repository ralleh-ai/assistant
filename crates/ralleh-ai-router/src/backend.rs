use async_trait::async_trait;

use crate::request::{CompletionRequest, CompletionResponse};

/// Abstraction over a concrete AI backend (a specific model provider,
/// local inference engine, etc.). The router never talks to a provider
/// directly -- it only ever calls through this trait, the same way
/// `ralleh-tool-gateway` only ever calls tools through `ToolHandler`.
///
/// This is deliberately `async_trait`-based rather than sync: real
/// backends are network calls or local inference that may block, and the
/// router itself is expected to run inside a Tokio runtime (it shares that
/// property with `ralleh-mcp-server`).
#[async_trait]
pub trait CompletionBackend: Send + Sync {
    /// Stable identifier for this backend, used in routing decisions and
    /// audit records (e.g. "local-echo", "openai-gpt", "anthropic-claude").
    fn name(&self) -> &str;

    /// Perform the actual completion call. Errors are returned as `Err`
    /// with a human-readable message -- the router translates these into
    /// `CompletionOutcome::Failed` rather than letting them propagate as
    /// panics or opaque error types, mirroring how `ToolHandler::invoke`
    /// works in `ralleh-tool-gateway`.
    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, String>;
}

/// A trivial backend for tests and local development: it does not call any
/// real model, it just echoes the prompt back with a fixed prefix. This is
/// the AI-router equivalent of `ralleh-tool-gateway::handler::EchoHandler`
/// -- proves the routing plumbing without requiring real provider
/// credentials or network access.
pub struct EchoBackend;

#[async_trait]
impl CompletionBackend for EchoBackend {
    fn name(&self) -> &str {
        "local-echo"
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, String> {
        Ok(CompletionResponse {
            backend: self.name().to_string(),
            text: format!("echo: {}", request.prompt),
        })
    }
}

/// A real (non-mocked) backend: speaks the OpenAI-compatible
/// `/chat/completions` wire format over HTTP. This is deliberately the
/// lowest common denominator across providers -- OpenAI itself,
/// self-hosted `vllm`/`ollama`/`llama.cpp` servers, and most third-party
/// gateways all speak this shape, so this one backend covers a wide swath
/// of real deployment targets without provider-specific code.
///
/// This is the ai-router equivalent of `FsReadTextHandler`/
/// `FsWriteTextHandler` in `ralleh-tool-gateway`: the first backend in this
/// crate that performs a real network operation rather than being a test
/// double, proving the router's dispatch path end-to-end against
/// something that can actually fail in realistic ways (timeouts, HTTP
/// errors, malformed responses).
pub struct HttpCompletionBackend {
    name: String,
    base_url: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl HttpCompletionBackend {
    /// `base_url` should point at the API root (e.g.
    /// `https://api.openai.com/v1` or `http://localhost:11434/v1`) -- this
    /// backend appends `/chat/completions` itself. `name` is the stable
    /// identifier surfaced in `CompletionResponse::backend` and audit
    /// records; callers should pick something that distinguishes this
    /// backend instance from others in a multi-backend routing setup (e.g.
    /// "openai-gpt-4o" vs. "local-ollama-llama3").
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            model: model.into(),
            api_key,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("failed to build reqwest client"),
        }
    }
}

#[derive(serde::Serialize)]
struct ChatCompletionRequestBody<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
}

#[derive(serde::Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(serde::Deserialize)]
struct ChatCompletionResponseBody {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(serde::Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionMessage,
}

#[derive(serde::Deserialize)]
struct ChatCompletionMessage {
    content: Option<String>,
}

#[async_trait]
impl CompletionBackend for HttpCompletionBackend {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, String> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let body = ChatCompletionRequestBody {
            model: request.model_hint.as_deref().unwrap_or(&self.model),
            messages: vec![ChatMessage {
                role: "user",
                content: &request.prompt,
            }],
        };

        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let response = req
            .send()
            .await
            .map_err(|e| format!("request to {url} failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read error body>".to_string());
            return Err(format!("backend returned HTTP {status}: {body_text}"));
        }

        let parsed: ChatCompletionResponseBody = response
            .json()
            .await
            .map_err(|e| format!("failed to parse backend response as JSON: {e}"))?;

        let text = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .ok_or_else(|| "backend response had no completion choices".to_string())?;

        Ok(CompletionResponse {
            backend: self.name.clone(),
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_backend_prefixes_prompt() {
        let backend = EchoBackend;
        let request = CompletionRequest {
            tenant_id: "t1".to_string(),
            device_id: "d1".to_string(),
            actor_id: "u1".to_string(),
            model_hint: None,
            prompt: "hello".to_string(),
        };
        let response = backend.complete(&request).await.unwrap();
        assert_eq!(response.text, "echo: hello");
        assert_eq!(response.backend, "local-echo");
    }

    fn test_request(prompt: &str, model_hint: Option<&str>) -> CompletionRequest {
        CompletionRequest {
            tenant_id: "t1".to_string(),
            device_id: "d1".to_string(),
            actor_id: "u1".to_string(),
            model_hint: model_hint.map(|s| s.to_string()),
            prompt: prompt.to_string(),
        }
    }

    #[tokio::test]
    async fn http_backend_returns_completion_text_on_success() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({
                    "choices": [
                        { "message": { "role": "assistant", "content": "hello back" } }
                    ]
                }),
            ))
            .mount(&server)
            .await;

        let backend = HttpCompletionBackend::new("test-backend", server.uri(), "test-model", None);
        let response = backend.complete(&test_request("hi", None)).await.unwrap();
        assert_eq!(response.text, "hello back");
        assert_eq!(response.backend, "test-backend");
    }

    #[tokio::test]
    async fn http_backend_uses_model_hint_over_default_model() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "model": "hinted-model"
            })))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({
                    "choices": [ { "message": { "role": "assistant", "content": "ok" } } ]
                }),
            ))
            .mount(&server)
            .await;

        let backend =
            HttpCompletionBackend::new("test-backend", server.uri(), "default-model", None);
        let response = backend
            .complete(&test_request("hi", Some("hinted-model")))
            .await
            .unwrap();
        assert_eq!(response.text, "ok");
    }

    #[tokio::test]
    async fn http_backend_sends_bearer_auth_when_api_key_configured() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer secret-key",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({
                    "choices": [ { "message": { "role": "assistant", "content": "authed" } } ]
                }),
            ))
            .mount(&server)
            .await;

        let backend = HttpCompletionBackend::new(
            "test-backend",
            server.uri(),
            "test-model",
            Some("secret-key".to_string()),
        );
        let response = backend.complete(&test_request("hi", None)).await.unwrap();
        assert_eq!(response.text, "authed");
    }

    #[tokio::test]
    async fn http_backend_reports_non_success_status_as_failed_not_panic() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(
                wiremock::ResponseTemplate::new(500).set_body_string("internal error upstream"),
            )
            .mount(&server)
            .await;

        let backend = HttpCompletionBackend::new("test-backend", server.uri(), "test-model", None);
        let err = backend.complete(&test_request("hi", None)).await.unwrap_err();
        assert!(err.contains("500"));
        assert!(err.contains("internal error upstream"));
    }

    #[tokio::test]
    async fn http_backend_reports_malformed_json_as_failed_not_panic() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let backend = HttpCompletionBackend::new("test-backend", server.uri(), "test-model", None);
        let err = backend.complete(&test_request("hi", None)).await.unwrap_err();
        assert!(err.contains("failed to parse"));
    }

    #[tokio::test]
    async fn http_backend_reports_empty_choices_as_failed_not_panic() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "choices": [] })),
            )
            .mount(&server)
            .await;

        let backend = HttpCompletionBackend::new("test-backend", server.uri(), "test-model", None);
        let err = backend.complete(&test_request("hi", None)).await.unwrap_err();
        assert!(err.contains("no completion choices"));
    }
}
