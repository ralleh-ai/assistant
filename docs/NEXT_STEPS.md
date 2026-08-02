# Next Steps — Prioritized Backlog

## Done recently

- Http-fetch private-IP / DNS-rebinding guards (T3), mic-capture binary,
  headless audio hardening, Whisper/Piper CLI e2e, Anthropic + T1 auth.

## High priority

1. **OIDC / device attestation** — replace shared-secret tokens when the
   control plane exists.
2. Optional native `piper-rs` / in-process whisper on Linux CI (never on
   default `cargo test --workspace`).
3. When adding clipboard/screen/hotkeys: follow HEADLESS.md rule (trait +
   mock + feature + ignored e2e).

## Medium priority

4. Reconcile crate naming with DEVELOPMENT.md §16.
5. Commit `Cargo.lock`.
6. Expand threat model for Tauri / NestJS.
7. Optional config flag to permit hostname→private resolution for
   intentional internal APIs (today: allowlist the IP literal).

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- Tauri/React shell — Phase 1 (needs display host).
- MCP connectors — Phase 3.
