# Status — Last Validated Snapshot

**As of:** 2026-08-01 (product setup UX: splash → settings gate → core)

## Build/test state

```
cargo test --workspace → headless-safe (audio-core mic feature off)
desktop-edge: splash/settings/core UI; mic on by default; settings gate
```

## Highlights

- **Product shell** — startup splash; Settings when critical fields missing;
  calm core placeholder with gear → Settings.
- **Voice style** — `calm` | `direct` | `warm` in `edge-settings.json`.
- Smoke IPC kept for developers; not shown on core home.

## Next up

Medium-priority backlog (OIDC when control plane exists, conversation UI,
etc.) — see NEXT_STEPS.md.
