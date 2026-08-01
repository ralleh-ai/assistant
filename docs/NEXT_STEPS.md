# Next Steps — Prioritized Backlog

## Done recently

- Config, durable approvals, http fetch, cpal, STT/TTS traits + mocks,
  optional whisper feature + download script, **Anthropic Messages backend**,
  **Bearer token caller auth (T1)**, Phase 0 threat model.

## High priority

1. **Whisper e2e on a real utterance** — run
   `scripts/download-whisper-model.ps1`, then
   `cargo test -p ralleh-audio-core --features whisper -- --ignored whisper_e2e`
   with `WHISPER_MODEL_PATH` set; wire mic→VAD→STT smoke binary.
2. **Native TTS engine** (Piper/Kokoro Rust binding) behind a feature,
   mirroring whisper.
3. **OIDC / device attestation** — replace shared-secret tokens when the
   control plane exists.
4. Harden http-fetch (private-IP / DNS-rebinding).

## Medium priority

5. Reconcile crate naming with DEVELOPMENT.md §16.
6. Commit `Cargo.lock`.
7. Expand threat model for Tauri / NestJS.

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- Tauri/React shell — Phase 1.
- MCP connectors — Phase 3.
