# Status — Last Validated Snapshot

**As of:** 2026-08-01 (last working session before the desktop environment transition)

## Build/test state

```
cargo build   → clean (only 2 pre-existing dead_code warnings: EchoHandler,
                AlwaysFailHandler test doubles never constructed outside
                their own test modules — harmless, unrelated to any bug)
cargo test    → 97 passed, 0 failed, 0 ignored
```

Per-crate test counts:

| Crate | Tests | Notes |
|---|---|---|
| `ralleh-policy-core` | 21 | rule matching, tenant isolation, default-deny, validation |
| `ralleh-audio-core` | 17 | VAD state machine, wake-word detection/cooldown |
| `ralleh-tool-gateway` | 24 | fs read/write handlers (9 each-ish), gateway dispatch logic |
| `ralleh-mcp-server` | 8 | HTTP routes incl. 1 full end-to-end integration test |
| `ralleh-ai-router` | 13 | EchoBackend (1), HttpCompletionBackend (6), AiRouter (6) |
| `ralleh-audit-store` | 7 | sink implementations, concurrency safety |
| **Total** | **97** | (plus 0 doc-tests across all crates) |

## What's real vs. test-double, per crate

- **`ralleh-policy-core`**: fully real, no test doubles needed (it's pure logic/rules).
- **`ralleh-tool-gateway`**: `FsReadTextHandler` and `FsWriteTextHandler` are real (actually touch the filesystem, sandboxed). `EchoHandler`/`AlwaysFailHandler` exist only as test doubles (hence the dead_code warnings — they're used inside `#[cfg(test)]` but the compiler warns because they're not constructed *outside* test code, which is expected and fine).
- **`ralleh-audit-store`**: `JsonlFileAuditSink` is real and is what's wired into `ralleh-mcp-server`. `NullAuditSink`/`InMemoryAuditSink` are test/dev doubles.
- **`ralleh-ai-router`**: `HttpCompletionBackend` is real (makes actual HTTP calls to an OpenAI-compatible endpoint). `EchoBackend` is a test/dev double, still the default in `ralleh-mcp-server` unless `RALLEH_AI_BASE_URL` is set.
- **`ralleh-audio-core`**: **entirely test-double / synthetic** right now — `MockSource` is the only `AudioSource` implementation. No real microphone capture, no real STT/TTS. This is the least-built crate relative to the DEVELOPMENT.md plan.
- **`ralleh-mcp-server`**: real HTTP server, genuinely runnable (`cargo run -p ralleh-mcp-server`), but all wiring (policy rules, handler registration) is hardcoded in `main.rs`, not config-driven.

## Environment/git state

- Repo pushed to `https://github.com/ralleh-ai/assistant.git`, `master` branch, tracking set up.
- This was the **first git init** for this project directory — it had no `.git` before this session. Full history starts from a single "Initial commit" containing the entire workspace as it stood.
- `.gitignore` excludes `/target` and `Cargo.lock` (workspace-root `.gitignore`; consider whether `Cargo.lock` should actually be committed for a binary-producing workspace — see [`NEXT_STEPS.md`](./NEXT_STEPS.md)).

## Known outstanding item from a *different* workstream (not blocking this repo)

Not related to the Rust workspace, but open as of the same session: a
request to change the OpenClaw gateway's own model fallback chain
(`agents.defaults.model.fallbacks`, drop `gpt-5.5`, promote
`claude-sonnet-4.6`) is blocked because those config paths are marked
protected and can't be changed via the gateway config tool. This is an
OpenClaw runtime/assistant-config concern, not a Ralleh product concern —
noted here only so it isn't lost, since it surfaced in the same session as
this documentation effort. Resolving it requires either a manual config
file edit + gateway restart, or lifting the protection through whatever
mechanism the OpenClaw operator uses.
