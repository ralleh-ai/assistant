# ADR-007: Two Distinct `AuditSink` Traits to Preserve One-Way Crate Dependencies

**Status:** Accepted — **implemented**

## Decision

Define two separate traits, both named `AuditSink`, in two different
crates, rather than one shared trait:

- `ralleh_tool_gateway::gateway::AuditSink` — infallible:
  `fn record(&self, event: &GatewayEvent)`.
- `ralleh_audit_store::sink::AuditSink` — fallible:
  `fn record(&self, record: &AuditRecord) -> Result<(), AuditSinkError>`.

`ralleh-audit-store::JsonlFileAuditSink` implements *both*, bridging them
internally (constructs an `AuditRecord::tool_dispatch(event.clone())` and
does a best-effort `eprintln!` on write failure rather than propagating the
error).

## Reason

Keeps the crate dependency graph strictly one-way:
`ralleh-tool-gateway` must never depend on `ralleh-audit-store`.
`ralleh-tool-gateway` is a lower-level crate — the tool dispatch chokepoint
— and audit *persistence* is a concern layered on top of it, not a
prerequisite for it. If there were only one shared `AuditSink` trait
defined in `ralleh-audit-store`, `ralleh-tool-gateway` would have to depend
on `ralleh-audit-store` just to name the trait, inverting the intended
layering.

The infallible/fallible split also reflects a real semantic difference:
`ralleh-tool-gateway`'s dispatch path must never fail *because* an audit
sink failed (an audit outage must not be able to take down tool execution)
— hence infallible at that boundary. `ralleh-audit-store`'s own trait is
fallible because a real sink implementation (writing to a file, eventually
a database) genuinely can fail, and callers of *that* trait directly
(e.g. tests, or future callers persisting `Completion` records) should be
able to observe and handle that.

## Consequences

Anyone extending this needs to know: adding a new privileged-action kind
that needs auditing does **not** mean adding a method to
`ralleh_tool_gateway::gateway::AuditSink`. It means adding a new
`AuditRecordKind` variant in `ralleh-audit-store`, and wiring whatever new
call-site to construct + persist that record through
`ralleh_audit_store::sink::AuditSink` directly (the way `ralleh-ai-router`'s
completion path would, if/when it's wired to audit — see
[`../NEXT_STEPS.md`](../NEXT_STEPS.md), this isn't done yet for
completions, only for tool dispatch).
