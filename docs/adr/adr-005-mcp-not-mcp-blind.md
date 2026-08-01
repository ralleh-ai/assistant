# ADR-005: MCP-Compatible but Not MCP-Blind

**Status:** Accepted (planning-time decision, copied from DEVELOPMENT.md §20) — **not yet implemented**

## Decision

Support MCP but wrap it in a Rust-based enterprise gateway with policy,
audit, redaction, and secret brokering.

## Reason

MCP is powerful but raw MCP servers are not automatically enterprise-safe;
mediating through the same Rust tool-gateway used for first-party
connectors keeps enforcement consistent regardless of tool origin.

## Implementation status

Not started. `ralleh-tool-gateway`'s `ToolGateway`/`ToolHandler`
abstraction (see [`../ARCHITECTURE.md`](../ARCHITECTURE.md)) is the
mediation layer this ADR calls for, and it's already proven with two real
handlers (`FsReadTextHandler`, `FsWriteTextHandler`). No actual MCP
protocol client/server integration exists yet — when it's built, it should
plug in as another `ToolHandler` implementation (or a family of them,
one per external MCP tool), not bypass the gateway.
