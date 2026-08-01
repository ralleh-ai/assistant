# Status — Last Validated Snapshot

**As of:** 2026-08-01 (desktop environment — config loading + approval flow)

## Build/test state

```
cargo build   → clean (same 2 pre-existing dead_code warnings on EchoHandler /
                AlwaysFailHandler test doubles)
cargo test    → 102 passed, 0 failed, 0 ignored
```

Per-crate test counts:

| Crate | Tests | Notes |
|---|---|---|
| `ralleh-policy-core` | 21 | rule matching, tenant isolation, default-deny, validation |
| `ralleh-audio-core` | 17 | VAD state machine, wake-word detection/cooldown |
| `ralleh-tool-gateway` | 30 | fs handlers + gateway + approval store/approve/reject |
| `ralleh-mcp-server` | 14 | HTTP routes + config loader + approve-write e2e |
| `ralleh-ai-router` | 13 | EchoBackend / HttpCompletionBackend / AiRouter |
| `ralleh-audit-store` | 7 | sink implementations, concurrency safety |
| **Total** | **102** | |

## What's real vs. test-double, per crate

- **`ralleh-policy-core`**: fully real.
- **`ralleh-tool-gateway`**: real fs read/write handlers; in-process
  `ApprovalStore` parks `RequireApproval` calls and resumes on approve.
- **`ralleh-audit-store`**: `JsonlFileAuditSink` is real.
- **`ralleh-ai-router`**: `HttpCompletionBackend` real; `EchoBackend` default unless `RALLEH_AI_BASE_URL` is set.
- **`ralleh-audio-core`**: entirely synthetic (`MockSource` only).
- **`ralleh-mcp-server`**: config-driven registry/policy; approval HTTP
  routes at `POST /v1/approvals/:id/approve|reject`.

## Next up

See [`NEXT_STEPS.md`](./NEXT_STEPS.md) — top of the backlog is now a
**second real tool handler** (search or allowlisted HTTP fetch).
