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

STT adapter surface started: `SpeechToText` + `MockStt` always on;
`WhisperStt` (`whisper-rs`) is available behind the `whisper` cargo
feature and still needs a ggml model + e2e smoke on a real utterance.
TTS / local-LLM bindings are not started. `HttpCompletionBackend` remains
a remote HTTP client (ADR-008), not an in-process model.
