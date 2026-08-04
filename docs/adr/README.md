# Architecture Decision Records

This folder holds ADRs in two categories:

1. **Planning-time ADRs** (ADR-001 through ADR-005) — copied verbatim from
   [`../DEVELOPMENT.md`](../DEVELOPMENT.md) §20 for convenience, so they're
   discoverable in the conventional `docs/adr/` location without needing to
   go hunting through the full planning doc. The full document remains the
   source of truth if these ever drift.
2. **Implementation-time ADRs** (ADR-006 onward) — decisions made while
   actually building the code, not previously recorded as formal ADRs
   anywhere. These are also summarized less formally in
   [`../DECISIONS.md`](../DECISIONS.md); the ADR files here give them the
   same structured treatment as the planning-time ones for consistency.

| ID | Title | Status |
|---|---|---|
| [ADR-001](./adr-001-rust-typescript-split.md) | Polyglot Split — Rust for Hot-Path/Security-Critical, TypeScript for Business Logic | Accepted (planning) |
| [ADR-002](./adr-002-tauri-desktop-shell.md) | Tauri v2 Desktop Edge Client, Rust-First Core | Accepted (planning, not yet implemented) |
| [ADR-003](./adr-003-rust-native-ai-bindings.md) | Rust-Native AI Bindings Preferred Over Python Sidecars | Accepted (planning, not yet implemented) |
| [ADR-004](./adr-004-rust-policy-engine.md) | Rust Policy Engine Core, Policy-Mediated Tool Execution | Accepted, **implemented** |
| [ADR-005](./adr-005-mcp-not-mcp-blind.md) | MCP-Compatible but Not MCP-Blind | Accepted (planning, not yet implemented) |
| [ADR-006](./adr-006-audit-jsonl-not-database.md) | Audit Persistence via Append-Only JSONL, Not a Database | Accepted, **implemented** |
| [ADR-007](./adr-007-dual-auditsink-traits.md) | Two Distinct `AuditSink` Traits to Preserve One-Way Crate Dependencies | Accepted, **implemented** |
| [ADR-008](./adr-008-http-completion-backend.md) | OpenAI-Compatible HTTP Wire Format as the First Real Completion Backend | Accepted, **implemented** |
| [ADR-009](./adr-009-anthropic-messages-backend.md) | Anthropic Messages API as Second Completion Backend | Accepted, **implemented** |
| [ADR-010](./adr-010-point-cloud-presence-entity.md) | Point Cloud Presence Entity — Rust-First Renderer (`winit` + `wgpu`), Not Three.js | Accepted (planning; Phase 1 prototype in progress) |
| [ADR-011](./adr-011-surface-point-generation-and-palette-setting.md) | Presence Points Lie On Surfaces, Not Through Volumes — and the Palette Is a User Setting | Accepted, **implemented** (Phase 1 prototype) |
| [ADR-012](./adr-012-additive-mode-composition.md) | Modes Compose Additively On One Shell, Rather Than Selecting Exclusive Shapes | Accepted, **implemented** (Phase 1 prototype) |
| [ADR-013](./adr-013-presence-window-and-process-model.md) | The Presence Runs In Its Own Process, As A Frameless Always-On-Top Droplet | Accepted (decision only; Phase 2 will implement) |
| [ADR-014](./adr-014-presence-engine-architecture.md) | The Presence Is An Engine — Shell-Side Brain Emits Bounded State, A Generic Behavior/Simulation/Render Pipeline Consumes It | Accepted (incremental implementation) |
| [ADR-015](./adr-015-single-continuous-presence.md) | One Continuous Presence — A Scene Is A Blended Parameter State Of A Single Persistent Entity, Not A Stack Of Entities | Proposed (pending review) |
