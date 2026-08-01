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
cargo run -p ralleh-mcp-server
```

Relevant environment variables (all optional, all have sane defaults):

- `RALLEH_MCP_ADDR` — bind address, default `127.0.0.1:8787`.
- `RALLEH_AUDIT_LOG_PATH` — where the JSONL audit log is written, default
  `<temp_dir>/ralleh-audit.jsonl`.
- `RALLEH_AI_BASE_URL` — if set, boots a real `HttpCompletionBackend`
  against this OpenAI-compatible API root (e.g.
  `https://api.openai.com/v1` or `http://localhost:11434/v1` for a local
  Ollama server). If unset, falls back to `EchoBackend` (no network calls,
  no credentials needed).
- `RALLEH_AI_MODEL` — model name, only relevant if `RALLEH_AI_BASE_URL` is
  set. Default `gpt-4o-mini`.
- `RALLEH_AI_API_KEY` — bearer token, only relevant if
  `RALLEH_AI_BASE_URL` is set. Optional — some local/self-hosted backends
  don't require one.
- `RALLEH_AI_BACKEND_NAME` — cosmetic identifier surfaced in responses/
  audit records, default `http-backend`.

## Git remote

- `origin` → `https://github.com/ralleh-ai/assistant.git`
- Branch: `master`
- `gh auth status` confirms authentication is under the `ralleh-ai`
  GitHub account in the previous environment — verify this is still valid
  (or re-authenticate) in the new environment before pushing.
