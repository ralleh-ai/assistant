# Status — Last Validated Snapshot

**As of:** 2026-08-01 (desktop environment — config loading landed)

## Build/test state

```
cargo build   → clean (same 2 pre-existing dead_code warnings on EchoHandler /
                AlwaysFailHandler test doubles)
cargo test    → 95 passed, 0 failed, 0 ignored
```

Per-crate test counts:

| Crate | Tests | Notes |
|---|---|---|
| `ralleh-policy-core` | 21 | rule matching, tenant isolation, default-deny, validation; Option fields `#[serde(default)]` for config loading |
| `ralleh-audio-core` | 17 | VAD state machine, wake-word detection/cooldown |
| `ralleh-tool-gateway` | 24 | fs read/write handlers, gateway dispatch logic |
| `ralleh-mcp-server` | 13 | 8 HTTP/router + 5 config-loader (TOML/JSON, validation) |
| `ralleh-ai-router` | 13 | EchoBackend (1), HttpCompletionBackend (6), AiRouter (6) |
| `ralleh-audit-store` | 7 | sink implementations, concurrency safety |
| **Total** | **95** | |

## What's real vs. test-double, per crate

- **`ralleh-policy-core`**: fully real, no test doubles needed (it's pure logic/rules).
- **`ralleh-tool-gateway`**: `FsReadTextHandler` and `FsWriteTextHandler` are real (actually touch the filesystem, sandboxed). `EchoHandler`/`AlwaysFailHandler` exist only as test doubles.
- **`ralleh-audit-store`**: `JsonlFileAuditSink` is real and is what's wired into `ralleh-mcp-server`. `NullAuditSink`/`InMemoryAuditSink` are test/dev doubles.
- **`ralleh-ai-router`**: `HttpCompletionBackend` is real. `EchoBackend` is a test/dev double, still the default in `ralleh-mcp-server` unless `RALLEH_AI_BASE_URL` is set.
- **`ralleh-audio-core`**: **entirely test-double / synthetic** — `MockSource` is the only `AudioSource` implementation.
- **`ralleh-mcp-server`**: real HTTP server. Tool registry + policy rules load from `config/default.toml` (or `RALLEH_CONFIG`); AI backend selection still env-driven.

## Environment/git state

- Repo: `https://github.com/ralleh-ai/assistant.git`, `master` branch.
- New desktop host: Rustup + VS 2022 Build Tools (MSVC) installed so `cargo test` links on Windows.
- `.gitignore` still excludes `Cargo.lock` — see [`NEXT_STEPS.md`](./NEXT_STEPS.md) item on committing it.

## Next up

See [`NEXT_STEPS.md`](./NEXT_STEPS.md) — top of the backlog is now **approval-flow
implementation** (writes already return `ApprovalRequired` but nothing can
grant + resume yet).
