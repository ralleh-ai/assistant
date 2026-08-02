# Next Steps — Prioritized Backlog

## Done recently

- OS capability traits (`ralleh-os-capabilities`) + policy-gated `clipboard_smoke`;
  Tauri station log settings; `voice_smoke`; Windows `tauri-dev.cmd` helpers.

## High priority — Tauri desktop shell (Phase 1 continued)

1. ~~Scaffold `desktop-edge/`~~ **done**
2. ~~Wire health / echo IPC (`core_ping`)~~ **done**
3. ~~Embed voice core (mock pipeline via `voice_smoke`)~~ **done**
4. ~~Settings / onboarding UI~~ **done** (station log plates; tenant/device/actor,
   mcp base URL, mic clearance; Rust-only write to app config dir).
5. ~~OS capabilities (clipboard first)~~ **done** — traits + mocks; screen/hotkey
   stubs; `clipboard_smoke` via policy + mock (optional `--features clipboard-os`).
6. Optional live mic from the shell (`--features mic` on edge / audio-core).

## Medium priority

7. **OIDC / device attestation** — when NestJS control plane exists (T1/T18).
8. Optional `allow_private_targets` for http-fetch internal APIs.
9. Approval cryptographically bound to approver identity (T4).
10. Audit integrity / queryability beyond JSONL (T5).
11. Real screen capture / hotkey OS backends (still trait-only stubs).

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- MCP connectors — Phase 3.
- Native in-crate `piper-rs`.
- Mass-rename crates — deferred ([`CRATE_NAMING.md`](./CRATE_NAMING.md)).
