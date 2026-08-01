# ADR-003: Rust-Native AI Bindings Preferred Over Python Sidecars

**Status:** Accepted — **partially implemented** (adapters + CLI e2e;
in-process bindings still feature-gated / platform-limited)

## Decision

Prefer native Rust bindings (`whisper-rs`, `llama-cpp-rs`, Rust Piper/Kokoro
bindings) for STT/TTS/local-LLM engines. Fall back to Python or cloud
sidecar processes only when no mature Rust binding covers a required
model/feature. Official vendor CLIs (`whisper-cli`, `piper`) are an
allowed interim for e2e and Windows hosts where bindgen fails.

## Reason

Native bindings avoid IPC overhead and reduce the process/dependency
surface versus spawning separate Python sidecars, while still allowing a
documented escape hatch when Rust tooling lags behind Python's ML
ecosystem. This supersedes the earlier "sidecar-by-default" framing (v0.1
ADR-003 of DEVELOPMENT.md) — sidecars are now the exception, not the
default.

## Implementation status

STT: `SpeechToText` + `MockStt`; `WhisperCliStt` (ggml via whisper.cpp CLI,
validated on Windows with `jfk.wav`); `WhisperStt` behind `--features
whisper` (Linux/bindgen hosts). TTS: `TextToSpeech` + `MockTts`;
`PiperCliTts` (ONNX via Piper CLI). Local LLM bindings not started.
`HttpCompletionBackend` / Anthropic remain remote HTTP (ADR-008 / ADR-009).
