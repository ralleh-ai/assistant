# Next Steps — Prioritized Backlog

## Done recently

- Crate naming map; Tauri/NestJS forward threats (T11–T22); Linux audio-e2e;
  Cargo.lock + CI; http-fetch SSRF; mic-capture; headless audio hardening.

## High priority — Tauri desktop shell (Phase 1)

Start the **Tauri v2 + React/TS UI** edge app (ADR-002). This is the next
implementation track. Follow [`HEADLESS.md`](./HEADLESS.md) for any OS
capability: trait + mock + feature + ignored e2e; map new surfaces to
threats T11–T16.

Suggested order:

1. **Scaffold `desktop-edge/`** — Tauri v2 app, React/TS UI shell, minimal
   window that loads; keep default `cargo test --workspace` headless-safe
   (Tauri not required for Rust crate CI).
2. **Wire health / echo IPC** — one allowlisted Tauri command calling into
   existing Rust (e.g. version or mcp-server `/healthz` client); prove
   UI→Rust boundary (T11).
3. **Embed or talk to voice core** — invoke `ralleh-audio-core` mocks from
   Rust side; optional `--features mic` path using `mic-capture` patterns.
4. **Settings / onboarding UI** — tenant/device labels, mic permission
   copy, link to local mcp-server config (no OIDC yet).
5. **OS capabilities only as needed** — clipboard/screen/hotkeys behind
   features + policy capabilities (T13); do not dump raw FS/net to JS.

## Medium priority

6. **OIDC / device attestation** — replace shared-secret Bearer tokens
   when the NestJS control plane exists (T1 / T18).
7. Optional `allow_private_targets` for http-fetch internal APIs.
8. Approval cryptographically bound to approver identity (T4).
9. Audit integrity / queryability beyond JSONL (T5).

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- MCP connectors — Phase 3.
- Native in-crate `piper-rs`.
- Mass-rename crates to §16 names — deferred ([`CRATE_NAMING.md`](./CRATE_NAMING.md)).
