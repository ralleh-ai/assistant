# Next Steps — Prioritized Backlog

## Done recently

- Optional Linux audio e2e workflow (`audio-e2e`, whisper-cli/piper +
  optional whisper-rs), Cargo.lock + default CI, http-fetch SSRF (T3),
  mic-capture, headless audio, Anthropic + T1.

## High priority

1. **OIDC / device attestation** — replace shared-secret tokens when the
   control plane exists.
2. When adding clipboard/screen/hotkeys: follow HEADLESS.md rule (trait +
   mock + feature + ignored e2e).

## Medium priority

3. Reconcile crate naming with DEVELOPMENT.md §16.
4. Expand threat model for Tauri / NestJS.
5. Optional config flag to permit hostname→private resolution for
   intentional internal APIs (today: allowlist the IP literal).

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- Tauri/React shell — Phase 1 (needs display host).
- MCP connectors — Phase 3.
- Native in-crate `piper-rs` (CLI path + Linux e2e already cover real models).
