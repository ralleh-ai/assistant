# ADR-006: Audit Persistence via Append-Only JSONL, Not a Database

**Status:** Accepted — **implemented**

## Decision

`ralleh-audit-store`'s production sink (`JsonlFileAuditSink`) writes
audit records as one JSON object per line to a plain append-only file,
rather than persisting to SQLite/Postgres via `sqlx` or any other database
dependency.

## Reason

The build environment had constrained resources (~1.9GB free RAM, 2 CPU
cores). A database dependency — even an embedded one like SQLite — was
judged unnecessary weight for the immediate need: durably persist an
append-only audit trail so `GatewayEvent`/`PolicyDecision` records are no
longer computed and discarded. `JsonlFileAuditSink` is:

- Append-only (matches the audit-log use case — records are never
  updated/deleted through this interface).
- Flushed on every write (no buffering that could lose records on crash).
- Mutex-serialized so concurrent writers can't interleave partial lines
  (proved with an 8-thread × 25-write concurrency test).

The `AuditSink` trait is the deliberate swap seam: a Postgres-backed
implementation can be added later without touching `ralleh-tool-gateway`
or `ralleh-mcp-server`, since they only ever depend on the trait, never
the concrete type.

## Consequences / when to revisit

DEVELOPMENT.md's data model treats `AuditEvent` as a first-class,
queryable entity (§13) — a flat JSONL file doesn't support efficient
querying, indexing, or multi-consumer access well at scale. This is
explicitly *not* meant to be the permanent answer for a multi-tenant
enterprise deployment. Revisit once Phase 2 (control plane / Postgres,
per DEVELOPMENT.md §15) work actually starts — and re-check whether the
resource-constraint premise still holds in whatever environment that work
happens in before assuming a database is now warranted.
