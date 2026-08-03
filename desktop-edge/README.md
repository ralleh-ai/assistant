# Ralleh desktop edge (Tauri Phase 1)

Tauri v2 + React/TypeScript shell over a Rust core (ADR-002).

**Not** a member of the root Cargo workspace — `cargo test --workspace`
stays headless-safe without WebView/GTK deps.

## Run (desktop machine)

Requires Node.js on `PATH`, Rust, WebView2, and the **MSVC C++ toolchain**
(Desktop development with C++ / Build Tools + Windows SDK).

**Recommended (Windows):** loads `VsDevCmd` so `link.exe` can find `msvcrt.lib`
(use the `.cmd` launcher so PowerShell execution policy does not block you):

```bat
scripts\tauri-dev.cmd
```

From PowerShell:

```powershell
cmd /c scripts\tauri-dev.cmd
```

## Product UI

1. **Splash** — short branded startup.
2. **Settings** — required on first run or when critical fields are missing
   (tenant, device, actor, mcp URL, mic clearance, voice style).
3. **Core shell** — calm placeholder once settings are complete; gear opens
   Settings again.

Live mic is **on by default** for this desktop shell. In Settings → Voice,
stamp clearance, then optionally **Listen once**. Developer smoke IPC
(`core_ping`, `voice_smoke`, `clipboard_smoke`, `mic_smoke`) remains for
CLI/tests — not on the core home screen.

## Environment variables

- `RALLEH_PRESENCE_BIN` / `PRESENCE_DROPLET` / `PRESENCE_TRANSPARENT` —
  see `../presence-prototype/README.md` for the presence-runtime
  side (child process, window chrome, transparency + click-through).
- `RALLEH_SCAN_SWEEP_MS` — opt-in interval (ms) for the sparse
  scan-sweep attention pulse. Missing / `0` / unparseable disables
  it; a minimum of 5000 ms is enforced. The sweep only fires while
  `AssistantState::is_idle()` is true, so it never competes with
  real thinking / tool-use / speaking activity.
- `RALLEH_COMPLETION_KIND` — completion backend selector: `echo`
  (default), `anthropic`, or `openai` (OpenAI-compatible
  `/chat/completions` — OpenAI itself, Ollama, LM Studio, vLLM,
  etc.). Missing or unrecognized falls back to Echo with a log
  line. **Note**: the in-app **Backend** settings panel takes
  precedence over these env vars — once the operator saves a
  config through the UI, that config wins on every subsequent
  startup and can only be reverted by pressing "Clear" (or
  deleting the `edge-settings.json` file).
- `RALLEH_COMPLETION_BASE_URL` — API root. For `openai`, include
  the `/v1` suffix if the provider requires it (the backend
  appends `/chat/completions`). For `anthropic`, root only
  (backend appends `/v1/messages`).
- `RALLEH_COMPLETION_MODEL` — model identifier the backend sends
  in each request.
- `RALLEH_COMPLETION_API_KEY` — optional for `openai` (local
  servers often accept unauthenticated calls), required for
  `anthropic`. A misconfigured non-echo kind (e.g. anthropic
  without a key) falls back to Echo with a warning; the shell
  always starts. Keys set through the in-app Backend settings
  panel are stored in the **OS keychain** (Windows Credential
  Manager, macOS Keychain, or the Linux Secret Service) — never
  in the cleartext `edge-settings.json`. On a host without a
  working keychain the panel surfaces a **Cleartext on disk**
  badge so the operator is never misled about where the secret
  lives. An older settings file with a cleartext key is
  migrated into the keychain on first startup (best-effort;
  leaves the cleartext copy in place only if the keychain is
  unavailable).
- **Settings schema versioning** — `edge-settings.json` now
  carries a `version` field (`CURRENT_SETTINGS_VERSION = 1` at
  time of writing). Loads route through an ordered migration
  chain so a shape-changing bump can rename or reshape fields
  without silent data loss. Missing `version` on legacy files
  defaults to 0 and triggers the v0→v1 no-op migration on first
  successful load, rewriting the file at the new version. A
  file at a version this build doesn't understand
  (`on_disk > CURRENT`) is *never* overwritten — the shell
  logs a warning, runs on in-memory defaults for the session,
  and emits a `settings-migrate-failed` audit event so an
  operator sees the mismatch. Successful migrations emit
  `settings-migrate` with `detail.from` / `detail.to`.
  Migrations must be idempotent (rerun after crash-mid-write
  is safe) — enforced by unit tests.
- **Router health probe** — a background thread pings the
  active completion backend every 60 s (overridable with
  `RALLEH_HEALTH_PROBE_INTERVAL_MS`, floor 5 s; disabled with
  `RALLEH_HEALTH_PROBE_DISABLED`) so `assistant_backend_status`
  reflects *current* reachability, not "we thought it worked at
  startup". Health lands on `BackendStatus.health` as
  `state = unknown | healthy | unhealthy | skipped` plus latency,
  last error, and a consecutive-failure count. State-machine
  edges (healthy ↔ unhealthy) emit `router-healthy` /
  `router-unhealthy` audit events with `detail.latency_ms` and
  `detail.error`. The `assistant_probe_backend` command triggers
  an on-demand probe (same code path, `detail.trigger=manual`).
  Echo backend is always `skipped` — its response is synthetic,
  and lighting the UI green for it would be misleading.
- **Presence stderr capture** — the runtime's stderr (its own
  `log::info!` output, wgpu validation errors, panic traces) is
  piped to a rotated text log `presence.log` under the Tauri app
  config dir. Same 4 MiB / one rollover policy as the audit log,
  so operators only learn one retention story. When a
  `presence-stalled` event fires, its `detail.log_path` names
  this file directly. Tail from the UI via the
  `presence_log_tail` Tauri command (default 100 lines, clamped
  1..=1000). Falls back to `log::debug!` when the file cannot be
  opened — a broken sink never drops a line silently.
- **Presence liveness monitor** — the shell spawns a background
  watcher that reads a heartbeat stream from the `presence-runtime`
  child (every 2 s over its stdout NDJSON channel) and flags a
  stall when no event of any kind arrives for
  `STALL_THRESHOLD_MS` (6 s, i.e. three missed beats). Transitions
  are recorded to the audit log as `presence-stalled` and
  `presence-recovered` events with the last heartbeat sequence,
  runtime uptime, and elapsed / recovery times attached. The
  monitor is inert when presence is disabled (no
  `RALLEH_PRESENCE_BIN`) and does not auto-restart the child —
  restarts are an explicit operator decision surfaced through the
  audit trail rather than a silent recovery. Snapshot the current
  state via the `presence_status` Tauri command (`last_event_ms_ago`,
  `last_heartbeat_sequence`, `last_heartbeat_uptime_ms`).
- **Audit log** — every policy-relevant event (egress
  allow/deny, backend swap, secret write/clear, keychain
  migration, presence stall/recovery) is appended as JSON-Lines
  to `audit.jsonl` under the
  Tauri app config dir (the same directory as
  `edge-settings.json`). Size-based rotation kicks in at 4 MiB;
  one rollover file (`audit.jsonl.1`) is retained. Read the tail
  from the settings-UI diagnostic panel via the
  `assistant_audit_tail` Tauri command, or open the file
  directly. Never contains raw API keys — only labels and
  storage-provenance strings. Failed audit writes never block the
  action being recorded (fail-open on evidence, fail-closed on
  authorization).
- `RALLEH_COMPLETION_ALLOWED_HOSTS` — comma-separated allowlist
  of hostnames the shell is willing to send completion traffic
  to. Enforced at three layers (settings save, "Test connection"
  probe, and request-time backend construction) so a hostile
  `base_url` cannot exfiltrate the OS-keychain-stored API key.
  Defaults to `api.openai.com,api.anthropic.com,localhost,
  127.0.0.1,0.0.0.0,::1`. Set to a narrower list to lock down an
  enterprise deployment (e.g.
  `RALLEH_COMPLETION_ALLOWED_HOSTS=llm.acme.internal`) or to an
  empty string to disable all outbound completions entirely
  (airgap testing). `http://` is refused for any non-loopback
  host regardless of the allowlist — credentials in the
  `Authorization` header must never travel in cleartext.
- `RALLEH_SKIP_LIVE_AUDIO` — set to any value to force the mic
  and speaker sinks to soft-skip (return `None` from
  `try_open_default`). Useful for headless dev on hosts with
  broken audio stacks or when running the shell over remote
  desktop where playing audio locally isn't desired.
- `RALLEH_LIVE_PLAYBACK` — set to `1` to force live speaker
  playback under CI (mirrors `RALLEH_LIVE_MIC` on the input
  side). Default CI runs soft-skip playback so headless jobs
  never open an output device.

## Layout

```text
desktop-edge/
  src/                 React UI (splash · settings · core)
  src-tauri/           Rust edge binary (separate Cargo project)
```

## Next

OIDC / control plane, conversation UI, mic→STT — see `/docs/NEXT_STEPS.md`
and `/docs/HEADLESS.md`.
