# Status — Last Validated Snapshot

**As of:** 2026-08-01 (Whisper CLI ggml e2e + Piper CLI TTS)

## Build/test state

```
cargo test --workspace → 130 passed, 2 ignored (CLI e2e)
  ai-router 17 | audio-core 25 (+2 ignored) | mcp-server 21 | tool-gateway 39
  policy 21 | audit-store 7
```

## Highlights

- **`WhisperCliStt`** — real ggml e2e via whisper.cpp CLI (Windows-friendly;
  in-process `whisper-rs` still blocked on MSVC bindgen).
- **`PiperCliTts`** — real ONNX voice e2e via Piper CLI.
- **Anthropic backend** + **Bearer auth (T1)** + mocks remain as before.

## Next up

OIDC/device attestation; http-fetch private-IP harden; in-process whisper on
Linux CI — see NEXT_STEPS.md.
