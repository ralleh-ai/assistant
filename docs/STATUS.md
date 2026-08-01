# Status — Last Validated Snapshot

**As of:** 2026-08-01 (Anthropic backend + caller auth + TTS mock)

## Build/test state

```
cargo test --workspace → 129 passed
  ai-router 17 | audio-core 24 | mcp-server 21 | tool-gateway 39
  policy 21 | audit-store 7
```

## Highlights

- **`AnthropicMessagesBackend`** — native `/v1/messages` wire format
  (`RALLEH_AI_PROVIDER=anthropic`).
- **Bearer token auth** — `RALLEH_API_TOKENS` / `_FILE`; spoofed tenant → 403
  (threat model T1 partial close).
- **`MockTts` / `TextToSpeech`**; whisper e2e via ignored test +
  `scripts/download-whisper-model.ps1`.

## Next up

Real whisper utterance smoke; native TTS engine; OIDC — see NEXT_STEPS.md.
