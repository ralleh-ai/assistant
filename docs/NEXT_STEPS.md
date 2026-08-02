# Next Steps — Prioritized Backlog

## Done recently

- Crate naming map (`CRATE_NAMING.md`); Tauri/NestJS forward threats
  (T11–T22); Linux audio-e2e; Cargo.lock + CI; http-fetch SSRF; mic-capture.

## High priority

1. **OIDC / device attestation** — replace shared-secret tokens when the
   control plane exists (closes T1 / T18).
2. When adding clipboard/screen/hotkeys: follow HEADLESS.md rule (trait +
   mock + feature + ignored e2e); map to T13.

## Medium priority

3. Optional config flag to permit hostname→private resolution for
   intentional internal APIs (today: allowlist the IP literal).
4. Approval cryptographically bound to approver identity (T4 gap).
5. Audit integrity / queryability beyond JSONL (T5).

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- Tauri/React shell — Phase 1 (needs display host).
- MCP connectors — Phase 3.
- Native in-crate `piper-rs` (CLI path + Linux e2e already cover real models).
- Mass-rename crates to §16 directory names — deferred (see CRATE_NAMING.md).
