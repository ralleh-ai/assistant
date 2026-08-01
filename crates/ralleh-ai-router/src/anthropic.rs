//! Anthropic Messages API completion backend — a second real wire format
//! alongside `HttpCompletionBackend`, proving `CompletionBackend` is not
//! tied to the OpenAI `/chat/completions` shape (ADR-008 follow-up /
//! NEXT_STEPS "second AI backend").

use async_trait::async_trait;

use crate::backend::CompletionBackend;
use crate::request::{CompletionRequest, CompletionResponse};

/// Speaks Anthropic's native `POST /v1/messages` API (`x-api-key` +
/// `anthropic-version` headers, `content` blocks in the response).
pub struct AnthropicMessagesBackend {
    name: String,
    base_url: String,
    model: String,
    api_key: String,
    max_tokens: u32,
    client: reqwest::Client,
}

impl AnthropicMessagesBackend {
    /// `base_url` is the API root (e.g. `https://api.anthropic.com`); this
    /// backend appends `/v1/messages`. An API key is required — Anthropic
    /// does not accept unauthenticated calls the way some local OpenAI-
    /// compatible servers do.
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            model: model.into(),
            api_key: api_key.into(),
            max_tokens: 1024,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("failed to build reqwest client"),
        }
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}

#[derive(serde::Serialize)]
struct MessagesRequestBody<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<AnthropicMessage<'a>>,
}

#[derive(serde::Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(serde::Deserialize)]
struct MessagesResponseBody {
    content: Vec<ContentBlock>,
}

#[derive(serde::Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

#[async_trait]
impl CompletionBackend for AnthropicMessagesBackend {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, String> {
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let body = MessagesRequestBody {
            model: request.model_hint.as_deref().unwrap_or(&self.model),
            max_tokens: self.max_tokens,
            messages: vec![AnthropicMessage {
                role: "user",
                content: &request.prompt,
            }],
        };

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
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

        let parsed: MessagesResponseBody = response
            .json()
            .await
            .map_err(|e| format!("failed to parse Anthropic response as JSON: {e}"))?;

        let text = parsed
            .content
            .into_iter()
            .find(|b| b.block_type == "text")
            .and_then(|b| b.text)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| "Anthropic response had no text content blocks".to_string())?;

        Ok(CompletionResponse {
            backend: self.name.clone(),
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn anthropic_backend_returns_text_block_on_success() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .and(wiremock::matchers::header("x-api-key", "test-key"))
            .and(wiremock::matchers::header("anthropic-version", "2023-06-01"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({
                    "content": [
                        { "type": "text", "text": "bonjour" }
                    ]
                }),
            ))
            .mount(&server)
            .await;

        let backend =
            AnthropicMessagesBackend::new("anthropic-test", server.uri(), "claude-test", "test-key");
        let response = backend.complete(&test_request("hi", None)).await.unwrap();
        assert_eq!(response.text, "bonjour");
        assert_eq!(response.backend, "anthropic-test");
    }

    #[tokio::test]
    async fn anthropic_backend_uses_model_hint() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "model": "claude-hint"
            })))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({
                    "content": [ { "type": "text", "text": "ok" } ]
                }),
            ))
            .mount(&server)
            .await;

        let backend =
            AnthropicMessagesBackend::new("anthropic-test", server.uri(), "default", "k");
        let response = backend
            .complete(&test_request("hi", Some("claude-hint")))
            .await
            .unwrap();
        assert_eq!(response.text, "ok");
    }

    #[tokio::test]
    async fn anthropic_backend_reports_http_error_without_panic() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;

        let backend = AnthropicMessagesBackend::new("anthropic-test", server.uri(), "m", "k");
        let err = backend.complete(&test_request("hi", None)).await.unwrap_err();
        assert!(err.contains("401"));
    }

    #[tokio::test]
    async fn anthropic_backend_reports_missing_text_block() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "content": [ { "type": "tool_use" } ] }),
            ))
            .mount(&server)
            .await;

        let backend = AnthropicMessagesBackend::new("anthropic-test", server.uri(), "m", "k");
        let err = backend.complete(&test_request("hi", None)).await.unwrap_err();
        assert!(err.contains("no text content"));
    }
}
