#!/usr/bin/env bash
# Ralleh — one-command bootstrap for a fresh clone.
# Idempotent: safe to re-run any time (e.g. after `git pull`).
#
# What it does:
#   1. Installs Rust (stable, minimal profile) via rustup if not already present.
#   2. Sources the cargo env for the current shell invocation.
#   3. Builds and runs the full workspace test suite.
#
# Usage:
#   ./scripts/bootstrap.sh
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

log() { printf '\033[1;36m[bootstrap]\033[0m %s\n' "$1"; }
fail() { printf '\033[1;31m[bootstrap:error]\033[0m %s\n' "$1" >&2; exit 1; }

log "Repo root: ${REPO_ROOT}"

# 1. Ensure Rust toolchain is present.
if ! command -v rustc >/dev/null 2>&1; then
  log "Rust not found. Installing via rustup (stable, minimal profile)..."
  if ! command -v curl >/dev/null 2>&1; then
    fail "curl is required to install rustup but was not found. Install curl and re-run."
  fi
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain stable --profile minimal
else
  log "Rust toolchain already present: $(rustc --version)"
fi

# 2. Make sure cargo is on PATH for this script invocation.
if [ -f "${HOME}/.cargo/env" ]; then
  # shellcheck disable=SC1091
  source "${HOME}/.cargo/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  fail "cargo still not found on PATH after install. Open a new shell and re-run, or 'source \$HOME/.cargo/env' manually."
fi

log "cargo: $(cargo --version)"
log "rustc: $(rustc --version)"

# 3. rustup will pick up rust-toolchain.toml automatically for the workspace,
#    but make sure the pinned toolchain is installed.
if command -v rustup >/dev/null 2>&1; then
  log "Ensuring pinned toolchain from rust-toolchain.toml is installed..."
  (cd "${REPO_ROOT}" && rustup show >/dev/null 2>&1) || true
fi

# 4. Build + test the full workspace. Default features are headless-safe
#    (no mic / display / whisper.cpp). See docs/HEADLESS.md for desktop
#    opt-in (--features mic, ignored STT/TTS e2e).
log "Running full workspace test suite (cargo test --workspace)..."
(cd "${REPO_ROOT}" && cargo test --workspace)

log "All workspace tests passed."

# 5. Smoke-test the MCP server: boot it for real, hit /healthz over the
#    network, then shut it down cleanly. This is a genuine runtime check,
#    not just a compile/unit-test check -- it proves the built binary
#    actually starts, binds a port, and serves traffic on this machine.
log "Building ralleh-mcp-server binary for smoke test..."
(cd "${REPO_ROOT}" && cargo build --package ralleh-mcp-server --bin ralleh-mcp-server)

SMOKE_PORT="${RALLEH_SMOKE_PORT:-38080}"
SERVER_BIN="${REPO_ROOT}/target/debug/ralleh-mcp-server"

if [ ! -x "${SERVER_BIN}" ]; then
  fail "Expected server binary not found at ${SERVER_BIN} after build."
fi

log "Starting ralleh-mcp-server on 127.0.0.1:${SMOKE_PORT} for smoke test..."
# Explicit config path so the smoke test works even if bootstrap was
# invoked from a directory other than the repo root.
RALLEH_MCP_ADDR="127.0.0.1:${SMOKE_PORT}" \
  RALLEH_CONFIG="${REPO_ROOT}/config/default.toml" \
  "${SERVER_BIN}" &
SERVER_PID=$!

# Ensure the server is always killed, even if the health check fails.
cleanup_server() {
  if kill -0 "${SERVER_PID}" >/dev/null 2>&1; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
}
trap cleanup_server EXIT

HEALTHY=0
for _ in $(seq 1 20); do
  if curl -sf "http://127.0.0.1:${SMOKE_PORT}/healthz" >/dev/null 2>&1; then
    HEALTHY=1
    break
  fi
  sleep 0.25
done

if [ "${HEALTHY}" -ne 1 ]; then
  fail "ralleh-mcp-server did not respond to /healthz within the timeout. Smoke test failed."
fi

log "ralleh-mcp-server smoke test passed: /healthz responded OK on 127.0.0.1:${SMOKE_PORT}."
cleanup_server
trap - EXIT

log "Bootstrap complete. All workspace tests passed and the MCP server smoke test succeeded."
log "Next: see README.md for per-crate notes and current build status."
