# Presence Adaptive Scenes — Development Plan

**Project**: Ralleh Assistant (`ralleh-ai/assistant`)
**Companion to**: [`PRESENCE_SCENES.md`](./PRESENCE_SCENES.md) (scene/entity/director
architecture), [`PRESENCE_VISUAL_ENTITY.md`](./PRESENCE_VISUAL_ENTITY.md) (visual
state system), [`PRESENCE_INTEGRATION_PLAN.md`](./PRESENCE_INTEGRATION_PLAN.md)
(phasing / process model).
**Builds on**: [ADR-011](./adr/adr-011-surface-point-generation-and-palette-setting.md)
(surface points + palette-as-setting), [ADR-012](./adr/adr-012-additive-mode-composition.md)
(additive term composition), [ADR-013](./adr/adr-013-presence-window-and-process-model.md)
(window/process model).
**Status**: Design complete → not yet implemented. This document is the
`ARCHITECTURE.md`-style implementation plan for the feature.
**Last Updated**: 2026-08-03

---

## 1. Concept

Today the presence renders a fixed vocabulary: one always-on `AssistantCloud`
shell whose additive terms express `listening` / `thinking` / `speaking` /
`tool_use` / `attention` / `error`, plus a toggleable `LoadingRing`. Scenes are
declared statically in a registry and constructed by hand in the Scene Director.

This plan extends that into an **adaptive scene system**: a small, fast LLM
watches the assistant's outgoing responses (and background work signals),
decides whether an ambient scene would add genuine visual context, and
**selects** a registered scene — or, when none fits, **composes** a new one from
a bounded primitive grammar. Users can also **build scenes on demand**, **refine**
them conversationally, **save** the ones they like, and **delete** the ones they
don't. Saved scenes are scoped to the operator's identity (`tenant · device ·
actor`), so each install grows its own visual vocabulary.

The load-bearing rule, preserved end to end:

> **The LLM never emits geometry or shader code.** It selects a registered
> scene id, or composes a declarative `SceneSpec` from an allow-listed set of
> engine primitives. The grammar is the safety boundary *and* the consistency
> standard; a `SceneSpec` that validates is renderable, in-range,
> palette-coherent, and budget-bounded by construction.

Three entry points, one pipeline:

1. **Automatic** — the classifier decides `present` / `abstain` / `compose`.
2. **On-demand** — the user asks (`build_scene`), always previews, no restraint gate.
3. **Curated** — the user saves / renames / pins / deletes (registry CRUD).

All three funnel through the **one** `SceneSpec` grammar and the **one**
validator, so every scene however it is born meets the same standard.

**Multiple scenes can be on air at once, each positioned independently** and each
either **overlaid** on the cloud (default) or **replacing** it (crossfade) — so
the presence can, say, keep thinking in the center while rain drifts in a corner.
The composition model (disposition, placement, z-order, global budget) is D-2 / §3.0.

---

## 2. Current-state audit (what we are refactoring)

| Area | File(s) | Today | Gap for this feature |
|---|---|---|---|
| Scene catalog | `presence-core/src/scene/registry.rs` | `SceneDescriptor` metadata in a `HashMap<&'static str, _>`; builtins only; **descriptors, not factories** | Needs runtime instantiation from data, mutation (add/remove), and per-tenant scoping |
| Scene construction | `presence-core/src/scene/director.rs` | Two entities hardcoded in `SceneDirector::new`; `builtins_match_the_scene_director` test pins them | Needs to build entities from a registry/spec; hold a **stack of N positioned scenes** (overlay/replace), TTL, global budget, add/remove at runtime |
| Entity model | `presence-core/src/scene/entity.rs` | `EntityInstance` with boxed `PointGenerator`/`PointBehavior`, `active`/`presence` fade | Add TTL + provenance + **disposition + placement** + spec-derived generator/behavior |
| Shapes/terms | `presence-core/src/sim/shapes.rs`, `sim/types.rs` | `PresenceShell` (fold/lobes/pulse/neck), `ResonancePlate`; additive `ShellDrive` terms with a `GATE` | Extract a **primitive term vocabulary** a `SceneSpec` can reference |
| Transport | `crates/presence-ipc/src/lib.rs` | Versioned NDJSON `Envelope`; `set_signals` / `set_mode` / `set_palette` etc. | Add `present_scene` / `dismiss_scene` / `build_scene` / `delete_scene` command variants; bump `VERSION` |
| Shell bridge | `desktop-edge/src-tauri/src/presence.rs` | Spawns runtime, sends commands, reads events (capped lines) | Add Tauri commands for the new verbs + capability allowlist entries |
| Model access | `crates/ralleh-ai-router/` | Policy-gated completion routing | Add a classify/compose call path + the `present_scene`/`compose_scene` tools |
| Policy | `crates/ralleh-policy-core/` | Deny-by-default decisions | Add a "may this content drive a scene" decision + rate rules |
| Audit | `crates/ralleh-audit-store/` | Hash-chained JSONL | Record every scene *event* (decision, compose, save, delete) — unchanged |
| Scene store | *(new)* | — | Small **searchable** per-tenant store (SQLite+FTS via `rusqlite`) for scene *state*: lookup/search/dedup (D-4) |
| Settings | `desktop-edge/src/settings.ts`, Rust `EdgeSettings` | tenant/device/actor/palette/style | Add `ambientScenes` on/off + per-tenant scene library location |

**Design constraints inherited (do not regress):**

- Renderer speaks **point-cloud surfaces only** (ADR-011). No MSAA, no depth
  buffer, additive blending, HDR + bloom.
- Modes/terms **compose additively**; below `ShellDrive::GATE` a term is skipped
  (ADR-012). New primitives must honor the same gate to keep idle cheap.
- Palette is a **user setting**, never chosen by the model (ADR-011).
- Every new IPC command is **versioned** and added to the Tauri **capability
  allowlist** (threat model T11 / finding H5).
- Presence → shell and shell → presence stay **bounded** (line caps, channel
  caps — finding H6).

---

## 3. Target architecture

```
 assistant response stream ─┐
 background work signals  ──┤→  Scene Director service  (new crate: ralleh-scene-director)
                            │      ├─ debounce · cooldown · rate cap
                            │      ├─ policy-core gate ("may content drive a scene")
                            │      ├─ fast model via AiRouter
                            │      │     • present_scene(decision, scene_id, params, confidence, ttl)
                            │      │     • compose_scene() → SceneSpec           (only on `compose`)
                            │      └─ SceneIntent
                            ▼
                     Deterministic validator  (schema · allowlist · clamp · budget · palette · dedup)
                            ▼
        presence_ipc::Envelope { PresentScene | DismissScene | ... }   (versioned)
                            ▼
     presence-runtime → SceneDirector: realize (select template or SpecGenerator/SpecBehavior),
                                        add to the live stack at its placement + disposition
                                        (overlay/replace), fade in, auto-dismiss on TTL
                            ▼
        tenant scene registry (small searchable store: SQLite+FTS)
                     ←→  save / delete / pin / search / suppression
```

### 3.0 Composition model — overlay + replace, multi-scene, positioned (D-2)

The director holds a **stack of live entities** (the always-present cloud plus
zero or more scenes) and composites them into one additive draw. Four mechanics:

**Disposition (per entity).**
- `Overlay` (default): the scene fades in *alongside* the cloud. The cloud (and
  any other lower-priority entities) are pulled down the attention hierarchy via
  the existing subdue path (`SUBDUED_PRESENCE` / `LOADING_ACTIVITY_SCALE` in
  `director.rs`), so the composite stays legible instead of summing into a blob.
- `Replace`: the cloud's target `presence → 0` while the scene fades `0 → 1` over
  `TRANSITION_SECONDS` — a true crossfade; the cloud returns on dismiss/TTL.

**Placement (per entity).** A `Placement { anchor, offset, scale }` drives the
entity's `EntityParams.center`/`scale` (which already exist), so scenes can be
positioned independently — e.g. anchored to a window corner, offset from center,
or scaled down to share space. Anchors are resolved against the current window
(or droplet) extent so layout is stable across window sizes. Overlapping
placements are allowed (additive blend handles it); spatial separation is the
main tool for keeping simultaneous scenes readable.

**Z-order / attention hierarchy.** `priority` (already on `EntityInstance`)
orders the stack: higher-priority entities render "on top" for tint/crease
salience and push lower ones further into the subdued tier. The cloud sits at a
mid priority so an `error`/`speaking` preemption can lift it back above ambient
overlays.

**Global budget.** The QualityTier point ceiling applies to the **sum** of all
live entities, not per-entity. The director allocates the budget across the
stack (cloud gets the floor it needs; remaining split across scenes by priority)
and re-generates at a lower per-scene budget when many scenes are live, so N
simultaneous scenes never exceed the frame budget (see §5).

`LoadingRing` is simply a builtin `Overlay` entity under this model — no special
case.

### 3.1 Core data types (new)

```rust
/// The declarative, engine-realizable description of a scene. The ONLY thing
/// the model is allowed to "create". Anything expressible here is renderable,
/// in-range, palette-coherent, and budget-bounded.
struct SceneSpec {
    base: BaseSurface,          // enum: Shell | Plate | Ring | Column
    terms: Vec<SceneTerm>,      // 1..=MAX_TERMS, each an allow-listed primitive
    motion: MotionProfile,      // time_scale, spring_hz (clamped)
    palette_role: PaletteRole,  // Neutral | Cool | Warm | Accent — maps to active PaletteId
    point_budget: usize,        // clamped; counts against the GLOBAL tier ceiling (§3.0)
    disposition: Disposition,   // Overlay (default) | Replace  (D-2)
    placement: Placement,       // anchor + offset + scale → EntityParams.center/scale (D-2)
    ttl: Option<Duration>,      // None = persistent (saved), Some = ephemeral
    provenance: Provenance,     // intent text, confidence, source (auto|ondemand|user)
}

/// Where a scene sits. Anchors resolve against the current window/droplet
/// extent so layout is stable across sizes; overlapping placements are allowed.
struct Placement {
    anchor: Anchor,             // Center | TopLeft | TopRight | BottomLeft | BottomRight | CloudRelative
    offset: Vec2,               // clamped to the visible extent
    scale: f32,                 // clamped to [SCENE_MIN_SCALE, 1.0]
}

/// One additive primitive. `kind` is an allow-listed enum; each carries a
/// typed, range-bounded param set (mirrors ShellDrive's per-term weights).
enum SceneTerm {
    Precipitation { density: f32, direction: Vec3, wind: f32 },
    Wave { amplitude: f32, hz: f32 },
    Lobe { count: u8, rise: f32 },
    Drift { amount: f32 },
    // …grows by review, never by the model
}

/// What the classifier emits and the validator consumes.
struct SceneIntent {
    decision: Decision,         // Present | Abstain | Compose
    target: SceneTarget,        // registered id  OR  a SceneSpec
    confidence: f32,
    ttl: Duration,
}
```

### 3.2 Model-facing tool contract (the standardizer)

`present_scene`'s `scene_id` **enum is generated from the live registry**, so
the model can never name a scene that does not exist. `compose_scene` returns a
`SceneSpec` whose `terms[].kind` enum is the **primitive allowlist**. See §7 for
the full schema, system prompt, and QA gate.

### 3.3 Where the model call lives

Orchestration is a **new crate `ralleh-scene-director`**; the model call goes
**through `AiRouter`** (policy-gated, audited, backend-swappable — hosted fast
model now, local model later, per the product direction). The runtime never
calls a model; it only receives validated IPC commands.

---

## 4. Refactor task list

Tasks are grouped by phase. Each phase is independently shippable and leaves the
system in a working state. IDs are stable references for commits/PRs.

### Phase 0 — Enabling refactor (no LLM) ⟶ *prove the plumbing*

**Goal:** registry becomes factory-driven; director builds N ephemeral overlay
scenes with TTL; one hand-built demo scene driven over IPC. Zero AI.

- [ ] **T-0.1** `registry.rs`: extend `SceneDescriptor` → `SceneTemplate` with a
  `build(&self, params, tier) -> EntityInstance` factory and a `param_schema`.
  Keep `id`/`label`/`summary`/`priority`/`default_active`.
- [ ] **T-0.2** `director.rs`: replace the two hardcoded entities with
  construction **from the registry**. Preserve `builtins_match_the_scene_director`
  by asserting registry ↔ built set.
- [ ] **T-0.3** `director.rs`: hold a **live stack** of dynamic entities
  (`MAX_LIVE_SCENES`), each with a per-entity **disposition** — `Overlay` (default;
  subdue/priority path, retained for `LoadingRing`) or `Replace` (crossfade: cloud
  `presence → 0` while the scene fades in; return on dismiss/TTL — D-2 §3.0). Add
  the preemption rule (`error`/`speaking` reclaim the cloud / demote overlays).
- [ ] **T-0.3a** `entity.rs`/`director.rs`: add `Placement { anchor, offset, scale }`
  driving `EntityParams.center`/`scale`; resolve anchors against the current
  window/droplet extent so multiple scenes can be positioned independently (D-2 §3.0).
- [ ] **T-0.3b** `director.rs`: **global point-budget allocator** — the QualityTier
  ceiling applies to the sum of all live entities; split across the stack by
  priority and re-generate scenes at a lower per-scene budget as more go live, so
  N simultaneous scenes never exceed the frame budget (§5).
- [ ] **T-0.4** `entity.rs`: add `ttl: Option<Duration>`, `spawned_at`, and
  `provenance` to `EntityInstance`; director auto-dismisses (fade → remove) on
  TTL expiry.
- [ ] **T-0.5** `entity.rs`/`director.rs`: implement add-scene / remove-scene at
  runtime (bounded: `MAX_LIVE_SCENES`), with graceful fade-out on removal of a
  live scene. Multiple scenes may be live at once (the stack from T-0.3).
- [ ] **T-0.6** `presence-ipc/src/lib.rs`: add `PresentScene { id, params,
  disposition, placement, transition, ttl }` (disposition + placement per D-2) and
  `DismissScene { id }` command variants; **bump `VERSION`**; keep the
  min-supported-version range check.
- [ ] **T-0.7** `presence-runtime`: wire the new commands into
  `SceneDirector::apply_command` (behind the `ipc` feature).
- [ ] **T-0.8** `desktop-edge/src-tauri/src/presence.rs` + `build.rs` +
  `capabilities/default.json`: add `presence_present_scene` / `presence_dismiss_scene`
  Tauri commands; declare + allowlist them.
- [ ] **T-0.9** Author **one** hand-built demo template (`precipitation`, rain
  variant) as a `SceneTemplate` in the registry.
- [ ] **T-0.10** Tests: deterministic generator test for the demo template; IPC
  round-trip test (incl. disposition + placement); TTL auto-dismiss test;
  `MAX_LIVE_SCENES` cap test; global-budget allocation test (sum ≤ tier ceiling
  with N live scenes); anchor/placement resolution test across window sizes.
- [ ] **T-0.11** Manual verify: drive `PresentScene`/`DismissScene` over stdin
  IPC; capture (a) an overlay scene positioned in a corner beside the cloud and
  (b) a replace scene crossfading the cloud, plus idle→scene(s)→idle.

### Phase 1 — Curated scene template library

**Goal:** a small, uniform set of hand-built templates; manual selection only.

- [ ] **T-1.1** Extract the **primitive term vocabulary** from `sim/shapes.rs`
  (`Precipitation`, `Wave`, `Lobe`, `Drift`, …) as reusable `SceneTerm`
  implementations honoring `ShellDrive::GATE`.
- [ ] **T-1.2** Author 3–5 templates: `precipitation`, `celebration`, `alert`,
  `dataflow`, `calm`. Each: param schema + defaults, palette bindings, bounded
  budget, reduced-motion behavior.
- [ ] **T-1.3** Per template: deterministic generator test, a screenshot, and a
  one-line "when to use" summary (feeds the model prompt in Phase 2).
- [ ] **T-1.4** Template authoring checklist added to this doc's §8 and enforced
  in review (nothing enters the enum without it).
- [ ] **T-1.5** Debug-panel affordance to select any template (dev-only).

### Phase 2 — Scene Director LLM (classify: present / abstain)

**Goal:** automatic, restraint-gated scene selection from the curated library.

- [ ] **T-2.1** New crate `ralleh-scene-director`: async service consuming
  finalized responses (tap the `assistant_think` stream) + work signals.
- [ ] **T-2.2** `present_scene` tool: JSON schema with `scene_id` enum
  **generated from the registry**; system prompt (see §7).
- [ ] **T-2.3** Model call through `AiRouter` (fastest/lowest-cost hosted model;
  pluggable for a future local model).
- [ ] **T-2.4** Deterministic validator: schema-validate → registry allowlist →
  **clamp** params → confidence threshold → **policy-core gate** → debounce /
  cooldown / rate cap → reduced-motion + `ambientScenes` modifiers → force
  active palette.
- [ ] **T-2.5** Emit validated `SceneIntent` → `PresentScene` IPC.
- [ ] **T-2.6** Audit every decision (present/abstain, scene, confidence,
  rationale) via `ralleh-audit-store`.
- [ ] **T-2.7** Settings: add `ambientScenes` on/off (`EdgeSettings` + settings UI).
- [ ] **T-2.8** Latency budget test + non-blocking guarantee (scene trails text,
  never delays it).

### Phase 2.5 — Compose path + on-demand build

**Goal:** create a scene when none fits, and let the user request one directly.
Cache-only (no persistence yet).

- [ ] **T-2.5.1** `SceneSpec` + `SceneTerm` types (crate-shared); the primitive
  **allowlist** enum.
- [ ] **T-2.5.2** `SpecGenerator` + `SpecBehavior`: deterministic realizer that
  interprets a `SceneSpec` into an `EntityInstance` (no codegen — drives existing
  additive terms from data).
- [ ] **T-2.5.3** `compose_scene` tool + prompt; classifier `Compose` branch.
- [ ] **T-2.5.4** Extend the validator for specs: primitive allowlist, per-term
  clamp, `MAX_TERMS`, density-sum cap, `point_budget` clamp to tier, palette forced.
- [ ] **T-2.5.5** Spec cache keyed by normalized-spec hash (reuse realized scenes;
  avoid re-calling the model / re-generating).
- [ ] **T-2.5.6** On-demand verbs: `build_scene(prompt)` (always previews, skips
  restraint gate) and `refine_scene(patch)` (param edits realize instantly with
  no model round-trip; structural edits re-call `compose_scene`).
- [ ] **T-2.5.7** IPC + Tauri commands + capability allowlist for `build_scene` /
  `refine_scene`.
- [ ] **T-2.5.8** Fail-safe: any spec validation failure degrades to nearest
  registry scene or abstain — never render an invalid/off-brand spec.
- [ ] **T-2.5.9** Tests: spec round-trip, clamp/allowlist enforcement, realizer
  determinism, cache hit/miss.

### Phase 3 — Signal fusion, polish, evaluation

**Goal:** react to *work being done*, not just text; make quality measurable.

- [ ] **T-3.1** Feed non-text signals (tool kind, latency, error/deny events) into
  the director alongside response text.
- [ ] **T-3.2** Offline **eval harness**: golden corpus `response → expected
  {decision, scene_id, param bounds}`; precision-weighted metric (false-present
  penalized); hard-negative suite (opinion/emotional/ambiguous → abstain).
- [ ] **T-3.3** CI gate: model/prompt/template changes must pass the eval set +
  latency budget.
- [ ] **T-3.4** Reduced-motion / `ambientScenes=off` / manual-override polish;
  cooldown + rate-cap tuning against the eval corpus.

### Phase 3.5 — Persistence, per-tenant library, curation (save / delete)

**Goal:** the library grows and personalizes; users curate it.

- [ ] **T-3.5.1** Persisted **per-tenant scene store** — SQLite+FTS via `rusqlite`
  (D-4), keyed by `tenant · device · actor` (from `EdgeSettings`): `scenes(id,
  tenant, name, spec_json, provenance, created_at, last_used_at, use_count,
  rating, pinned, deleted_at)` + FTS on name/intent (+ optional embedding column).
  Bounded size with LRU eviction of unpinned, unrated entries.
- [ ] **T-3.5.2** `save_scene(name)`: promote a preview/spec → tenant store
  (dedup **warns**, doesn't block on explicit save); generate id + screenshot +
  eval stub; sets an explicit positive rating.
- [ ] **T-3.5.3** Auto-promotion loop (D-5): promote a cached spec to a named
  template only when **both** hold — recurrence `use_count ≥ N` (config) **and** a
  positive explicit `rating`. Dedup against existing templates (param + embedding
  similarity via the FTS/embedding index).
- [ ] **T-3.5.4** `delete_scene(id)`: soft-delete + audit + short undo window →
  hard purge. **Builtins protected** (idle/loading undeletable). Live scene →
  fade to idle first.
- [ ] **T-3.5.5** Rejection signal: deleting an auto-promoted scene adds it to a
  per-tenant **suppression list** so the promotion loop won't re-mint it.
- [ ] **T-3.5.6** Curation + search verbs: `list_scenes` / `search_scenes(query)`
  (FTS/similarity over the tenant store — also backs the classifier's "is there
  already a scene for this?" check) / `rename_scene` / `pin_scene` /
  `disable_scene` (IPC + Tauri + capability + policy gate).
- [ ] **T-3.5.7** Minimal scenes-management UI in the shell (list, preview,
  rename, pin, delete) — first user-facing surface beyond the dev panel.
- [ ] **T-3.5.8** Tests: promotion/dedup, suppression, undo, builtin-protection,
  per-tenant isolation, eviction ceiling.

### Phase 4 — Stretch

- [ ] **T-4.1** Richer primitive vocabulary (more `SceneTerm` kinds, reviewed).
- [ ] **T-4.2** User-authored / hand-edited `SceneSpec`s (a spec editor).
- [ ] **T-4.3** Optional **local** classify/compose model behind the same
  `AiRouter` tool interface.
- [ ] **T-4.4** Shared/global vetted scene tier above the per-tenant libraries.

---

## 5. Cost & performance budget (reference)

Grounded in the actual structs (`Particle` = 84 B, `InstanceRaw` = 48 B):

- **Resident per scene:** ~132 B/point (84 CPU + 48 GPU/staging). A 30k-point
  composed scene ≈ ~4 MB.
- **Create:** one `budget × 84 B` allocation + O(N) generate (~1–5 ms for
  ≤80k); GPU buffer grows once if capacity exceeded.
- **Maintain:** ¼N staggered noise evals + one N×48 B upload per frame; full
  pipeline ~6 ms/frame (≈150+ fps) at 80k + bloom on a 2-core box.
- **Decision latency (hosted fast model):** ~0.3–1.2 s decision + <5 ms local
  realize + 0.7 s fade. Fully async — the text answer never waits. Local model
  later: ~50–300 ms decision.

**Multiple simultaneous scenes** share the **global** tier budget (§3.0), so they
don't add cost linearly: the director splits the same ceiling across the live
stack (each scene generated at a smaller per-scene budget as more go live). E.g.
an 80k ceiling might be cloud 50k + two scenes at 15k each — same ~6 ms/frame,
just distributed. Placement is free (a transform on existing `center`/`scale`).

Implication: showing several positioned adaptive scenes stays within the frame
budget by construction; the only meaningful latency is the model call, and it is
non-blocking.

---

## 6. Guardrails (must hold across all phases)

- **Grammar is the boundary** — model selects an id or composes a `SceneSpec`
  from allow-listed primitives; never geometry/shaders/raw math.
- **Validator is load-bearing** — schema + allowlist + clamp + budget + palette +
  policy run on *every* intent regardless of source; bad output is made
  impossible, not merely discouraged.
- **Restraint** — automatic path defaults to `abstain`; confidence threshold +
  debounce + cooldown + rate cap + reduced-motion + user off-switch.
- **Privacy** — prefer a local model; the model call is policy-gated and audited;
  content that policy flags cannot drive a scene.
- **Bounded everything** — `MAX_TERMS`, density-sum cap, **global** tier budget
  cap across all live entities (§3.0), `MAX_LIVE_SCENES`, clamped placement
  (offset within extent, `scale ≤ 1.0`), per-tenant library ceiling + LRU.
- **Reversible & attributable** — compose/save/delete all audited; delete is
  soft with an undo window; builtins protected.
- **Security parity** — every new IPC command versioned; every new Tauri command
  in the capability allowlist (H5); shell↔runtime stays bounded (H6).

---

## 7. Standardization: prompt & QA (summary)

The consistency guarantee is two layers — the prompt makes good output *likely*,
the schema + validator make bad output *impossible*.

- **Tool contract** — `present_scene(decision, scene_id?, params?, confidence,
  ttl, rationale)` with `scene_id` enum generated from the registry;
  `compose_scene() → SceneSpec` with `terms[].kind` from the primitive allowlist.
  Per-template/param sub-schemas carry ranges + defaults.
- **System prompt** — role & single job; **bias to abstain**; the registry-
  injected scene catalog (label/summary/when-to-use); few-shot mapping rules with
  hard negatives; parameter discipline (degree from the text, not drama); honest
  confidence/TTL; exactly one tool call.
- **Deterministic QA gate** — schema-validate → registry/primitive allowlist →
  clamp → confidence threshold → policy gate → rate/cooldown → reduced-motion /
  off modifiers → force palette → dedup.
- **Offline eval set** — golden corpus in CI, precision-weighted, hard-negative
  suite, param-sanity, latency budget; gates model/prompt/template changes.

---

## 8. Template & primitive authoring checklist

Nothing enters the scene enum or the primitive allowlist without:

- [ ] Param schema with explicit ranges **and** defaults.
- [ ] Palette bindings only (no hardcoded color); honors active `PaletteId`.
- [ ] Bounded point budget; honors `ShellDrive::GATE` so idle stays cheap.
- [ ] Reduced-motion behavior defined.
- [ ] Deterministic generator/realizer test.
- [ ] A screenshot (via `presence-prototype/capture.ps1`).
- [ ] ≥2 eval cases (one positive, one hard-negative).

---

## 9. Resolved decisions (2026-08-03)

### D-1 Model choice — start hosted, route via `AiRouter`, local later

Selection criterion is non-negotiable: the model **must** support native
**structured output / tool-calling**, because the entire QA guarantee rests on
the constrained `present_scene` / `compose_scene` schema. Research (pricing +
latency, 2026) narrows it to three viable tiers:

| Role | Recommended | Price (in/out per 1M) | Latency | Why |
|---|---|---|---|---|
| **Classify** (primary) | **Gemini 2.5 Flash-Lite** | $0.10 / $0.40 | TTFT <600 ms | Cheapest major model with solid quality + native function calling; ~90% cache discount on the static system prompt + scene catalog, 50% batch |
| **Classify** (speed-critical alt) | **Groq Llama 3.3 8B** | ~$0.05 in | **TTFT ~0.15 s, ~0.43 s total** | Fastest + cheapest, deterministic structured output on LPU; Llama-only, slightly weaker on nuance |
| **Compose** (rarer, more reasoning) | **Gemini 2.5 Flash** | $0.30 / $2.50 | TTFT <600 ms | Step up only on the `compose` branch where a `SceneSpec` is synthesized |

Ruled out: DeepSeek V4 Flash (cheap output but ~2 s TTFT — too slow for an
interactive scene). OpenAI `gpt-5-nano`/`gpt-4.1-nano` ($0.05–0.10 / $0.40) are
an equivalent-tier fallback if we standardize on the OpenAI ecosystem.

**Cost is negligible:** a classify call is ~300 input tokens (mostly a cached
static prompt) + ~60 output → **~$0.00003–0.00005/call**; even 10k responses/day
is well under **$1/day**. Wire all calls through `AiRouter` so provider is a
config choice and we can A/B. **Local milestone (T-4.3):** a small local model
(Gemma-4-class or a DeepSeek distill via llama.cpp/Ollama) behind the same tool
interface — targets ~50–300 ms decision with zero egress.

### D-2 Composition model — **both overlay and replace; multiple positioned scenes**

The director supports a **stack of simultaneously-visible entities**, each with:

- a **disposition** — `Overlay` (**default**: composite alongside the cloud,
  which is subdued via the existing attention-hierarchy path) or `Replace`
  (crossfade: cloud `presence → 0` while the scene fades in, cloud returns on
  dismiss/TTL); and
- a **placement** — an anchor + offset + scale so scenes can sit in *different*
  positions rather than all stacking on the cloud's center.

So "rain in the corner while the cloud keeps thinking in the middle" is an
overlay scene with a corner anchor; "the cloud becomes a celebration" is a
replace scene. `LoadingRing` is just a builtin `Overlay` entity under this model.
See §3.0 for the mechanics (fade, layout, z-order, global budget,
attention hierarchy).

**Preemption rule:** a high-salience mode still wins. If `error` (or, by config,
`speaking`) engages, `Replace` scenes fade the cloud back immediately and
`Overlay` scenes are pushed further down the attention hierarchy (dimmed/subdued)
so the assistant's own state is never occluded by ambient context.

### D-3 Trigger scope — text-first (recommended)

Text-only in Phase 2; add background work signals (tool kind, latency,
error/deny) in Phase 3 (T-3.1). Keeps the first classifier surface small and the
eval corpus tractable.

### D-4 Persistence — small **searchable** embedded store

The per-tenant scene library uses a small structured store we can **look up and
search** (recommend **SQLite via `rusqlite`**), not append-only JSONL:
`scenes(id, tenant, name, spec_json, provenance, created_at, last_used_at,
use_count, rating, pinned, deleted_at)` plus an **FTS index** on name/intent (and
room for an optional embedding column for similarity dedup/search). The
hash-chained JSONL **audit** log is unchanged — it records *events* (compose,
save, delete); the SQLite store holds *queryable state*. Two stores, two jobs.

### D-5 Promotion threshold — recurrence count **and** explicit rating

An auto-composed scene is promoted to a named template only when **both** signals
agree: it has recurred ≥ N times (config, default small) **and** carries a
positive explicit rating (a user save/thumbs-up). Recurrence alone risks
promoting a frequent-but-unloved scene; rating alone promotes one-offs. Requiring
both keeps the library high-signal.

---

## 10. Definition of done (per phase)

- **P0**: registry is factory-driven; N TTL overlays; rain demo over IPC; tests +
  screenshots; security parity (versioned IPC, allowlisted commands).
- **P1**: 3–5 curated templates with schemas/tests/screenshots; dev selection.
- **P2**: automatic classify → present/abstain, restraint-gated, audited,
  `ambientScenes` setting; latency budget met; non-blocking.
- **P2.5**: compose path + `SpecGenerator` realizer + `build_scene`/`refine_scene`;
  cache-only; fail-safe degrade; tests.
- **P3**: work-signal fusion; eval harness in CI; polish.
- **P3.5**: per-tenant **searchable** store (SQLite+FTS); save/delete/curation +
  search; auto-promotion (recurrence **and** rating) + dedup + suppression;
  scenes-management UI; tests.
