# Status — Last Validated Snapshot

**As of:** 2026-08-01 (crate naming map + Tauri/NestJS threat expansion)

## Build/test state

```
cargo test --workspace → unchanged (docs-only change this step)
```

## Highlights

- **`docs/CRATE_NAMING.md`** — §16 ↔ actual crate map; mass rename deferred.
- **`docs/THREAT_MODEL.md`** — forward Tauri (T11–T16) and NestJS (T17–T22)
  threats without claiming those surfaces exist yet.

## Next up

OIDC when control plane exists; optional `allow_private_targets` for
http-fetch — see NEXT_STEPS.md.
