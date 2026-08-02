# Status — Last Validated Snapshot

**As of:** 2026-08-01 (Tauri station log settings + voice_smoke)

## Build/test state

```
cargo test --workspace → headless-safe
desktop-edge: load/save_edge_settings + edge_settings_path; core_ping; voice_smoke
```

## Highlights

- **Station log** — onboarding plates (Station / Identity / Conduit / Voice);
  settings written only via Rust IPC to OS app config `edge-settings.json`.
- **`voice_smoke`** — mock mic → VAD → MockStt → MockTts.
- `scripts/tauri-dev.cmd` for MSVC + npm on Windows.

## Next up

OS capabilities behind features (clipboard/screen/hotkeys) — see NEXT_STEPS.md.
