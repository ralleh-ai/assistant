# ADR-009: Anthropic Messages API as Second Completion Backend

**Status:** Accepted — **implemented**

## Decision

Add `AnthropicMessagesBackend` speaking Anthropic's native
`POST /v1/messages` wire format (`x-api-key`, `anthropic-version`, text
content blocks), selected via `RALLEH_AI_PROVIDER=anthropic`.

## Reason

ADR-008 deliberately chose OpenAI-compatible `/chat/completions` as the
widest LCD. NEXT_STEPS asked for a second backend with a **different**
request/response shape to prove `CompletionBackend` is not locked to that
LCD. Anthropic Messages is the clearest widely-used counterexample.

## Consequences

`AiRouter` unchanged. Operators pick provider via env. Local/native LLM
(`llama-cpp-rs`) remains a separate ADR-003 track.
