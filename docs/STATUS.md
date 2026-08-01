# Status — Last Validated Snapshot

**As of:** 2026-08-01 (http fetch + cpal mic source)

## Build/test state

```
cargo test --workspace → all crates green
  (audio-core 19, tool-gateway 37, mcp-server 16, …)
```

| Crate | Tests | Notes |
|---|---|---|
| `ralleh-policy-core` | 21 | |
| `ralleh-audio-core` | 19 | VAD/wake-word + FrameAssembler + cpal try_open |
| `ralleh-tool-gateway` | 37 | fs + http fetch + approvals + gateway |
| `ralleh-mcp-server` | 16 | HTTP + config + approve e2e |
| `ralleh-ai-router` | 13 | |
| `ralleh-audit-store` | 7 | |

## Highlights

- **`tool.http.fetch`**: allowlisted egress GET (`HttpFetchHandler`).
- **`CpalMicSource`**: live mic via `cpal`; `try_open_default()` → `None` on headless hosts.
- Approvals + declarative config remain as previously landed.

## Next up

STT binding (`whisper-rs`), durable approvals, second AI backend, threat model — see [`NEXT_STEPS.md`](./NEXT_STEPS.md).
