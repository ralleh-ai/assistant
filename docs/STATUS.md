# Status — Last Validated Snapshot

**As of:** 2026-08-01 (live mic smoke + OS caps + station log)

## Build/test state

```
cargo test --workspace → headless-safe (audio-core mic feature off)
desktop-edge default: mic on (cpal); mic_smoke after station-log Voice clearance
```

## Highlights

- **`mic_smoke`** — policy `os.mic.capture` + `micAcknowledged`; metrics via
  `ralleh_audio_core::run_live_mic_smoke`.
- **`scripts/tauri-dev.cmd`** — Tauri dev (mic on by default for the shell).
- Clipboard / station log / voice mock smokes as before.

## Next up

Medium-priority backlog (OIDC when control plane exists, etc.) — see NEXT_STEPS.md.
