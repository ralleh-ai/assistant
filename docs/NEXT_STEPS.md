# Next Steps — Prioritized Backlog

## Done recently

- **Tauri Phase 1 scaffold** (`desktop-edge/`): Tauri v2 + React/TS, branded
  shell, `core_ping` IPC; outside workspace so CI stays headless.
- Crate naming map; Tauri/NestJS threats; Linux audio-e2e; Cargo.lock + CI;
  http-fetch SSRF; mic-capture.

## High priority — Tauri desktop shell (Phase 1 continued)

1. ~~Scaffold `desktop-edge/`~~ **done**
2. ~~Wire health / echo IPC (`core_ping`)~~ **done**
3. **Embed or talk to voice core** — invoke `ralleh-audio-core` mocks from
   `src-tauri` (path dep); optional `--features mic` later.
4. **Settings / onboarding UI** — tenant/device labels, mic permission copy,
   link to local mcp-server config (no OIDC yet).
5. **OS capabilities only as needed** — clipboard/screen/hotkeys behind
   features + policy (T13); never raw FS/net to JS.

## Medium priority

6. **OIDC / device attestation** — when NestJS control plane exists (T1/T18).
7. Optional `allow_private_targets` for http-fetch internal APIs.
8. Approval cryptographically bound to approver identity (T4).
9. Audit integrity / queryability beyond JSONL (T5).

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- MCP connectors — Phase 3.
- Native in-crate `piper-rs`.
- Mass-rename crates — deferred ([`CRATE_NAMING.md`](./CRATE_NAMING.md)).
