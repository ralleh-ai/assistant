# Next Steps — Prioritized Backlog

For whoever (human or agent) continues this work after the environment
transition. Ordered roughly by what unblocks the most future work per unit
of effort, not strictly by DEVELOPMENT.md phase order.

## Done recently

- **Policy/registry config loading.** Declarative TOML/JSON via
  `config/default.toml` / `RALLEH_CONFIG`.
- **Approval-flow (minimal in-process).** `ApprovalStore` +
  `POST /v1/approvals/:id/approve|reject`.
- **Allowlisted HTTP fetch handler.** `tool.http.fetch` /
  `HttpFetchHandler` with explicit `allowed_hosts` egress allowlist.
- **Real mic capture via `cpal`.** `CpalMicSource` implements
  `AudioSource`; `try_open_default()` is safe on headless hosts.
  STT/TTS bindings still outstanding.

## High priority — spine gaps

1. **STT binding** (`whisper-rs` / whisper.cpp per ADR-003) on top of
   the now-real `AudioSource` path; keep VAD/wake-word unchanged.
2. **Durable approvals** — persist `ApprovalStore` once Phase 2 storage
   exists; keep the approve/reject HTTP contract.
3. **Additional tool handlers** as deployment needs appear (search API,
   etc.).

## Medium priority — breadth

4. **Second AI backend** to prove `CompletionBackend` genuinely
   generalizes beyond one wire format — e.g. a native Anthropic or Google
   backend, or a local `llama-cpp-rs` binding per ADR-003.
5. **Threat model document** (DEVELOPMENT.md §15 Phase 0 deliverable,
   still not written) — should live in `docs/THREAT_MODEL.md` once
   started. Not yet begun.
6. **Reconcile crate naming** with DEVELOPMENT.md §16's planned layout.
7. **`Cargo.lock` handling** — reconsider committing it for reproducible
   binary builds.

## Lower priority — not yet relevant at this stage

- NestJS control plane, Postgres/pgvector, Redis, NATS, Temporal — Phase 2+.
- Tauri/React desktop shell — Phase 1, separate TypeScript codebase.
- MCP connector runtime / first-party SaaS connectors — Phase 3.

## Process reminders

- Read [`DEVELOPMENT.md`](./DEVELOPMENT.md) §22 before privileged-action code.
- Build in small, independently validated steps.
- Check [`DECISIONS.md`](./DECISIONS.md) before re-deciding implementation calls.
