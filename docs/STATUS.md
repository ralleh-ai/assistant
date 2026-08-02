# Status — Last Validated Snapshot

**As of:** 2026-08-01 (Linux audio-e2e workflow + Cargo.lock CI)

## Build/test state

```
cargo test --workspace → default features (headless CI on every push/PR)
workflow_dispatch: audio-e2e (whisper-cli + piper; optional whisper-rs)
```

## Highlights

- **`.github/workflows/audio-e2e.yml`** — manual Linux job downloads CLI
  tools/models and runs ignored Whisper/Piper e2e; optional
  `--features whisper` job.
- Linux download scripts: `scripts/download-whisper-*.sh`,
  `scripts/download-piper.sh`.
- Default CI + `Cargo.lock` unchanged (no mic/whisper on every PR).

## Next up

OIDC when control plane exists; crate naming; Tauri threat model — see
NEXT_STEPS.md.
