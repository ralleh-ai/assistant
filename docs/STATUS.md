# Status — Last Validated Snapshot

**As of:** 2026-08-01 (Tauri voice_smoke + audio-core path dep)

## Build/test state

```
cargo test --workspace → headless-safe
desktop-edge: path-dep ralleh-audio-core; IPC core_ping + voice_smoke
```

## Highlights

- **`voice_smoke`** — UI invokes mock mic → VAD → MockStt → MockTts via
  `ralleh_audio_core::run_mock_voice_pipeline`.
- `scripts/tauri-dev.cmd` for MSVC + npm on Windows.

## Next up

Settings / onboarding UI in `desktop-edge` — see NEXT_STEPS.md.
