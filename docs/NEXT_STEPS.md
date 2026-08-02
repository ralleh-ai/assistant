# Next Steps — Prioritized Backlog

## Done recently

- Live mic smoke from desktop-edge (`--features mic`); OS capability traits +
  clipboard smoke; station log settings; voice_smoke.

## High priority — Tauri desktop shell (Phase 1 continued)

1. ~~Scaffold `desktop-edge/`~~ **done**
2. ~~Wire health / echo IPC (`core_ping`)~~ **done**
3. ~~Embed voice core (mock pipeline via `voice_smoke`)~~ **done**
4. ~~Settings / onboarding UI~~ **done** (station log plates; tenant/device/actor,
   mcp base URL, mic clearance; Rust-only write to app config dir).
5. ~~OS capabilities (clipboard first)~~ **done** — traits + mocks; screen/hotkey
   stubs; `clipboard_smoke` via policy + mock (optional `--features clipboard-os`).
6. ~~Optional live mic from the shell~~ **done** — `mic_smoke` (policy + clearance);
   **on by default** in `desktop-edge` (`mic` feature / `build.features`).
   Workspace audio-core stays mic-off for headless CI.

## Medium priority

7. **OIDC / device attestation** — when NestJS control plane exists (T1/T18).
8. Optional `allow_private_targets` for http-fetch internal APIs.
9. Approval cryptographically bound to approver identity (T4).
10. Audit integrity / queryability beyond JSONL (T5).
11. Real screen capture / hotkey OS backends (still trait-only stubs).
12. Live mic → VAD → STT path in the shell (beyond capture metrics).

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- MCP connectors — Phase 3.
- Native in-crate `piper-rs`.
- Mass-rename crates — deferred ([`CRATE_NAMING.md`](./CRATE_NAMING.md)).
