# ADR-004: Rust Policy Engine Core, Policy-Mediated Tool Execution

**Status:** Accepted (planning-time decision, copied from DEVELOPMENT.md §20) — **implemented**

## Decision

No tool executes without schema validation and policy authorization,
evaluated by a shared Rust policy core (edge + control plane), not a
TypeScript policy module.

## Reason

Enterprise trust depends on predictable governance, auditability, and
least privilege; putting policy evaluation in the same Rust crate family
as the tool gateway and audit ingestion keeps the security spine in one
consistent, high-performance, memory-safe layer instead of splitting it
awkwardly across Node and native code.

## Implementation status

**Fully implemented** and is the most mature part of this repository:

- `ralleh-policy-core` — the policy engine itself (rules, decisions,
  default-deny, tenant/device/actor/capability-prefix/sensitivity
  matching, first-match-wins semantics). 21 tests.
- `ralleh-tool-gateway::ToolGateway::dispatch` — the actual chokepoint;
  every tool call is policy-evaluated before a handler is ever invoked, and
  a `GatewayEvent` is produced (and persisted, per ADR-006) regardless of
  outcome.
- `ralleh-ai-router::AiRouter::complete` — the same discipline applied to
  AI completion requests, not just tool calls: policy-gated before the
  backend is ever called.

See [`../ARCHITECTURE.md`](../ARCHITECTURE.md) for full detail on how
`ralleh-policy-core` and `ralleh-tool-gateway` work together.

**Known gap:** policy rules are currently hardcoded in
`ralleh-mcp-server`'s `main.rs`, not loaded from a declarative config file
as this ADR's reasoning (and DEVELOPMENT.md §8.3) implies they eventually
should be. See [`../NEXT_STEPS.md`](../NEXT_STEPS.md) item 1.
