# Status — Last Validated Snapshot

**As of:** 2026-08-01 (headless-hardened audio: `mic` feature-gated)

## Build/test state

```
cargo test --workspace → 130 passed, 2 ignored (CLI e2e)
  (CI=true verified; no cpal link in default features)
cargo test -p ralleh-audio-core --features mic → 26 passed, 3 ignored
```

## Highlights

- **Default audio is headless-safe** — `cpal` behind `--features mic`;
  `try_open_default` soft-fails; live smoke `#[ignore]` + `RALLEH_LIVE_MIC`.
- **Mock pipeline smoke** — VAD → MockStt → MockTts without a device.
- **`WhisperCliStt` / `PiperCliTts`** — ignored real-model e2e (opt-in).
- Anthropic backend + Bearer auth (T1) unchanged.

## Next up

OIDC; http-fetch private-IP harden; Linux CI job notes — see NEXT_STEPS.md
and HEADLESS.md.
