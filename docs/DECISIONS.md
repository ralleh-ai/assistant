# Implementation Decisions

Decisions made *while building* the current Rust workspace — distinct from
the product/architecture ADRs already recorded in
[`DEVELOPMENT.md`](./DEVELOPMENT.md) §20 (which are planning-time
decisions). These are implementation-time calls, the kind a new engineer or
agent would otherwise have to rediscover by reading git history or asking
"why is it built this way?"

## Audit persistence: JSONL file, not a database

**Decision:** `ralleh-audit-store`'s real sink (`JsonlFileAuditSink`) is a
plain append-only JSONL file, not SQLite/Postgres/sqlx.

**Why:** the host this was built on has limited resources (~1.9GB free RAM,
2 cores). A DB dependency (even embedded SQLite via sqlx) was judged
unnecessary weight for what's currently just "durably persist an
audit trail." The `AuditSink` trait is the swap seam — a Postgres-backed
implementation can be added later without touching `ralleh-tool-gateway` or
`ralleh-mcp-server` at all, since they only depend on the trait.

**Caveat:** this is explicitly *not* meant to be the permanent answer for a
multi-tenant enterprise deployment (DEVELOPMENT.md's data model assumes
`AuditEvent` is a first-class, queryable entity — a flat file doesn't
support that well at scale). Revisit once Phase 2 (control plane / Postgres)
work actually starts.

## Two different `AuditSink` traits, not one

**Decision:** `ralleh-tool-gateway::gateway::AuditSink` (infallible,
`fn record(&self, event: &GatewayEvent)`) and
`ralleh-audit-store::sink::AuditSink` (fallible,
`fn record(&self, record: &AuditRecord) -> Result<(), AuditSinkError>`) are
two distinct traits with the same name, not one shared trait.

**Why:** keeps the crate dependency graph one-way —
`ralleh-tool-gateway` must never depend on `ralleh-audit-store` (it's a
lower-level crate; audit persistence is a concern layered on top of it, not
the other way around). `ralleh-tool-gateway` defines its own minimal,
infallible trait for the one thing it actually needs (never let an audit
outage break dispatch). `ralleh-audit-store` implements *both* traits for
`JsonlFileAuditSink`, bridging them internally. Anyone confused by the
apparent duplication should read this as "the tool gateway's opinion of
what an audit sink needs to do" vs. "the audit store's opinion of what a
sink needs to do" — they're allowed to differ, and did.

## `pub mod gateway` instead of curated re-exports

**Decision:** `ralleh-tool-gateway/src/lib.rs` uses `pub mod gateway;`
(the whole module is public) rather than only re-exporting specific items.

**Why:** `ralleh-audit-store` needs to reference
`ralleh_tool_gateway::gateway::AuditSink` directly to implement it. A
curated `pub use gateway::{AuditSink, ToolGateway};` re-export wasn't
sufficient once other crates needed to name the trait's fully-qualified
path for `impl SomeTrait for X` syntax. This was a deliberate, if slightly
looser-than-ideal, visibility call — revisit if the module surface grows
enough that hiding internals becomes worth the friction.

## `reqwest` + `wiremock` added as dependencies

**Decision:** `ralleh-ai-router` gained `reqwest` (with `rustls-tls`, no
default features) as a real dependency, and `wiremock` as a dev-dependency,
specifically to build `HttpCompletionBackend`.

**Why:** no existing HTTP client dependency in the workspace was suitable —
`axum`/`hyper` (already present transitively via `ralleh-mcp-server`) are
server-side, not client-side, and `ralleh-ai-router` doesn't depend on axum
anyway. `reqwest` is the de facto standard Rust HTTP client; `rustls-tls`
avoids an OpenSSL system dependency, which matters on a
resource-constrained/potentially-minimal host. `wiremock` lets the 6
`HttpCompletionBackend` tests run against a real local mock HTTP server
with zero live network calls — faster, deterministic, and works in CI/
sandboxed environments with no egress.

## `CompletionOutcome`/`CompletionRequest`/`CompletionResponse` needed `Deserialize`, not just `Serialize`

**Decision:** added `Deserialize` derives (previously only `Serialize`
existed on some of these types) plus `#[serde(tag = "outcome", rename_all =
"snake_case")]` on `CompletionOutcome`.

**Why:** discovered while wiring audit persistence — `AuditRecord` needs to
round-trip through JSON (write to JSONL, and in principle read back), which
requires `Deserialize` on everything nested inside it, including
`CompletionOutcome`. This was a real, previously-latent gap, not a
stylistic choice — worth flagging in case similar gaps exist elsewhere
(check any type that flows into an audit record for full
serde round-trip support before assuming it's fine).

## Writes require approval; reads don't

**Decision:** in `ralleh-mcp-server`'s hardcoded policy rules,
`tool.fs.read_text` is gated `Allow`, but `tool.fs.write_text` is gated
`RequireApproval` — a materially stricter default.

**Why:** DEVELOPMENT.md §11.1 explicitly calls out "destructive, external,
financial, admin, or reputation-impacting actions require explicit
confirmation." A filesystem write (even sandboxed) is a mutation with
real consequences (data loss via unintended overwrite, disk exhaustion,
etc.) in a way a read isn't. This is deliberately expressed as an actual
policy rule, not left to the handler's own defenses (refuse-overwrite,
root confinement) to carry alone — DEVELOPMENT.md's non-negotiables (§22)
are explicit that policy gating can't be skipped just because a handler
"seems safe enough."

## `sandbox_root` export naming collision

**Decision:** `FsReadTextHandler`'s and `FsWriteTextHandler`'s
`sandbox_root` helper functions are re-exported from `lib.rs` as
`fs_read_sandbox_root` and `fs_write_sandbox_root` respectively, not both
as a bare `sandbox_root`.

**Why:** straightforward naming collision once both handler modules
existed side by side — confirmed via grep that no external code depended
on the old unqualified name before renaming, so this was a safe,
non-breaking rename.
