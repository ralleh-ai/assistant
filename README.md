# Ralleh — Enterprise Voice Assistant (Implementation Repo)

This is the buildable code repository for **Ralleh**, the enterprise-grade voice
assistant. Product/architecture planning lives in
`../voice-assistant/DEVELOPMENT.md` (also indexed in the project Vault/wiki) —
this repo is where that plan actually gets built, one validated core module at
a time.

## Guiding rule for this repo

We build in small, independently validated steps. Each core module must have
a real automated test suite and pass CI-equivalent checks locally before the
next module is layered on top. See `DEVELOPMENT.md` §15 (roadmap) and §22
(non-negotiables) for the rules every change here must respect — most
importantly: policy-gated tool execution, no raw secrets, tenant isolation,
and audit events for privileged actions.

## Requirements

- Rust (stable) via [rustup](https://rustup.rs) — this repo pins a toolchain
  via `rust-toolchain.toml`, so `rustup` will auto-install the correct version
  the first time you build.
- No other system dependencies are required for the crates currently in this
  workspace (audio I/O is mocked — see below).

## Quickstart (bulletproof pull-and-run)

```bash
git clone <this-repo-url> ralleh
cd ralleh
./scripts/bootstrap.sh   # installs/verifies Rust toolchain, runs full test suite
```

`bootstrap.sh` is intentionally the *only* command you should need to run on a
fresh machine. It is idempotent — safe to re-run any time, including after
`git pull`, to pick up new dependencies or workspace members.

If you don't want to run a script blindly, the three steps it performs are:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"
cargo test --workspace
```

## Workspace layout

```text
projects/ralleh/
  Cargo.toml                 # workspace manifest
  rust-toolchain.toml        # pinned toolchain version
  scripts/
    bootstrap.sh             # one-command install + verify (see above)
  crates/
    ralleh-policy-core/      # policy evaluation engine (tenant/device/capability gating)
    ralleh-audio-core/       # audio pipeline core; mocked/simulated input for headless dev
    ralleh-tool-gateway/     # policy-gated dispatch chokepoint for all tool/capability calls
    ralleh-mcp-server/       # Axum HTTP surface exposing the tool gateway (real, runnable binary)
    ralleh-ai-router/        # routes completion requests to a pluggable AI backend
```

## Current status (Steps 1-5 of the phased build)

| Module | Status | Notes |
|---|---|---|
| `ralleh-policy-core` | 🟢 validated | Deny-by-default policy evaluation, schema-validated requests, audit-ready decisions. 21 tests, including proof cross-tenant rule leakage is impossible. |
| `ralleh-audio-core` | 🟢 validated | Audio capture is **mocked/simulated** (no mic on the build host). VAD state machine + wake-word trigger detection (utterance windowing, cooldown/debounce) fully tested against synthetic frames. Real device backend and real acoustic wake-word matching (Porcupine/openWakeWord for "Ralleh") are follow-ups once tested on hardware with a mic. 17 tests. |
| `ralleh-tool-gateway` | 🟢 validated | Single chokepoint for every tool/capability call: registry lookup → policy evaluation → conditional handler dispatch → audit event, for every outcome. Includes a real (non-mocked) sandboxed filesystem-read handler. 16 tests, including full cross-tenant isolation and path-traversal rejection against genuinely-existing escape targets. |
| `ralleh-mcp-server` | 🟢 validated | Thin Axum HTTP surface over the tool gateway. Real runnable binary (`RALLEH_MCP_ADDR`), boots and serves `/healthz` + `POST /v1/tools/dispatch`, mapping every gateway outcome to the correct HTTP status. Smoke-tested end-to-end as part of `scripts/bootstrap.sh` (build → boot → real HTTP health check → clean shutdown). 5 tests. |
| `ralleh-ai-router` | 🟢 validated | Routes completion requests through a pluggable `CompletionBackend` trait, mirroring the tool gateway's design, and is now **policy-gated through `ralleh-policy-core`** the same way tool dispatch is — every request is evaluated (tenant/device/actor scoped) before the backend is ever invoked; denied/approval-required decisions short-circuit before touching the backend. Ships with a local `EchoBackend` for dev/testing (no real provider credentials required yet); real provider backends (OpenAI, Anthropic, local inference) are a follow-up. Exposed live over HTTP via `ralleh-mcp-server`'s `POST /v1/completions`. 7 tests in the router crate + 2 HTTP-level tests in mcp-server, including failure-path handling and a policy-denial-through-HTTP proof. |

**Desktop shell (Tauri v2):** deferred by explicit user decision until the dev environment moves to a machine with a display and adequate memory headroom (this headless VPS OOM-killed `cargo install tauri-cli` even after webkit2gtk/gtk system deps were installed).

Each module gets its own `README.md` inside its crate directory documenting
what's validated, what's stubbed, and what "done" means for that module before
the next one is started.

## Running tests

```bash
cargo test --workspace
```

## Running just one crate's tests

```bash
cargo test -p ralleh-policy-core
cargo test -p ralleh-audio-core
```
