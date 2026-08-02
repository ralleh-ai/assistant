# Presence Integration Plan — Point Cloud Entity

**Status:** Phase 0 (design) complete. **Phase 1 (standalone Rust
prototype) substantially complete (2026-08-02)** — see
`presence-prototype/` at the repo root. Phase 2's design decisions are
now locked ([ADR-013](./adr/adr-013-presence-window-and-process-model.md))
but the implementation has not started. Phases 3–4 are planning only.

**Companion documents:**
- [`PRESENCE_VISUAL_ENTITY.md`](./PRESENCE_VISUAL_ENTITY.md) — the visual
  design/state-system source of truth (what it looks like, what it means).
- [`PRESENCE_SCENES.md`](./PRESENCE_SCENES.md) — the Scene/Entity/
  Generator/Behavior/Director architecture that concretely realizes the
  modes as renderable visuals, plus the two scenes (Idle, Loading) this
  plan's Phase 1 actually targets.
- [`adr/adr-010-point-cloud-presence-entity.md`](./adr/adr-010-point-cloud-presence-entity.md) —
  the single highest-level architecture decision this plan expands on
  (Rust-first `winit`+`wgpu`, revised from an earlier Three.js/Tauri-webview draft).
- [`adr/adr-011-surface-point-generation-and-palette-setting.md`](./adr/adr-011-surface-point-generation-and-palette-setting.md) —
  points lie on parametric surfaces rather than filling volumes, and the
  palette is a user setting rather than a compile-time constant. Both
  landed during Phase 1 and both change this plan's Phase 1/2/4 content
  below; ADR-010's stack decision is unaffected.
- [`adr/adr-012-additive-mode-composition.md`](./adr/adr-012-additive-mode-composition.md) —
  modes compose as weighted terms on one shell rather than selecting
  exclusive shapes, so `thinking`/`speaking`/`tool_use` moved into Phase 1.
  Also records two constraints the implementation surfaced: the surface
  behavior's spring bandwidth, and why a tool-use pendant cannot detach.

**Revision note:** this plan originally targeted a Three.js renderer
embedded in the `desktop-edge` Tauri webview. That was corrected — the
project's stated direction is to do this work in Rust — and this document
now reflects the Rust-first (`winit`+`wgpu`) path throughout. Anywhere you
see "Phase 1" below, assume it means the standalone Rust prototype, not a
web prototype.

**Last Updated:** 2026-08-02

---

## 1. Why a separate plan doc

`PRESENCE_VISUAL_ENTITY.md` is a design doc — it should stay stable and
mostly about *what the entity is*, the same way `DEVELOPMENT.md` stays the
product/architecture source of truth while `ARCHITECTURE.md` tracks what's
*actually built*. This doc plays the `ARCHITECTURE.md` role for the
presence entity specifically: current-state audit, the decisions needed to
build and eventually integrate it, phasing, dependencies, and
test/threat-model obligations.

## 2. Current-state audit (as of this session)

**Presence-specific code:**
- `presence-prototype/` — new, standalone Cargo project at the repo root
  (Phase 1). Not a workspace member (see §3, D-Workspace below). This is
  where the actual point-cloud rendering/simulation code lives right now.

**What exists in `desktop-edge/` (relevant for later phases, untouched by Phase 1):**
- Tauri v2 + React 19 + TypeScript + Vite. Not a Cargo workspace member
  (see `HEADLESS.md`) — built/run separately via `npm run tauri dev` /
  `scripts\tauri-dev.cmd`.
- One main window, three views swapped via a single crossfade (`App.tsx`):
  `splash` → `settings` → `core`. `Core.tsx` is the post-setup home screen
  — currently a static headline ("Your edge is ready.") plus a settings
  gear; the "calm core placeholder" referenced in `ADR-002` and `NEXT_STEPS.md`.
- Brand tokens live in `App.css` (`--ink`, `--teal`, `--teal-deep`,
  `--mist`, `--foam`, `--amber`) — the palette the prototype's shaders
  reuse (`PRESENCE_VISUAL_ENTITY.md` §3.1).
- `src-tauri/src/` has three narrow, single-purpose IPC modules
  (`mic.rs`, `os_caps.rs`, `settings.rs`), and already depends on
  `ralleh-audio-core` + `ralleh-policy-core` directly as path dependencies
  — the "Rust-first edge core" from ADR-002 is real, not aspirational, for
  audio. It does **not** yet depend on `ralleh-ai-router` or `ralleh-tool-gateway`.
- `NEXT_STEPS.md` #13, "Live mic → VAD → STT path in the shell," is not
  done — today's mic IPC only reports capture metrics (peak RMS, frame
  count), not a running VAD state machine.

**Net:** Phase 1 needs none of the above — it is a fully standalone binary
with synthetic/dev-driven signals (keyboard shortcuts + an `egui` panel),
by design, so it can start immediately. The real prerequisite gap (no live
VAD in the shell, no `ai-router`/`tool-gateway` embedded in `desktop-edge`)
only matters starting Phase 3, and is unchanged by the Rust-first pivot.

## 3. Decisions locked for this plan

Full rationale for the headline decision is in ADR-010; this section
records the supporting decisions at implementation-plan granularity.

### D-Workspace — `presence-prototype/` is a standalone Cargo project, excluded from the main workspace.

New top-level directory `presence-prototype/` (sibling to `desktop-edge/`),
with its own `Cargo.toml` and no `[workspace]` table (same shape as
`desktop-edge/src-tauri/Cargo.toml`). Added to the root `Cargo.toml`'s
`[workspace] exclude` list, next to the existing
`"desktop-edge/src-tauri"` entry, with the same justification: it needs a
GPU/window surface, so it must never be required by headless
`cargo test --workspace` / CI (`HEADLESS.md`'s rule).

### D1 — Render/embedding target: **resolved (2026-08-02) by [ADR-013](./adr/adr-013-presence-window-and-process-model.md)**.

The presence runs in its own OS process, as a frameless transparent
always-on-top droplet, click-through by default. The original deferral is
now settled — see ADR-013 for the four-way pick (process model,
presentation, chrome, settings location) and the rationale. Phase 1
continued as a fully standalone binary; Phase 2 is the implementation of
this decision. What is still open per ADR-013's "Not decided here"
section: the specific IPC transport and encoding, the launch/discovery
model, and multi-monitor placement.

### D2 — Rendering stack: `winit` + `wgpu` + `noise`, in a new Rust crate. Not Three.js.

See ADR-010 for the full reasoning. Concretely, `presence-prototype/Cargo.toml` depends on:
- `winit` — window + event loop
- `wgpu` — GPU rendering (instanced billboarded quads + soft-falloff
  fragment shader; `wgpu` has no portable point-size primitive across
  backends, so billboards are used instead of `PrimitiveTopology::PointList`)
- `noise` — Simplex noise; curl noise is derived from it (finite-difference
  curl of a noise-derived potential field)
- `glam` — `Vec3`/`Mat4` math
- `bytemuck` — safe GPU buffer casts (`Pod`/`Zeroable`)
- `pollster` — blocks on `wgpu`'s async device/adapter init in `main()`
- `egui` + `egui-wgpu` + `egui-winit` — dev-only debug overlay: current
  mode/entities, raw signal values, and buttons/sliders to drive them
  (`PRESENCE_VISUAL_ENTITY.md` §9's "simple debug overlay" requirement)

### D3 — Palette: a user setting, defaulting to brand-reconciled teal (revised — ADR-011).

Recorded in full in `PRESENCE_VISUAL_ENTITY.md` §3.1 and ADR-011. The
brand reconciliation is unchanged and remains the **default**: idle/calm
uses `--foam`/`--teal` at low brightness, thinking/heavy-compute cools
toward `--teal-deep`, error/attention reuses the existing `--amber`
accent, and the teal preset introduces no new hue families. Its values are
derived from the same hex codes in `App.css`, not a second source of truth.

What changed is that this is now one of four selectable presets
(`teal`/`lime`/`ice`/`ember`) carried as runtime data rather than hardcoded
in the shader. The concrete consequences for this plan:

- **Phase 1** proves the plumbing with a live selector in the debug panel.
  This is the one debug-panel control that is not development-only — it is
  a real user setting being exercised early.
- **Phase 2** persists it as `EdgeSettings.presence_palette`, a `String`
  validated against the same fixed list, mirroring how `voice_style` is
  already handled in `desktop-edge/src-tauri/src/settings.rs`. It is
  explicitly **not** a critical field: colour is cosmetic, so an
  unrecognised or missing value must degrade to `teal` rather than
  failing to load settings. Validate-and-fall-back, not validate-and-error.
- **Phase 4**'s "color variant" line item is therefore already done by the
  time Phase 4 starts — see §4.

### D4 — State bridge: **cross-process IPC (2026-08-02, via ADR-013's D1 resolution)**.

D1's resolution to run the presence in its own process settles this too:
the bridge is a small local IPC channel from `desktop-edge` to the
presence, carrying `PresenceSignals` (`PRESENCE_SCENES.md` §7) and a
settings-update message. The webview / JSON boundary the first revision
of this plan worried about is still gone — both ends are Rust — but there
is now a serialization step. The transport and encoding are open per
ADR-013's "Not decided here"; the leading candidate is
protobuf-over-length-prefixed-frames because JSON at 60 Hz is wasteful.
The type shape at the Rust API is unchanged from what the Phase 1
prototype's dev panel already drives, which is the whole point of
having modelled `PresenceSignals` up front.

The T9/T14 constraint from §6 (no raw audio, transcript, or
prompt/completion content on the wire) has to be enforced in the *type*
rather than by convention. The `PresenceSignals` fields today are all
either enums or scalars in `[0, 1]`, which already satisfies this;
Phase 2 must not relax it.

Unchanged from the first revision: presence state must only ever be
derived scalars (mode, intensity, audio *level*, progress, confidence) —
never raw audio, transcript text, or prompt/completion content
(`PRESENCE_VISUAL_ENTITY.md` §5.2, T9/T14).

### D5 — Sequencing dependency on `NEXT_STEPS.md` #13 (unchanged by the Rust pivot).

Real signal wiring for `listening`/`thinking`/`speaking`/`tool_use` needs:
- A running VAD state machine wired into the shell (not just capture
  metrics) → blocked on `NEXT_STEPS.md` #13, "Live mic → VAD → STT path in the shell."
- An in-process (or at least in-shell-reachable) `AiRouter`/`ToolGateway`
  call lifecycle to hang `thinking`/`tool_use` off of — **does not exist in
  `desktop-edge` at all yet** (today those crates are only wired into
  `ralleh-mcp-server`). This is a second, distinct prerequisite this plan
  surfaces that isn't currently tracked anywhere in `NEXT_STEPS.md`.

Phase 1–2 (prototype + mocked/synthetic signals) do **not** depend on
either prerequisite and can proceed now. Phase 3 (real signals) cannot
complete until both prerequisites land. See §4.

## 4. Phased roadmap (mapped onto this repo)

### Phase 0 — Design lock (done)
- `PRESENCE_VISUAL_ENTITY.md`, `PRESENCE_SCENES.md`, this plan, and
  ADR-010 written and reviewed (twice — corrected to Rust-first mid-flight).
- Primary modes, entity types, visual signatures, palette, and initial
  tunables agreed.

### Phase 1 — Standalone Rust prototype (`presence-prototype/`) — **substantially complete (2026-08-02)**
Goal: tune the entity's feel and scene differentiation in a real, running
Rust binary, with zero coupling to `desktop-edge`. Scope per
`PRESENCE_SCENES.md` §9:

- `presence-prototype/` crate (D-Workspace), `winit` window + `wgpu` render
  loop, instanced-billboard point renderer with soft circular falloff (D2).
- `PRESENCE_SCENES.md` §5 architecture as literal Rust traits:
  `PointGenerator`/`PointBehavior`/`Scene`/`EntityInstance`/`SceneRegistry`,
  a thin `SceneDirector`, and a `Transition` model (parameter + density
  morph, 400–1200ms, smoothstep-eased). Joined mid-phase by `SurfaceShape`
  (ADR-011), which sits alongside that split rather than replacing part of it.
- Two concrete entities/scenes, per `PRESENCE_SCENES.md` §4. Both now run
  the same `SurfaceGenerator` + `SurfaceBehavior` and differ only by shape,
  which is the point — adding a state is an engine change in neither case:
  - **Idle** — `AssistantCloud` wearing `PresenceShell`. Originally specified
    as the viscous cloud / lava-drip dynamics; those relocated to
    Thinking/ToolUse (`PRESENCE_SCENES.md` §4.3) once it was clear they read
    as activity in the one state that should not be expressing any.
  - **Loading** — `LoadingRing` wearing `ResonancePlate` (Chladni-style),
    composited *alongside* a subdued idle shell per the multi-entity note in
    `PRESENCE_SCENES.md` §2. "Subdued" is load-bearing: at equal strength the
    two entities sum into a denser blob and the modal pattern is not visible
    at all.
- Point budget started at `pointCount = 3000` for the primary cloud
  (the original resource-conscious default), was raised to 12,000 on
  measurement, and is now **80,000 for the idle shell and 40,000 for the
  loading plate**. The last jump is not a tuning change — it is the surface
  model (ADR-011): a volume hides most of its points behind its own front,
  a surface does not, so the count that reads as a dense volume reads as
  countable dots on a skin. A release build measures ~150–178 FPS at idle
  and ~109–171 in every activity mode at 2560×1600 on 2 cores at those
  budgets. This supersedes `PRESENCE_VISUAL_ENTITY.md` §9's original
  2,500–12,000 range and `PRESENCE_SCENES.md` §8's "rarely more than 8–12k";
  both documents now carry the revised numbers.
- Render path beyond the plain point pipeline, added while tuning: HDR
  `Rgba16Float` target → bright-pass → 5-level bloom → ACES tonemap +
  vignette composite; per-particle `Core`/`Body`/`Halo` layer tags driving
  §3.3's density, motion, and material gradients within one population; and
  density-driven highlight desaturation so §3.1's near-white hotspots emerge
  from accumulation rather than being assigned. See
  `presence-prototype/README.md` for why HDR is load-bearing here and why
  MSAA and a depth buffer are deliberately absent.
- Per-particle `normal` and `crease` (ADR-011), growing the instance record
  from 32 to 48 bytes, feeding the point shader's grazing/silhouette and
  fold-filament terms. These are what make the entity read as a scanned
  solid rather than a nebula, and they have no volumetric equivalent.
- **Palette as a real user setting**, not a Phase 4 item: `PaletteId`
  (`teal`/`lime`/`ice`/`ember`) with a live selector in the debug panel.
  Pulled forward from Phase 4 per D3/ADR-011 — the plumbing is the whole
  cost, and baking the palette in would have to be undone to ship the
  setting at all.
- **Activity modes, pulled forward from Phase 3** (ADR-012): `thinking`,
  `speaking`, and `tool_use` as weighted terms on the same `PresenceShell`,
  with a `ModeLayer` holding a *set* of engaged modes and an eased per-weight
  ramp over the 300–900ms this document specifies. They composed cheaply
  enough to land in Phase 1 because the model makes them terms rather than
  shapes; a per-state-shape design would have made this a phase of its own.
  Two revisions came out of the implementation and are recorded in ADR-012:
  thinking's curl swirl does not survive the move to a surface, and speech's
  syllable rate cannot pass the behavior's spring, so it drives brightness
  while a phrase envelope drives geometry.
- Dev controls (keyboard + `egui` panel): toggle Idle/Loading, toggle the
  ring on/off independently, per-mode checkboxes, intensity/progress/
  audio-level overrides — driving the exact `PresenceSignals` shape
  (`PRESENCE_SCENES.md` §7) the real bridge uses later, so this simulation
  code is not rewritten in Phase 2.
- `egui` debug overlay: active entities, engaged modes and their resolved
  term weights, raw signal values, fps.
- **Exit criteria — met (2026-08-02):** Idle and Loading are clearly,
  immediately distinguishable (density/motion/character, not just
  color); Idle reads as calm within a few seconds (further softened by
  the 2026-08-02 idle-calm pass — halved evolution/breath cadence plus
  a slow crease-brightness rest); the ring's on/off toggle proves
  multi-entity composition works, and the `activity_scale` hierarchy
  makes composition with active modes read cleanly rather than as two
  entities fighting for attention; each activity mode is
  distinguishable from idle and from the others; two modes at once
  read as both rather than as either; transitions are continuous, not
  hard cuts. Recorded as an informal manual QA pass — see §5.
- **Pulled forward from the improvement-guidance pass (2026-08-02),
  originally scoped for later phases:**
  - `listening`, `attention`, `error` implemented as material-only
    modes on the same `ModeLayer` (no new geometry — the invariant is
    tested in `material_modes_never_reach_the_shell_drive`). Formerly
    Phase 3 signal work.
  - Reduced-motion preset (R key). Formerly Phase 4.
  - Quality tiers `Balanced` / `Low` with runtime `deform_stride` and
    adaptive downshift after 3 s under 45 FPS. Formerly Phase 4.
  - `SceneRegistry` productized with `entity_kind`/`priority`/
    `default_active` and a sync-with-director test; §8 of
    `PRESENCE_SCENES.md` now names files and functions for the
    "adding a scene" flow.
- **Remaining Phase 1 items** (opportunistic, not blocking Phase 2):
  - Very-long-run (30+ minutes) peripheral-idle QA — currently
    informal.
  - Optional `High` quality tier (100k+) if a target machine warrants
    it — measurement, not design.
  - GPU compute path for deformation — deferred per ADR-011's
    fallback ordering; not needed at current numbers.

### Phase 2 — Core integration (window productization + desktop-edge bridge)

Design locked in [ADR-013](./adr/adr-013-presence-window-and-process-model.md).
Concrete work in rough execution order:

1. **Split the prototype into a shippable shape.** Refactor
   `presence-prototype/` into `presence-core` (renderer + simulation
   library — no `winit`, no key handling, no debug overlay) and
   `presence-runtime` (the binary opening a window and running the
   loop). The dev panel becomes a `dev` feature on the runtime so a
   production build does not ship it. Do not rewrite — promote,
   per §7 open item #2.
2. **Define the IPC surface.** New `presence-ipc` crate holding the
   wire type for `PresenceSignals` (already shaped by the Phase 1 dev
   panel) plus a settings-update message (palette, quality tier,
   reduced-motion, window bounds). Transport and encoding are open
   per ADR-013 "Not decided here"; leading candidate is
   protobuf-over-length-prefixed-frames because JSON at 60 Hz is
   wasteful. Enforce the T9/T14 constraint in the type — see D4.
3. **Frameless / transparent / always-on-top droplet, Windows first.**
   Per-pixel alpha, click-through by default, hover-hold or global
   hotkey to bring focus. Windows first because per-pixel alpha +
   click-through is where the platform is fussiest; macOS and Linux
   follow.
4. **Position and layout persistence.** Presence-side layout store —
   the shell does not own window geometry. Single monitor first;
   multi-monitor placement is still open per ADR-013.
5. **Launch and discovery.** How the shell finds or spawns the
   presence process. Options: shell-spawned child, user-launched
   alongside the shell, OS service. Not decided; prototype
   shell-spawned first because it is the simplest and does not
   preclude the others.
6. **`EdgeSettings.presence_*`** — persist palette (D3), plus
   `presence_quality_tier` and `presence_reduced_motion`. All three
   are validated against a fixed list (mirroring `voice_style` in
   `desktop-edge/src-tauri/src/settings.rs`) and fall back on
   unknown/missing values rather than failing the load. On startup
   and on change the shell IPCs the resolved values into the
   presence. The Phase 1 selectors already drive the same
   `PaletteId`/`QualityTier`, so this is persistence and UI, not new
   render plumbing.
7. **Still driven by synthetic signals** — this phase proves the
   window + IPC + settings mechanics, not real assistant state.

### Phase 3 — Real signal enrichment (blocked on D5 prerequisites)
1. Replace synthetic signals with the real VAD state machine once
   `NEXT_STEPS.md` #13 lands (`idle`/`listening` become real).
2. Decide and implement how `thinking`/`tool_use` get their signal —
   requires wiring `ralleh-ai-router`/`ralleh-tool-gateway` (or a local
   stand-in call) into the desktop edge process, which does not exist
   today (see D5). Size this separately once #13 is done. The *visuals* for
   both landed in Phase 1 (ADR-012), so this is signal plumbing only:
   `ModeLayer::set` is the whole surface it has to reach.
3. Wire real audio level (input and/or output) into the `speaking` pulse.
   Note the split it has to feed: geometry takes a phrase envelope and
   brightness takes the raw level, because the surface behavior's spring
   cannot pass a syllable rate (ADR-012).
4. Introduce secondary events (scan sweeps, inbound streams) — sparse, per
   the anti-patterns list.
5. Map policy `Denied` / handler `Failed` outcomes to `error`.

### Phase 4 — Hardening & options
1. Performance budget testing: confirm 60fps at the Phase-1-tuned point
   counts (80,000 idle / 40,000 loading, with 2–3 concurrent entities) on
   representative hardware. Phase 1's own measurement was taken on a 2-core
   development machine, which is a useful floor but not a substitute for
   this pass. If a target machine cannot hold the budget, the fallbacks in
   order are: drop the shell to 2 noise octaves, then widen the deform
   refresh stride, then `rayon`. Do not reach for GPU compute before those
   (ADR-011).
2. User settings: density, intensity scale, reduced-motion override.
   **Colour variant is no longer part of this phase** — it moved to Phase 1
   (prototype) and Phase 2 (`EdgeSettings.presence_palette`) per D3/ADR-011.
3. Edge-case behavior: very high load, rapid state changes.
4. Accessibility: optional text status line alongside the visual, honoring
   a reduced-motion preference (mirroring `desktop-edge`'s existing
   `prefers-reduced-motion` convention even though this renderer has no DOM).
5. Document extension points for new entity types (`PRESENCE_SCENES.md` §8).

## 5. Testing & validation strategy

This repo's guiding rule is "each core module must have a real automated
test suite and pass before the next module is layered on top." A
point-cloud rendering feature does not fit that rule literally (rendered
output is not meaningfully unit-testable), so this plan draws an explicit
line between what gets real automated tests and what gets manual/visual QA:

- **Automated (`cargo test` inside `presence-prototype/`):** generators,
  behaviors, surface shapes, the noise/ridged/curl math, the palette's
  chroma normalization and name round-tripping, the `Transition`
  progress/easing calculation, and the `SceneDirector`'s mode-selection
  logic are all pure functions over data (no window/GPU needed) and must be
  unit-tested, the same way every other Rust module in this repo is. This
  is real, meaningful coverage, distinct from "rendering can't be
  unit-tested" — don't let the second excuse the first.

  Shapes in particular test better than they might look. The properties
  worth asserting are geometric invariants rather than pixel values: that
  a shell's displaced radius stays inside a band (a shell reaching the
  origin has self-intersecting folds; one growing without bound leaves the
  viewport), that its creases land on ridges rather than troughs, that the
  plate's grains stay on the plate, that the staggered deform refresh
  reaches every particle within one cycle, and that no layer is ever empty
  at any density bias. Each of those corresponds to a failure the
  prototype actually hit.
- **Not automated (rendering itself):** the actual `wgpu` draw output.
  Verified via the `egui` debug overlay + manual observation, per Phase 1's exit criteria.
- **Manual/visual QA:** the "clearly distinguishable scenes" pass from
  Phase 1's exit criteria, repeated whenever tuning changes.
- **Performance:** Phase 4's fps budget pass is manual/instrumented, not a
  unit test — record results in `STATUS.md` once run, the same way other
  validated-state snapshots are recorded there.
- **CI boundary:** `presence-prototype/`'s exclusion from the workspace
  (D-Workspace) means `cargo test --workspace` at the repo root will
  **not** run its tests automatically. Run `cargo test` from inside
  `presence-prototype/` directly; consider a dedicated, manually-triggered
  CI job later (mirroring the `audio-e2e` workflow's `workflow_dispatch`-only pattern) if this crate grows enough to warrant it.

## 6. Threat-model cross-references

To be folded into `THREAT_MODEL.md`'s Tauri section (T11–T16) once Phase 2
implementation actually starts — recorded here first so it isn't lost:

- **T9 / T14 (mic/transcript leakage, always-on mic exfiltration):** the
  presence bridge is a one-way, lossy summarizer (see
  `PRESENCE_VISUAL_ENTITY.md` §5.2's privacy constraint). Whatever state
  channel Phase 2 lands on must never carry raw audio, transcript text, or
  prompt/completion content — enforce this by construction (the
  `PresenceSignals`/`PresenceState` type itself should have no field
  capable of holding that data), not by convention.
- **T12 (malicious/compromised webview content):** not applicable to the
  Rust-rendered surface at all (no webview involved for this specific
  window) — narrows this threat's scope rather than adding a new one.
- **T11 (IPC capability bypass):** only relevant if Phase 2 chooses the
  Tauri-managed-window path from D1 and that window still exposes any
  Tauri commands; a pure `winit`/`wgpu` surface with no webview has no
  Tauri IPC surface to bypass in the first place.
- **Deferred, not yet applicable:** T13/T16-style always-on-top/click-
  through/window-chrome threats only become relevant once Phase 2 actually
  implements frameless/always-on-top behavior — do not add those
  mitigations speculatively now.

## 7. Open items to confirm

These are recommendations, not blockers — recorded so they're visible
rather than decided silently:

1. ~~Phase 2's D1/D4 embedding decision~~ **resolved (2026-08-02) by
   [ADR-013](./adr/adr-013-presence-window-and-process-model.md)**:
   separate process, frameless transparent always-on-top droplet,
   click-through by default, shell-authoritative settings via IPC.
   Sub-decisions still open per ADR-013's "Not decided here" section:
   the specific IPC transport/encoding, the launch/discovery model,
   whether the droplet has a "docked to shell" secondary mode, and
   reduced-motion-as-OS-preference-vs-shell-toggle (both are cheap to
   support and probably both land).
2. Whether `presence-prototype/`'s code is meant to be thrown away or
   gradually promoted into the real crate (e.g. renamed/moved rather
   than rewritten) once Phase 2 starts. Recommendation: **promote,
   don't rewrite** — structure the crate now as if it might become the
   real thing (clean module boundaries, no throwaway hacks) even
   though its purpose today is tuning. Phase 2 §4.1 (split into
   `presence-core` + `presence-runtime`) is the concrete first step.
3. Whether `presence-prototype/` should eventually move under
   `crates/` as a real workspace member (with GPU/display bits still
   feature-gated per `HEADLESS.md`'s rule) once it's no longer just a
   tuning tool. Recommendation: revisit as part of Phase 2 §4.1's
   split — either the two new crates go under `crates/` from the
   start, or they stay outside the workspace and get promoted
   together in Phase 3 once real signals arrive.

---

*Update this document as phases complete or decisions change — like
`ARCHITECTURE.md`, it should always reflect what's actually true about the
integration, not the original aspiration.*
