# Ralleh desktop edge (Tauri Phase 1)

Tauri v2 + React/TypeScript shell over a Rust core (ADR-002).

**Not** a member of the root Cargo workspace — `cargo test --workspace`
stays headless-safe without WebView/GTK deps.

## Run (desktop machine)

Requires Node.js on `PATH` (and Rust + WebView2 on Windows).

```bash
cd desktop-edge
npm install
npm run tauri dev
```

UI: **Ping Rust core** → `core_ping` IPC (threat model T11 allowlisted command).

## Layout

```text
desktop-edge/
  src/                 React UI
  src-tauri/           Rust edge binary (separate Cargo project)
```

## Next

Wire voice (`ralleh-audio-core`), settings/onboarding, then OS capabilities
behind features — see `/docs/NEXT_STEPS.md` and `/docs/HEADLESS.md`.
