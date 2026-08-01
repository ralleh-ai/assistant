# ADR-003: Rust-Native AI Bindings Preferred Over Python Sidecars

**Status:** Accepted (planning-time decision, copied from DEVELOPMENT.md §20) — **not yet implemented**

## Decision

Prefer native Rust bindings (`whisper-rs`, `llama-cpp-rs`, Rust Piper/Kokoro
bindings) for STT/TTS/local-LLM engines. Fall back to Python or cloud
sidecar processes only when no mature Rust binding covers a required
model/feature.

## Reason

Native bindings avoid IPC overhead and reduce the process/dependency
surface versus spawning separate Python sidecars, while still allowing a
documented escape hatch when Rust tooling lags behind Python's ML
ecosystem. This supersedes the earlier "sidecar-by-default" framing (v0.1
ADR-003 of DEVELOPMENT.md) — sidecars are now the exception, not the
default.

## Implementation status

Not started for STT/TTS/local-LLM. The one real "AI backend" built so far
(`HttpCompletionBackend` in `ralleh-ai-router`, see ADR-008) is a remote
HTTP client, not a local/native binding — it doesn't fall under this ADR's
scope directly, since it's calling a *cloud or self-hosted server*, not
embedding a model in-process. When local STT/TTS/LLM work starts (see
[`../NEXT_STEPS.md`](../NEXT_STEPS.md) item 4), this ADR's "native binding
first" preference should govern the choice.
