# Next Steps — Prioritized Backlog

## Done recently

- Tauri `voice_smoke` IPC wired to `ralleh-audio-core` mock pipeline;
  Windows `tauri-dev.cmd` helpers; Phase 1 scaffold + `core_ping`.

## High priority — Tauri desktop shell (Phase 1 continued)

1. ~~Scaffold `desktop-edge/`~~ **done**
2. ~~Wire health / echo IPC (`core_ping`)~~ **done**
3. ~~Embed voice core (mock pipeline via `voice_smoke`)~~ **done**
4. **Settings / onboarding UI** — tenant/device labels, mic permission copy,
   link to local mcp-server config (no OIDC yet).
5. **OS capabilities only as needed** — clipboard/screen/hotkeys behind
   features + policy (T13); never raw FS/net to JS.
6. Optional live mic from the shell (`--features mic` on edge / audio-core).

## Medium priority

7. **OIDC / device attestation** — when NestJS control plane exists (T1/T18).
8. Optional `allow_private_targets` for http-fetch internal APIs.
9. Approval cryptographically bound to approver identity (T4).
10. Audit integrity / queryability beyond JSONL (T5).

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- MCP connectors — Phase 3.
- Native in-crate `piper-rs`.
- Mass-rename crates — deferred ([`CRATE_NAMING.md`](./CRATE_NAMING.md)).
