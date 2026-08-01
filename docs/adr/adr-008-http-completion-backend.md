# ADR-008: OpenAI-Compatible HTTP Wire Format as the First Real Completion Backend

**Status:** Accepted — **implemented**

## Decision

The first real (non-mocked) `CompletionBackend` implementation,
`HttpCompletionBackend`, speaks the OpenAI-compatible `/chat/completions`
wire format rather than a provider-specific format (e.g. Anthropic's native
Messages API shape).

## Reason

This format is the widest-coverage lowest-common-denominator across real
deployment targets: OpenAI itself, and self-hosted `vLLM`, `Ollama`, and
`llama.cpp` servers all speak it. One backend implementation therefore
covers a meaningfully broad swath of realistic providers, directly serving
DEVELOPMENT.md §17's "adapter interfaces over provider lock-in" principle
and §22's non-negotiable "prefer adapter interfaces over provider
lock-in." Building a genuinely provider-agnostic backend first (rather than
committing to one vendor's native API shape) was judged more valuable than
optimizing for any single provider's specific features.

## Implementation notes

- Configurable via `base_url` (this backend appends `/chat/completions`
  itself), `model` (with per-request `model_hint` override support),
  optional bearer `api_key`, and a cosmetic `name` for audit/routing
  identification.
- 30-second request timeout.
- All failure modes — network errors, non-2xx HTTP status, malformed JSON
  response, empty `choices` array — are converted to `Err(String)`, never
  a panic, matching the same "handler errors don't crash the caller"
  discipline `ToolHandler` implementations follow.
- Tested with `wiremock` (6 tests): success path, model-hint override,
  bearer auth header, HTTP error status, malformed JSON, empty choices.
  Zero live network calls in the test suite.

## Consequences / when to revisit

This does **not** cover local/native model inference (e.g. embedding
`llama-cpp-rs` directly in-process) — that's a different backend
implementation, governed by ADR-003's "native binding preferred" guidance,
and is still on the backlog (see [`../NEXT_STEPS.md`](../NEXT_STEPS.md)
item 5). It also doesn't cover any provider whose native API materially
diverges from the OpenAI shape (e.g. if a future requirement needs
Anthropic-specific features not exposed through an OpenAI-compatible
proxy) — that would need its own `CompletionBackend` implementation,
which the trait already supports without any changes to `AiRouter` itself.
