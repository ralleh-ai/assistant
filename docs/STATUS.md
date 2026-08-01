# Status — Last Validated Snapshot

**As of:** 2026-08-01 (STT adapters + durable approvals + threat model)

## Build/test state

```
cargo test --workspace → 118 passed
  audio-core 22 | tool-gateway 39 | mcp-server 16 | policy 21
  ai-router 13 | audit-store 7
```

## Highlights

- **`SpeechToText` + `MockStt`** in `ralleh-audio-core`; optional
  `WhisperStt` behind `--features whisper` (ADR-003).
- **Durable `ApprovalStore::open`** — JSON snapshot; mcp-server defaults
  to `RALLEH_APPROVAL_STORE_PATH` / temp `ralleh-approvals.json`.
- **`docs/THREAT_MODEL.md`** — Phase 0 draft for the current Rust spine.

## Next up

Whisper e2e with a real ggml model; TTS; second AI backend — see
[`NEXT_STEPS.md`](./NEXT_STEPS.md).
