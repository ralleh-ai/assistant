# Status — Last Validated Snapshot

**As of:** 2026-08-01 (Tauri Phase 1 scaffold)

## Build/test state

```
cargo test --workspace → headless-safe (desktop-edge NOT in workspace)
cd desktop-edge && npm install && npm run build   # UI
cd desktop-edge && npm run tauri dev              # full app (desktop)
```

## Highlights

- **`desktop-edge/`** — Tauri v2 + React/TS; product name **Ralleh**; IPC
  command `core_ping` returns core status (T11-friendly allowlist).
- Separate Cargo project under `desktop-edge/src-tauri` so default CI never
  needs WebView2/GTK.

## Next up

Path-dep `ralleh-audio-core` into the edge binary; settings UI — see
NEXT_STEPS.md.
