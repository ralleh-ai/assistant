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

## Layout

```text
desktop-edge/
  src/                 React UI (splash · settings · core)
  src-tauri/           Rust edge binary (separate Cargo project)
```

## Next

OIDC / control plane, conversation UI, mic→STT — see `/docs/NEXT_STEPS.md`
and `/docs/HEADLESS.md`.
