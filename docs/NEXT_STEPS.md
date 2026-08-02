# Next Steps — Prioritized Backlog

## Done recently

- Headless audio hardening (`mic` feature, soft-fail open, mock pipeline
  smoke, `docs/HEADLESS.md`), Whisper/Piper CLI e2e, Anthropic + T1 auth.

## High priority

1. **OIDC / device attestation** — replace shared-secret tokens when the
   control plane exists.
2. Harden http-fetch (private-IP / DNS-rebinding).
3. Optional native `piper-rs` / in-process whisper on Linux CI (never on
   default `cargo test --workspace`).
4. When adding clipboard/screen/hotkeys: follow HEADLESS.md rule (trait +
   mock + feature + ignored e2e).

## Medium priority

5. Reconcile crate naming with DEVELOPMENT.md §16.
6. Commit `Cargo.lock`.
7. Expand threat model for Tauri / NestJS.

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- Tauri/React shell — Phase 1 (needs display host).
- MCP connectors — Phase 3.
