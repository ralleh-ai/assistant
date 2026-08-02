# Crate Naming — Plan (§16) vs This Repo

Reconciliation of DEVELOPMENT.md §16 repository names with the crates that
actually exist. **No mass rename** until a NestJS control plane / Tauri app
forces a shared package boundary — renames now would churn git history and
path deps for no runtime win.

## Mapping

| DEVELOPMENT.md §16 (plan) | This workspace (actual) | Notes |
|---|---|---|
| `ralleh-core/` | *(split)* `ralleh-audio-core` (+ future intent/echo crates) | Plan bundled audio+intent; we extracted audio first |
| `policy-core/` | `ralleh-policy-core` | `ralleh-` prefix for crates.io / workspace clarity |
| `mcp-gateway/` | `ralleh-tool-gateway` | Named for the broker role; HTTP face is `ralleh-mcp-server` |
| `ai-router/` | `ralleh-ai-router` | Prefixed consistently |
| `audit-core/` | `ralleh-audit-store` | “store” reflects JSONL persistence, not only ingestion |
| `memory-core/` | *(not started)* | Phase 3 — keep plan name when introduced |
| `desktop-edge/` | `desktop-edge/` (Tauri; not a workspace member) | Own Cargo.toml under `src-tauri/` |

| `apps/control-plane/` (NestJS) | *(not started)* | Separate package / repo layout later |

## Rules going forward

1. **New Rust crates** use the `ralleh-` prefix and live under `crates/`.
2. Prefer **splitting** plan mega-crates (`ralleh-core`) over forcing one
   crate to match the doc name.
3. When NestJS or Tauri lands, add an explicit “published name / import
   path” row here — do not silently rename existing crates mid-feature.
4. Docs may say “policy-core” as shorthand; Cargo package names stay
   `ralleh-policy-core`.

## Why not rename now

Path dependency updates across the workspace + every agent/doc reference
are high churn for zero behavior change. Headless CI already pins
`Cargo.lock` against current names. Revisit only if an external consumer
needs the §16 names as public package IDs.
