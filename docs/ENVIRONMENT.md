# Environment Notes

Practical notes for building/running this repo in a new environment,
carried over from the environment this was originally built in.

## Toolchain

- Rust toolchain is pinned via `rust-toolchain.toml` at the workspace root
  — `rustup` auto-installs the correct version on first build, no manual
  step needed beyond having `rustup` itself installed.
- `./scripts/bootstrap.sh` is the intended single entrypoint for a fresh
  machine: installs/verifies Rust, runs the full test suite. Idempotent —
  safe to re-run after `git pull` to pick up new dependencies or workspace
  members.
- `Cargo.lock` is committed so CI and local builds resolve the same
  dependency versions.
- GitHub Actions (`.github/workflows/ci.yml`) runs headless
  `cargo test --workspace` on push/PR to `master`.
- Optional audio e2e (`.github/workflows/audio-e2e.yml`) is
  **workflow_dispatch only** — downloads Linux whisper-cli/piper and runs
  ignored tests; set `run_whisper_rs` to also exercise `--features whisper`.
- Linux download helpers: `scripts/download-whisper-cli.sh`,
  `download-whisper-model.sh`, `download-piper.sh` (PowerShell twins for
  Windows).
- If not using the bootstrap script, the three manual steps it performs:
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  source "$HOME/.cargo/env"
  cargo test --workspace
  ```

## A real gotcha from the previous environment (may or may not recur)

In the previous (server/headless) environment, a fresh shell did not have
`cargo`/`rustc` on `PATH` even after `rustup` install, because
`$HOME/.cargo/env` needed to be sourced explicitly and wasn't automatically
picked up by the exec/shell wrapper being used. Workaround used throughout
that session: prefix every build/test command with
`source "$HOME/.cargo/env" 2>/dev/null;`. **This may be irrelevant in a
proper desktop environment** where a login shell correctly sources
`~/.cargo/env` (or equivalent) via `.bashrc`/`.zshrc` — but if `cargo`/
`rustc` mysteriously aren't found right after a fresh `rustup` install,
this is the first thing to check.

## Host resource constraints (previous environment — may not apply to new one)

The previous build environment had **~1.9GB free RAM and 2 CPU cores**.
This directly influenced at least one architecture decision (see
[`DECISIONS.md`](./DECISIONS.md) — JSONL file instead of SQLite/sqlx for
audit persistence, to avoid a heavier DB dependency). If the new desktop
environment has materially more resources, it may be worth revisiting
whether that constraint still applies before continuing to design around
it — but don't assume more resources automatically means "add a database
now"; re-evaluate the actual need first.

## Build dependencies added during the last session (network-fetched, not vendored)

The following crates were newly added and will need to be fetched from
crates.io on first build in the new environment (normal `cargo build`
behavior, just flagging since they weren't there originally):

- `ralleh-ai-router`: `reqwest` (with `rustls-tls`, `json` features, no
  default features — avoids an OpenSSL system dependency)
- `ralleh-ai-router` dev-dependencies: `wiremock`
- `ralleh-mcp-server` dev-dependencies: `tempfile`

First build after cloning in a new environment will take noticeably longer
than subsequent builds due to fetching + compiling these (and their
transitive dependency trees — `reqwest` in particular pulls in `hyper`,
`rustls`, `tokio` extras, ICU/unicode data crates, etc.). This is expected,
not a sign of a problem.

## Running the server locally

```bash
# from the repo root (so config/default.toml resolves), or set RALLEH_CONFIG
cargo run -p ralleh-mcp-server
```

Relevant environment variables (all optional, all have sane defaults):

- `RALLEH_CONFIG` — path to the declarative server config (TOML or JSON).
  Default: `config/default.toml` relative to the process cwd. See that
  file for the tool registry + policy rules shape.
- `RALLEH_MCP_ADDR` — bind address, default `127.0.0.1:8787`.
- `RALLEH_AUDIT_LOG_PATH` — where the JSONL audit log is written, default
  `<temp_dir>/ralleh-audit.jsonl`.
- `RALLEH_APPROVAL_STORE_PATH` — JSON snapshot of pending/resolved
  approvals so `RequireApproval` work survives restarts. Default
  `<temp_dir>/ralleh-approvals.json`.
- `RALLEH_AI_BASE_URL` — if set, boots a real completion backend.
- `RALLEH_AI_PROVIDER` — `openai` (default, OpenAI-compatible chat
  completions) or `anthropic` (native Messages API).
- `RALLEH_AI_MODEL` — model name; defaults depend on provider.
- `RALLEH_AI_API_KEY` — bearer / `x-api-key`; required for `anthropic`.
- `RALLEH_AI_BACKEND_NAME` — cosmetic id in responses/audit.
- `RALLEH_API_TOKENS` — enable caller auth:
  `token:tenant:actor[:device];...` (see threat model T1).
- `RALLEH_API_TOKENS_FILE` — JSON token file (preferred over inline env).
## Live mic / desktop audio

- Default builds **do not** link `cpal` — see [`HEADLESS.md`](./HEADLESS.md).
- `RALLEH_LIVE_MIC=1` — run ignored live-mic smoke with `--features mic`.
- `RALLEH_SKIP_LIVE_AUDIO` — force soft-skip of live open in tests
  (`try_open_default`); `mic-capture` clears this if set.
- Interactive capture:
  `cargo run -p ralleh-audio-core --features mic --bin mic-capture -- --seconds 5 --out capture.wav`
- Tauri desktop edge (needs MSVC env on Windows):
  `scripts\tauri-dev.cmd` from the repo root (avoids PowerShell script policy).
- `WHISPER_MODEL_PATH` — ggml model path for ignored whisper e2e tests.
- `WHISPER_CLI_PATH` — path to `whisper-cli` (see
  `scripts/download-whisper-cli.ps1`) for `WhisperCliStt` e2e.
- `PIPER_CLI_PATH` / `PIPER_MODEL_PATH` — Piper executable + `.onnx` voice
  (`scripts/download-piper.ps1`) for `PiperCliTts` e2e.

## Git remote

- `origin` → `https://github.com/ralleh-ai/assistant.git`
- Branch: `master`
- `gh auth status` confirms authentication is under the `ralleh-ai`
  GitHub account in the previous environment — verify this is still valid
  (or re-authenticate) in the new environment before pushing.
