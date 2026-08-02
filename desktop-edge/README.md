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

Manual equivalent:

```powershell
# Developer PowerShell for VS, or:
#   & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\Launch-VsDevShell.ps1" -Arch amd64
cd desktop-edge
npm.cmd install
npm.cmd run tauri dev
```

If `npm install` warns about pending install scripts for `esbuild`, either ignore it when `package.json` already has `"allowScripts": { "esbuild@…": true }`, or run:

```powershell
npm.cmd approve-scripts --all esbuild
```

Do **not** use bare `npm approve-scripts esbuild` — that only matches direct deps and returns `ENOMATCH`.

**PowerShell note:** if you see `npm.ps1 cannot be loaded because running scripts is disabled`, use `npm.cmd` (as above) or `Set-ExecutionPolicy -Scope Process Bypass`.

If you get `LNK1104: cannot open file 'msvcrt.lib'` or missing `excpt.h`, you
started a normal shell without the VS C++ environment — use
`./scripts/tauri-dev.ps1` or “Developer PowerShell for VS”.

UI: **Ping Rust core** (`core_ping`) and **Voice smoke (mock)**
(`voice_smoke` → `ralleh-audio-core` mock VAD/STT/TTS).

## Layout

```text
desktop-edge/
  src/                 React UI
  src-tauri/           Rust edge binary (separate Cargo project)
```

## Next

Wire voice (`ralleh-audio-core`), settings/onboarding, then OS capabilities
behind features — see `/docs/NEXT_STEPS.md` and `/docs/HEADLESS.md`.
