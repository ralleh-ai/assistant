# Next Steps — Prioritized Backlog

For whoever (human or agent) continues this work after the environment
transition. Ordered roughly by what unblocks the most future work per unit
of effort, not strictly by DEVELOPMENT.md phase order.

## Done since last session

- **Policy/registry config loading** (was #1). `ralleh-mcp-server` now
  loads tools + ordered policy rules from a declarative TOML/JSON file
  (`config/default.toml` by default, override with `RALLEH_CONFIG`).
  Handler implementations stay in code; the config names which known
  handlers to wire. See `crates/ralleh-mcp-server/src/config.rs`.

## High priority — spine gaps

1. **Approval-flow implementation.** `PolicyEngine`/`ToolGateway` correctly
   *stop* execution and report `ApprovalRequired`, but there is no mechanism
   yet for an approval to actually be granted and the original request
   re-executed. DEVELOPMENT.md's data model (§13) has `ApprovalRequest` as
   an entity — none of that persistence/workflow exists yet. This is
   arguably a Phase 4 (Temporal/workflow) concern, but a minimal
   in-process version could be built much sooner to prove the concept.
   Especially relevant now that `tool.fs.write_text` is
   `RequireApproval`-gated via config — the HTTP surface returns the
   approval-required outcome but nothing can yet grant it and resume.
2. **Second/third real tool handler beyond filesystem read/write** — to
   prove the gateway pattern generalizes. Good candidates: a "web search"
   tool wrapping an existing search API (low policy risk, easy to make
   `Allow`-gated), or a simple HTTP-fetch tool with an explicit egress
   allowlist (directly exercises DEVELOPMENT.md §11.1's "egress controls").
   New handlers need a `HandlerKind` variant in the config loader plus an
   entry in `config/default.toml` (or a deployment-specific config).
3. **Real audio I/O in `ralleh-audio-core`.** Currently 100% synthetic. Per
   DEVELOPMENT.md §2.3/§17/Phase 1: wire real microphone capture (likely
   `cpal`), then a real STT binding (`whisper-rs`/whisper.cpp preferred per
   ADR-003), keeping the existing VAD/wake-word state machines as-is since
   they're already tested against the `AudioSource` trait abstraction —
   swapping `MockSource` for a real capture source shouldn't require
   touching VAD/wake-word logic at all if the trait boundary is respected.

## Medium priority — breadth

4. **Second AI backend** to prove `CompletionBackend` genuinely
   generalizes beyond one wire format — e.g. a native Anthropic or Google
   backend (different request/response shape than OpenAI's), or a local
   `llama-cpp-rs` binding per ADR-003's "native Rust binding preferred"
   guidance.
5. **Threat model document** (DEVELOPMENT.md §15 Phase 0 deliverable,
   still not written) — should live in `docs/THREAT_MODEL.md` once
   started. Not yet begun.
6. **Reconcile crate naming** with DEVELOPMENT.md §16's planned layout
   (`ralleh-tool-gateway` vs. planned `mcp-gateway`; `ralleh-audit-store`
   vs. planned `audit-core`). Purely cosmetic/organizational — low
   priority, but worth doing before more crates accumulate and the
   divergence gets harder to unwind. Alternatively, treat DEVELOPMENT.md
   §16 as advisory rather than binding and formally record the actual
   naming convention in an ADR instead of renaming crates.
7. **`Cargo.lock` handling** — previously gitignored. For a workspace that
   produces binaries (`ralleh-mcp-server`), the general Rust convention is
   to commit `Cargo.lock` for reproducible builds. Reconsider whether the
   `.gitignore` entry was the right call, especially once this repo starts
   being built in CI or by multiple people/environments. (A lockfile is
   generated locally on first `cargo build`/`test` either way.)

## Lower priority — not yet relevant at this stage

- NestJS control plane, Postgres/pgvector, Redis, NATS, Temporal — all
  Phase 2/3/4 per DEVELOPMENT.md, and none of the Phase 1 (real audio I/O,
  desktop shell) work is done yet, so starting these now would be
  building the wrong layer first.
- Tauri/React desktop shell — same reasoning; also explicitly Phase 1
  scope, but is a separate (TypeScript) codebase that this Rust repo would
  eventually be consumed by, not something to build inside this repo.
- MCP connector runtime / first-party SaaS connectors — Phase 3.

## Process reminders for whoever picks this up

- **Read [`DEVELOPMENT.md`](./DEVELOPMENT.md) §22 ("Non-Negotiables")
  before writing any new privileged-action code.** Every new capability
  needs a policy gate and an audit event — no exceptions, no "we'll add
  policy later."
- **Build in small, independently validated steps** (the repo's own
  README guiding rule) — each new module needs its own real test suite
  before the next thing is layered on top. This discipline is why the
  existing crates all have solid test coverage; don't break the pattern.
- Check [`DECISIONS.md`](./DECISIONS.md) before re-deciding something that
  was already decided during implementation (e.g. why there are two
  differently-scoped `AuditSink` traits, why `reqwest`/`wiremock` were
  chosen, why JSONL instead of SQLite, why config is TOML/JSON not YAML).
