# Status — Last Validated Snapshot

**As of:** 2026-08-01 (Cargo.lock + headless CI workflow)

## Build/test state

```
cargo test --workspace → 134 passed, 2 ignored (CI=true)
GitHub Actions: .github/workflows/ci.yml (ubuntu, default features)
```

## Highlights

- **`Cargo.lock` tracked** for reproducible CI/app builds.
- **Headless CI** — `cargo test --workspace` on push/PR to `master`.
- Http-fetch SSRF harden (T3); mic-capture; audio defaults headless-safe.

## Next up

OIDC when control plane exists; crate naming; Tauri threat model — see
NEXT_STEPS.md.
