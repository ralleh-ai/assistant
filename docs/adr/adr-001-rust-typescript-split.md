# ADR-001: Polyglot Split — Rust for Hot-Path/Security-Critical, TypeScript for Business Logic

**Status:** Accepted (planning-time decision, copied from DEVELOPMENT.md §20)

## Decision

Use **Rust** as the default for real-time, security-critical, and hot-path
components (edge voice/audio core, policy engine, MCP/tool gateway, AI
router, audit ingestion). Use **TypeScript + NestJS** for business-CRUD
control-plane logic (tenants, RBAC, SSO, billing, workflow definitions,
admin console backend) and the admin/desktop UI shells.

## Reason

Real-time audio (wake detection, echo/interrupt, transcript timing) and the
security spine (policy evaluation, tool brokering, audit) benefit from
Rust's performance, memory safety, and lack of GC pauses — and can share
crates between edge and server. Business logic and enterprise integrations
(OIDC, SaaS SDKs, admin UI) remain faster to build and hire for in
TypeScript/NestJS. This supersedes the earlier "TypeScript primary
language" framing (v0.1 of DEVELOPMENT.md) — Rust is now co-primary,
scoped by where hot-path/security-critical logic lives.

## Implementation status

This repository (`ralleh-ai/assistant`) *is* the Rust half of this split —
all six crates currently in the workspace exist because of this decision.
No TypeScript/NestJS control plane has been started yet (see
[`../NEXT_STEPS.md`](../NEXT_STEPS.md)).
