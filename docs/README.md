# Ralleh Documentation

This `/docs` folder is the single onboarding point for anyone (human or AI
coding agent) picking up development on this repository after an
environment change. It exists because we are transitioning to a new desktop
development environment and need zero-loss continuity of context, decisions,
and next steps.

**Read order for a new agent/session:**

1. [`PROJECT_OVERVIEW.md`](./PROJECT_OVERVIEW.md) — what Ralleh is, product vision, current implementation status.
2. [`DEVELOPMENT.md`](./DEVELOPMENT.md) — the full product/architecture planning document (source of truth for scope, ADRs, non-negotiables, roadmap).
3. [`ARCHITECTURE.md`](./ARCHITECTURE.md) — how the *current* Rust workspace actually implements the plan (crate-by-crate, what's real vs. stubbed).
4. [`STATUS.md`](./STATUS.md) — exact current state: what's built, tested, and validated as of the last session.
5. [`NEXT_STEPS.md`](./NEXT_STEPS.md) — prioritized backlog for whoever continues this work.
6. [`DECISIONS.md`](./DECISIONS.md) — key engineering decisions made during implementation that aren't in the original DEVELOPMENT.md (i.e., decisions made *while building*, not while planning).
7. [`ENVIRONMENT.md`](./ENVIRONMENT.md) — dev environment constraints, bootstrap instructions, tool versions, host resource notes.
8. [`HEADLESS.md`](./HEADLESS.md) — what is safe without mic/display; desktop opt-in features and env flags.
9. [`CRATE_NAMING.md`](./CRATE_NAMING.md) — DEVELOPMENT.md §16 names vs actual crates; rename policy.
10. [`THREAT_MODEL.md`](./THREAT_MODEL.md) — Phase 0 threat-model draft (+ Tauri/NestJS forward threats).
11. [`adr/`](./adr/) — architecture decision records, both from the original planning doc and any made during implementation.

## Why this exists

The original product/architecture plan (`DEVELOPMENT.md`) lived in a
sibling, undocumented project directory (`../voice-assistant/DEVELOPMENT.md`)
outside this repo, referenced only by a README pointer. That's fragile: a
new environment or a fresh agent session has no guarantee of finding it. This
`/docs` folder copies and consolidates everything into the repo itself so
the plan travels with the code.
