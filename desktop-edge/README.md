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
  always starts.
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
