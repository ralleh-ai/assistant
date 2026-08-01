# Next Steps — Prioritized Backlog

## Done recently

- Config, durable approvals, http fetch, cpal, STT/TTS traits + mocks,
  Anthropic Messages backend, Bearer token caller auth (T1),
  **WhisperCliStt ggml e2e** (JFK sample), **PiperCliTts** e2e,
  Phase 0 threat model.

## High priority

1. **In-process whisper on Linux CI** — `--features whisper` +
   `whisper_rs_e2e` where bindgen works; keep `WhisperCliStt` as Windows
   fallback.
2. **OIDC / device attestation** — replace shared-secret tokens when the
   control plane exists.
3. Harden http-fetch (private-IP / DNS-rebinding).
4. Optional native `piper-rs` / Kokoro behind a cargo feature (CLI path
   already covers real-model validation).

## Medium priority

5. Reconcile crate naming with DEVELOPMENT.md §16.
6. Commit `Cargo.lock`.
7. Expand threat model for Tauri / NestJS.

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- Tauri/React shell — Phase 1.
- MCP connectors — Phase 3.
