use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

use crate::request::{CompletionRequest, CompletionResponse};

/// Sink handed to `CompletionBackend::stream_complete`. Each item is
/// either the next chunk of completion text or a fatal error that
/// terminates the stream. Concatenating every `Ok` in the order it
/// arrives reproduces the full response — same invariant as
/// `CompletionStreamEvent::Chunk` in `crate::request`.
///
/// Dropping the sender closes the channel, which is how the router
/// distinguishes "stream ended successfully" from "stream ended
/// with an error" — a clean drop with no `Err` sent is `Done`, an
/// `Err(_)` sent (before or without drop) is `Failed`.
pub type StreamChunkSender = UnboundedSender<Result<String, String>>;

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

    /// Streaming variant. Push each chunk of completion text through `tx`
    /// as it becomes available; drop `tx` (i.e. return) to signal a clean
    /// end-of-stream, or send an `Err` to signal a fatal error (the router
    /// will translate that into `CompletionStreamEvent::Failed`).
    ///
    /// The default implementation calls `complete` and yields the whole
    /// response as one chunk, so backends that don't have real network
    /// streaming (e.g. `EchoBackend` in its raw form, or providers that
    /// only expose a non-streaming API) still work through
    /// `AiRouter::route_stream`. Override this method to plug into a real
    /// SSE / chunked-transfer stream and emit deltas as they arrive.
    async fn stream_complete(&self, request: &CompletionRequest, tx: StreamChunkSender) {
        match self.complete(request).await {
            Ok(response) => {
                let _ = tx.send(Ok(response.text));
            }
            Err(error) => {
                let _ = tx.send(Err(error));
            }
        }
    }
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

    /// Override the default one-shot streaming to yield word-by-word with
    /// a small pacing delay. This gives the presence UI (and any human
    /// testing against the echo backend) something visibly progressive to
    /// render even when no real network I/O is involved. Real backends
    /// don't need this pacing because their network latency naturally
    /// spaces chunks out.
    async fn stream_complete(&self, request: &CompletionRequest, tx: StreamChunkSender) {
        let full = format!("echo: {}", request.prompt);
        for piece in split_preserving_whitespace(&full) {
            if tx.send(Ok(piece)).is_err() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}

/// Word-by-word splitter that preserves every byte: the concatenation
/// of every returned piece equals the input exactly. This is the
/// invariant `AiRouter::route_stream` (and its tests) rely on to
/// reconstruct the full response from chunks.
fn split_preserving_whitespace(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut in_whitespace = text.chars().next().is_some_and(|c| c.is_whitespace());
    for ch in text.chars() {
        let ch_is_ws = ch.is_whitespace();
        if ch_is_ws == in_whitespace {
            current.push(ch);
        } else {
            if !current.is_empty() {
                pieces.push(std::mem::take(&mut current));
            }
            current.push(ch);
            in_whitespace = ch_is_ws;
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
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
            // Timeout tuning notes:
            // - `connect_timeout` bounds TCP + TLS handshake only.
            //   A slow DNS or a black-holed IP shouldn't hang the
            //   router; 10s is generous but finite.
            // - `read_timeout` bounds per-read idle. Applies to
            //   both the non-streaming JSON body read and each SSE
            //   chunk pull. A stuck provider that stops emitting
            //   tokens for 60s is dead to us; we surface a
            //   `stream ... interrupted` error and the presence UI
            //   flips out of Speaking.
            // - We deliberately do NOT set the total-request
            //   `.timeout(...)` here: legitimate completions can
            //   run for many minutes, and the streaming path
            //   applies its own wall-clock budget via
            //   `tokio::time::timeout` in `stream_complete`. The
            //   non-streaming path relies on `read_timeout` and
            //   the caller's own budget (the router / test
            //   harness).
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .read_timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("failed to build reqwest client"),
        }
    }
}

/// Wall-clock budget for a single streaming completion. Real
/// completions rarely exceed a couple of minutes even on the
/// slowest hosted models; anything past this is either a stuck
/// provider or a runaway prompt, and we'd rather cut it here than
/// keep the router in-flight indefinitely.
const STREAM_WALL_CLOCK_BUDGET: std::time::Duration = std::time::Duration::from_secs(5 * 60);

#[derive(serde::Serialize)]
struct ChatCompletionRequestBody<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
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

// SSE streaming shape: each `data: {...}` frame decodes into this
// (roughly the OpenAI chat.completion.chunk envelope, plus the parts of
// it that self-hosted engines like vllm / ollama / llama.cpp emit).
#[derive(serde::Deserialize)]
struct ChatCompletionStreamChunk {
    #[serde(default)]
    choices: Vec<ChatCompletionStreamChoice>,
}

#[derive(serde::Deserialize)]
struct ChatCompletionStreamChoice {
    #[serde(default)]
    delta: ChatCompletionStreamDelta,
}

#[derive(serde::Deserialize, Default)]
struct ChatCompletionStreamDelta {
    #[serde(default)]
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
            stream: false,
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

    /// Real network streaming: sets `stream: true` on the OpenAI-shaped
    /// request body and parses the resulting Server-Sent Events stream,
    /// emitting each `delta.content` through `tx` as it arrives. This is
    /// the payoff for the whole `stream_complete` plumbing -- with a real
    /// backend, tokens arrive on the presence UI as the model generates
    /// them rather than all at once after generation finishes.
    ///
    /// Malformed frames are skipped rather than fatal: real providers
    /// occasionally emit heartbeats, empty `data: {}` keepalives, or
    /// chunks with `delta: {}` (role-only prefixes) that would otherwise
    /// abort the stream. We only treat transport-level failures and
    /// non-2xx HTTP responses as `Err`.
    async fn stream_complete(&self, request: &CompletionRequest, tx: StreamChunkSender) {
        // Whole-request wall-clock budget. See STREAM_WALL_CLOCK_BUDGET
        // for rationale. `tokio::time::timeout` cancels the awaited
        // future on expiry, which propagates as `response.chunk()`
        // never completing → we drop the response, tearing down the
        // TCP connection.
        let fut = self.stream_complete_inner(request, tx.clone());
        match tokio::time::timeout(STREAM_WALL_CLOCK_BUDGET, fut).await {
            Ok(()) => {}
            Err(_) => {
                let _ = tx.send(Err(format!(
                    "stream exceeded {}s wall-clock budget; aborting",
                    STREAM_WALL_CLOCK_BUDGET.as_secs()
                )));
            }
        }
    }
}

impl HttpCompletionBackend {
    async fn stream_complete_inner(&self, request: &CompletionRequest, tx: StreamChunkSender) {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let body = ChatCompletionRequestBody {
            model: request.model_hint.as_deref().unwrap_or(&self.model),
            messages: vec![ChatMessage {
                role: "user",
                content: &request.prompt,
            }],
            stream: true,
        };

        let mut req = self
            .client
            .post(&url)
            .header("accept", "text/event-stream")
            .json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let response = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(Err(format!("request to {url} failed: {e}")));
                return;
            }
        };

        let status = response.status();
        if !status.is_success() {
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read error body>".to_string());
            let _ = tx.send(Err(format!("backend returned HTTP {status}: {body_text}")));
            return;
        }

        let mut parser = SseParser::new();
        let mut response = response;
        loop {
            match response.chunk().await {
                Ok(Some(bytes)) => {
                    let frames = match parser.push_bytes(&bytes) {
                        Ok(f) => f,
                        Err(e) => {
                            // Parser aborted itself (buffer cap
                            // exceeded); tell the router this stream
                            // is dead and stop pulling from the
                            // socket. Dropping `response` closes the
                            // connection.
                            let _ = tx.send(Err(e));
                            return;
                        }
                    };
                    for frame in frames {
                        match frame {
                            SseFrame::Data(text) => {
                                if tx.send(Ok(text)).is_err() {
                                    return;
                                }
                            }
                            SseFrame::Done => return,
                        }
                    }
                }
                Ok(None) => {
                    // Flush any final buffered frame that wasn't terminated
                    // by a trailing blank line (some providers omit it).
                    for frame in parser.flush() {
                        if let SseFrame::Data(text) = frame {
                            if tx.send(Ok(text)).is_err() {
                                return;
                            }
                        }
                    }
                    return;
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("stream from {url} interrupted: {e}")));
                    return;
                }
            }
        }
    }
}

/// Semantic output of the SSE parser: either a decoded chunk of text
/// (`delta.content`), or a sentinel signaling the provider sent
/// `data: [DONE]`. Empty deltas, malformed JSON, comments, and
/// non-`data:` lines are dropped by the parser -- callers only see
/// these two variants.
#[derive(Debug, PartialEq)]
enum SseFrame {
    Data(String),
    Done,
}

/// Incremental Server-Sent Events parser for OpenAI-shaped chat
/// completion streams. Split off from `HttpCompletionBackend` so it
/// can be unit-tested without spinning up a mock HTTP server.
///
/// Semantics:
/// - Events are separated by `\n\n` (a blank line).
/// - Within an event, only `data:` lines are inspected; other fields
///   (`id:`, `event:`, comments starting with `:`) are ignored.
/// - `data: [DONE]` yields `SseFrame::Done`.
/// - `data: {...}` is parsed as `ChatCompletionStreamChunk`; if that
///   succeeds and there's a non-empty `delta.content`, it yields a
///   `SseFrame::Data(text)`. Otherwise (parse error, empty delta) the
///   frame is silently skipped.
///
/// ## Bounded buffer
///
/// The internal buffer is capped at [`SSE_MAX_BUFFER_BYTES`] to
/// keep a malicious or malfunctioning server from OOMing the shell
/// with an unterminated stream (imagine a proxy that sends 1 GiB
/// of `data:` bytes without a `\n\n` delimiter). Exceeding the cap
/// is a hard error the calling backend surfaces to the router,
/// which then closes out the stream as a normal HTTP-style failure.
struct SseParser {
    buffer: String,
}

/// Max bytes we'll hold buffered without seeing a frame delimiter.
/// A well-behaved OpenAI-compatible provider emits chunks of a few
/// hundred bytes and delimits every one; anything past ~1 MiB is
/// almost certainly either a hostile server or a broken proxy. This
/// is deliberately larger than any legitimate chunk so we don't
/// false-positive on latency-induced batching, but small enough to
/// bound worst-case memory pressure on a stalled connection.
const SSE_MAX_BUFFER_BYTES: usize = 1 << 20; // 1 MiB

impl SseParser {
    fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Ingest a byte chunk and return any complete frames that fell
    /// out. Bytes that aren't yet part of a complete event stay
    /// buffered until the next call (or `flush`). Returns an error
    /// when the pending buffer exceeds [`SSE_MAX_BUFFER_BYTES`] —
    /// see the type-level doc for the DoS-defence rationale.
    fn push_bytes(&mut self, bytes: &[u8]) -> Result<Vec<SseFrame>, String> {
        // Providers only ever emit UTF-8 here in practice, but split
        // multi-byte codepoints across TCP chunks are possible. We
        // tolerate them by using `from_utf8_lossy` (replacement chars
        // are fine for our purposes -- SSE framing is ASCII).
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        let mut out = Vec::new();
        while let Some(idx) = self.buffer.find("\n\n") {
            let event = self.buffer[..idx].to_string();
            self.buffer.drain(..idx + 2);
            if let Some(frame) = parse_sse_event(&event) {
                out.push(frame);
            }
        }
        if self.buffer.len() > SSE_MAX_BUFFER_BYTES {
            let len = self.buffer.len();
            self.buffer.clear();
            return Err(format!(
                "SSE frame exceeded {SSE_MAX_BUFFER_BYTES}-byte cap ({len} bytes buffered without a delimiter); aborting stream"
            ));
        }
        Ok(out)
    }

    /// Drain any trailing event still in the buffer that wasn't
    /// terminated by `\n\n`. Called when the underlying transport
    /// closes.
    fn flush(&mut self) -> Vec<SseFrame> {
        if self.buffer.trim().is_empty() {
            self.buffer.clear();
            return Vec::new();
        }
        let event = std::mem::take(&mut self.buffer);
        parse_sse_event(&event).into_iter().collect()
    }
}

fn parse_sse_event(event: &str) -> Option<SseFrame> {
    // Concatenate every `data:` line in the event, per the SSE spec
    // (multi-line data uses multiple `data:` fields joined by `\n`).
    let mut data = String::new();
    for line in event.lines() {
        let line = line.strip_prefix('\u{feff}').unwrap_or(line);
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    if data.is_empty() {
        return None;
    }
    if data.trim() == "[DONE]" {
        return Some(SseFrame::Done);
    }
    let chunk: ChatCompletionStreamChunk = serde_json::from_str(&data).ok()?;
    let content = chunk
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.delta.content)?;
    if content.is_empty() {
        None
    } else {
        Some(SseFrame::Data(content))
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
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "choices": [
                        { "message": { "role": "assistant", "content": "hello back" } }
                    ]
                })),
            )
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
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "choices": [ { "message": { "role": "assistant", "content": "ok" } } ]
                })),
            )
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
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "choices": [ { "message": { "role": "assistant", "content": "authed" } } ]
                })),
            )
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
        let err = backend
            .complete(&test_request("hi", None))
            .await
            .unwrap_err();
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
        let err = backend
            .complete(&test_request("hi", None))
            .await
            .unwrap_err();
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
        let err = backend
            .complete(&test_request("hi", None))
            .await
            .unwrap_err();
        assert!(err.contains("no completion choices"));
    }

    // ---- SSE parser unit tests ------------------------------------

    fn make_data_frame(content: &str) -> String {
        let payload = serde_json::json!({
            "choices": [{ "delta": { "content": content } }]
        });
        format!("data: {payload}\n\n")
    }

    #[test]
    fn sse_parser_yields_content_from_a_single_frame() {
        let mut parser = SseParser::new();
        let frames = parser
            .push_bytes(make_data_frame("Hello").as_bytes())
            .expect("well-formed frame parses");
        assert_eq!(frames, vec![SseFrame::Data("Hello".to_string())]);
    }

    #[test]
    fn sse_parser_buffers_across_partial_chunks() {
        // Real TCP transports split frames at arbitrary byte offsets.
        // Feed the same "Hello" frame in three pieces and confirm the
        // parser only emits it once, whole, after the terminator arrives.
        let mut parser = SseParser::new();
        let full = make_data_frame("Hello");
        let (a, rest) = full.split_at(5);
        let (b, c) = rest.split_at(10);
        assert!(parser.push_bytes(a.as_bytes()).unwrap().is_empty());
        assert!(parser.push_bytes(b.as_bytes()).unwrap().is_empty());
        let frames = parser.push_bytes(c.as_bytes()).unwrap();
        assert_eq!(frames, vec![SseFrame::Data("Hello".to_string())]);
    }

    #[test]
    fn sse_parser_yields_done_on_done_sentinel() {
        let mut parser = SseParser::new();
        let frames = parser.push_bytes(b"data: [DONE]\n\n").unwrap();
        assert_eq!(frames, vec![SseFrame::Done]);
    }

    #[test]
    fn sse_parser_skips_role_only_and_empty_delta_frames() {
        // OpenAI's first chunk is typically a role prefix with no content.
        // We must skip these silently rather than treating them as errors,
        // otherwise every stream would emit a stray empty chunk up front.
        let mut parser = SseParser::new();
        let role_only = r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#;
        let empty_content = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        let real = make_data_frame("Hi");
        let mut input = String::new();
        input.push_str(role_only);
        input.push_str("\n\n");
        input.push_str(empty_content);
        input.push_str("\n\n");
        input.push_str(&real);
        let frames = parser.push_bytes(input.as_bytes()).unwrap();
        assert_eq!(frames, vec![SseFrame::Data("Hi".to_string())]);
    }

    #[test]
    fn sse_parser_skips_malformed_json_rather_than_aborting() {
        // If a provider emits a garbled frame mid-stream we don't want
        // to lose the frames that come after it. The parser must skip
        // the bad frame and keep going.
        let mut parser = SseParser::new();
        let mut input = String::new();
        input.push_str("data: {not valid json\n\n");
        input.push_str(&make_data_frame("still here"));
        let frames = parser.push_bytes(input.as_bytes()).unwrap();
        assert_eq!(frames, vec![SseFrame::Data("still here".to_string())]);
    }

    #[test]
    fn sse_parser_ignores_comment_and_non_data_lines() {
        let mut parser = SseParser::new();
        let event = ": ping\nid: 42\nevent: message\ndata: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n";
        let frames = parser.push_bytes(event.as_bytes()).unwrap();
        assert_eq!(frames, vec![SseFrame::Data("ok".to_string())]);
    }

    #[test]
    fn sse_parser_aborts_when_a_single_frame_exceeds_the_buffer_cap() {
        // A malicious server (or broken proxy) could pin memory by
        // sending an unbounded stream of `data:` bytes without a
        // `\n\n` delimiter. Confirm the parser refuses to grow past
        // the cap and surfaces an error instead of silently OOMing
        // the caller. We reach the cap by streaming a partial frame
        // in chunks so the tail never contains the delimiter.
        let mut parser = SseParser::new();
        // 32 KiB chunks; enough of them to blow past 1 MiB.
        let chunk = vec![b'x'; 32 * 1024];
        let mut hit_cap = false;
        for _ in 0..64 {
            match parser.push_bytes(&chunk) {
                Ok(_) => continue,
                Err(msg) => {
                    assert!(
                        msg.contains("cap") || msg.contains("byte"),
                        "unexpected error text: {msg}"
                    );
                    hit_cap = true;
                    break;
                }
            }
        }
        assert!(
            hit_cap,
            "parser accepted >2 MiB without a delimiter — cap not enforced"
        );
    }

    #[test]
    fn sse_parser_recovers_after_hitting_the_cap() {
        // Post-abort the parser clears its buffer so a subsequent
        // (well-formed) chunk can still be parsed. The caller is
        // expected to have torn down the offending stream, but the
        // parser type itself must not become permanently poisoned.
        let mut parser = SseParser::new();
        let chunk = vec![b'x'; 32 * 1024];
        for _ in 0..64 {
            if parser.push_bytes(&chunk).is_err() {
                break;
            }
        }
        let frames = parser
            .push_bytes(make_data_frame("recovered").as_bytes())
            .expect("parser should have cleared its buffer");
        assert_eq!(frames, vec![SseFrame::Data("recovered".to_string())]);
    }

    // ---- HTTP streaming end-to-end -------------------------------

    fn sse_body(chunks: &[&str], terminate_with_done: bool) -> String {
        let mut out = String::new();
        for c in chunks {
            out.push_str(&make_data_frame(c));
        }
        if terminate_with_done {
            out.push_str("data: [DONE]\n\n");
        }
        out
    }

    #[tokio::test]
    async fn http_backend_streams_sse_deltas_in_order() {
        let server = wiremock::MockServer::start().await;
        let body = sse_body(&["Hello", " ", "world"], true);
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "stream": true
            })))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string(body)
                    .insert_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let backend = HttpCompletionBackend::new("test-backend", server.uri(), "test-model", None);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<String, String>>();
        backend.stream_complete(&test_request("hi", None), tx).await;
        let mut collected = String::new();
        while let Some(item) = rx.recv().await {
            collected.push_str(&item.unwrap());
        }
        assert_eq!(collected, "Hello world");
    }

    #[tokio::test]
    async fn http_backend_stream_reports_http_error_as_err_not_partial_data() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(429).set_body_string("rate limited"))
            .mount(&server)
            .await;

        let backend = HttpCompletionBackend::new("test-backend", server.uri(), "test-model", None);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<String, String>>();
        backend.stream_complete(&test_request("hi", None), tx).await;
        let first = rx.recv().await.expect("terminal error");
        let err = first.expect_err("must be Err");
        assert!(err.contains("429"), "expected 429 in error, got {err}");
        assert!(rx.recv().await.is_none(), "stream must end after error");
    }

    #[tokio::test]
    async fn http_backend_stream_handles_stream_that_omits_trailing_done() {
        // Not every provider terminates cleanly with `data: [DONE]`;
        // some just close the connection. Confirm we still deliver
        // every buffered frame in that case.
        let server = wiremock::MockServer::start().await;
        let body = sse_body(&["A", "B"], false);
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string(body)
                    .insert_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let backend = HttpCompletionBackend::new("test-backend", server.uri(), "test-model", None);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<String, String>>();
        backend.stream_complete(&test_request("hi", None), tx).await;
        let mut collected = String::new();
        while let Some(item) = rx.recv().await {
            collected.push_str(&item.unwrap());
        }
        assert_eq!(collected, "AB");
    }
}
