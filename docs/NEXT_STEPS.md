# Next Steps — Prioritized Backlog

## Done recently

- Declarative config, in-process then **durable** approvals, http fetch,
  cpal mic, **STT trait + MockStt** (`whisper` feature optional), Phase 0
  **threat model draft**.

## High priority — spine gaps

1. **Exercise `whisper` feature end-to-end** — download a ggml model,
   `cargo test -p ralleh-audio-core --features whisper`, wire mic → VAD →
   STT in a small binary/smoke path. Trait surface already exists.
2. **TTS binding** (Piper/Kokoro Rust bindings per ADR-003).
3. **Second AI backend** (Anthropic/Google native shape or `llama-cpp-rs`).
4. **Authenticated callers** — today `tenant_id`/`actor_id` are labels only
   (see [`THREAT_MODEL.md`](./THREAT_MODEL.md) T1).

## Medium priority — breadth

5. Harden http-fetch (private-IP / DNS-rebinding controls).
6. Reconcile crate naming with DEVELOPMENT.md §16.
7. Commit `Cargo.lock` for reproducible binary builds.
8. Expand threat model when Tauri / NestJS land.

## Lower priority

- NestJS control plane, Postgres, Redis, NATS, Temporal — Phase 2+.
- Tauri/React desktop shell — Phase 1, separate TS codebase.
- MCP connector runtime — Phase 3.

## Process reminders

- Read DEVELOPMENT.md §22 before privileged-action code.
- Small validated steps; check DECISIONS.md before re-deciding.
