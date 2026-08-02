# Status — Last Validated Snapshot

**As of:** 2026-08-01 (live mic smoke + OS caps + station log)

## Build/test state

```
cargo test --workspace → headless-safe (mic feature off)
desktop-edge default: mic_smoke errors cleanly without --features mic
desktop-edge --features mic: live capture (~1s) after station-log Voice clearance
```

## Highlights

- **`mic_smoke`** — policy `os.mic.capture` + `micAcknowledged`; metrics via
  `ralleh_audio_core::run_live_mic_smoke`.
- **`scripts/tauri-dev-mic.cmd`** — Tauri dev with `--features mic`.
- Clipboard / station log / voice mock smokes as before.

## Next up

Medium-priority backlog (OIDC when control plane exists, etc.) — see NEXT_STEPS.md.
