# Presence Engine Evolution — Roadmap

**Status:** In progress. This doc tracks the incremental evolution of the
presence from a mode-driven point cloud into a believable AI Presence Engine,
per [ADR-014](./adr/adr-014-presence-engine-architecture.md).

**Companion documents:**
- [`PRESENCE_VISUAL_ENTITY.md`](./PRESENCE_VISUAL_ENTITY.md) — visual/state
  design source of truth (what it looks like, what it means).
- [`PRESENCE_SCENES.md`](./PRESENCE_SCENES.md) — Scene/Entity/Generator/
  Behavior/Director architecture.
- [`PRESENCE_INTEGRATION_PLAN.md`](./PRESENCE_INTEGRATION_PLAN.md) — phasing
  and desktop-edge integration (this roadmap is the engine-architecture layer
  on top of that plan's Phase 3+).
- [ADR-014](./adr/adr-014-presence-engine-architecture.md) — the architecture
  this roadmap implements, and its reconciliation with ADR-011/012.

## Principle

Cognition and rendering never touch. They communicate only through two
immutable, bounded structures: `PresenceState` (Brain → engine, over IPC) and
`SimulationParameters` (Behavior Graph → simulation). The renderer stays
generic; behaviors emit forces/weights, never animations; morphing morphs
fields, not particles.

## Target architecture

```
LLM / audio / cursor  ──►  Presence Brain  ──►  PresenceState  ──►(IPC)──►  Behavior Graph
                            (desktop-edge)      (bounded, immutable)         (presence process)
                                                                                   │
                                                                                   ▼
                              Render ◄── Particle Simulation ◄── Force Fields ◄── SimulationParameters
```

## What already exists (preserved, not rebuilt)

- Generic renderer (`presence-core/src/render/`) — consumes particle slices only.
- Cognition resolver (`presence-core/src/scene/mode.rs`) — `ModeLayer` →
  `ShellDrive` + material, eased/additive (ADR-012).
- Bounded IPC contract (`crates/presence-ipc`) — versioned, scalars/enums only.
- Two-process model (`desktop-edge` spawns `presence-runtime`).

## Milestones

Each milestone leaves the app in a working state and passes `cargo fmt`,
`cargo clippy --workspace --all-targets -- -D warnings`, and tests. The
prototype builds via `scripts\presence-dev.cmd`; the root workspace via
`scripts\cargo-dev.cmd` and stays headless (`HEADLESS.md`).

| M | Deliverable | Substrate |
|---|---|---|
| M0 | ADR-014 + this roadmap | docs |
| M1 | `PresenceState` + `SetPresenceState` in `presence-ipc`; adapter in `presence-core` (no visual change) | contract |
| M2 | `desktop-edge` `presence_brain.rs`; wire speaking/progress/thinking/tool/listening/cursor; emit `SetPresenceState` | Brain |
| M3 | `presence-core/behavior/` Behavior Graph; port `ModeLayer`; add Confidence/Curiosity/Uncertainty | Behavior |
| M4 | `sim/field/` force-field substrate (curl/attractor/drift/turbulence) + one free-space entity | Simulation (CPU) |
| M5 | `sim/field/sdf.rs` morph targets (sphere/ring/helix) driven by focus/confidence | Simulation (CPU) |
| M6 | Richer layers (Aura/Energy/Sparks/Trails) with independent params | Simulation + Render |
| M7 | Speech amplitude → shell vibration/expansion/brightness; cursor look-at | Simulation |
| M8 | GPU compute substrate (deferred) — integrator in WGSL, positions in storage buffers | Simulation (GPU) |

## Invariants honored every milestone

- `PresenceState` carries only bounded scalars/enums/directions (T9/T14).
- Every new IPC message is versioned; `MIN_SUPPORTED_VERSION` keeps old
  commands valid.
- Every new Tauri command is added to the capability allowlist.
- ADR-012's additive composition, gating, and spring-bandwidth rule hold:
  geometry-moving behaviors stay inside the ~0.7 Hz spring; fast signals drive
  brightness/size.
- Force fields apply to free-space entities only; the shell keeps its surface
  spring (ADR-011).
- The surface builtins stay in sync with the director
  (`default_active_builtins_match_the_director`).

## Status log

- M0 (this doc + ADR-014): done.
- M1 (`PresenceState` + `SetPresenceState`, `VERSION` 2→3, core adapter, tests):
  done.
- M2 (Presence Brain): done. Pure logic in the new headless `presence-brain`
  crate (root CI); shell glue in `desktop-edge/src/presence_brain.rs`
  (`PresenceBrainHandle`) emits one authoritative `SetPresenceState` per change.
  `hold_mode`/`pulse_*`/`current_modes`, the dev-panel setters, and the
  mic/TTS/scan-sweep pumps all route through the Brain — cognition no longer
  travels the wire as `SetMode`/`SetSignals`. No new Tauri commands, so the
  capability allowlist is unchanged. The low-rate cursor→attention sampler is
  deferred to M7 (the `set_cursor` plumbing and wire fields already exist).
- M3 (Behavior Graph): done. New `presence-core/src/behavior/` adds a
  `Behavior` trait + `BehaviorStack` (ordered, blended). `ModeLayer` is ported
  as `ModeBehavior` (a thin wrapper — its tuned math is untouched, and a test
  proves a `[ModeBehavior]` stack reproduces `ModeLayer` output exactly).
  A new `CognitiveState` + `CognitionBehavior` add bounded Confidence /
  Curiosity / Uncertainty modulations that touch only material fields
  (intensity/cool/expand), never `ShellDrive` — so the ~0.7 Hz spring-bandwidth
  rule holds by construction. The director now stores a neutral-by-default
  `cognition` snapshot (populated by the `SetPresenceState` adapter) and applies
  it after the mode layer; neutral cognition is a no-op, so the resting shell
  and `default_active_builtins_match_the_director` are unchanged.
- M4 (Force-field substrate): done. New `presence-core/src/sim/field/` adds a
  `ForceField` trait, a `CompositeField` (forces superpose = sum), and a
  `FieldBehavior` integrator (semi-implicit Euler + frame-rate-independent
  exponential damping + hard speed clamp). Four data-driven forces —
  `Attractor` (linear spring, the M5 morph seed), `Curl` (finally uses the
  divergence-free `noise::curl` ADR-011 left dead), `Turbulence` (multi-octave
  curl), `Drift` (low-freq wander) — plus a `FieldCloudGenerator` that fills a
  bounded ball (the volume fill ADR-011 forbids for the *shell* and ADR-014
  permits for free space). One free-space entity ships as the `field_cloud`
  ("Nebula") builtin: registered but **not** `default_active`, presented on
  demand, so the resting app is still only the idle shell + loading plate.
  Deterministic field tests cover force direction, curl determinism, composite
  summation, damping, the speed cap, generator boundedness, and a long-run
  stability check. Forces are plain parameters (not closures), so the M8 WGSL
  port is mechanical.
- M5 (Morph via fields): done. New `sim/field/sdf.rs` adds a `MorphTarget`
  (sphere/ring/helix) with `sdf`/`gradient`/`project`, and an `SdfAttractor`
  force that pulls each particle "downhill" onto the shape's zero level set —
  no per-particle destinations, so the cloud finds the shape collectively while
  curl/drift keep it circulating along it. Morph coherence is cognition: the
  pull scales with `focus` (a coherence floor keeps a loose shape at rest) and
  `confidence` (tightness). Two new `EntityParams` fields (`focus`,
  `confidence`, neutral defaults) carry the Behavior Graph's cognition to
  free-space forces; the director copies them onto live free-space entities each
  frame (surface entities ignore them). The shipped `field_cloud` now uses
  `FieldBehavior::morph` toward a sphere, so it reads diffuse when idle and
  condenses as focus rises. Tests cover SDF signs, gradient direction, surface
  projection, focus/confidence scaling, deterministic convergence, and that a
  focused cloud condenses tighter than an unfocused one.
- M6 (Richer layers): done. The `Layer` enum gains four effect/material classes
  — `Aura`, `Energy`, `Sparks`, `Trails` — alongside the Core/Body/Halo density
  ramp, each with independent `LayerMaterial` params (size/brightness) via
  `Layer::material()` and an `is_surface()` predicate. They encode past the
  density ramp (`as_f32` ≥ 3), so `shader.wgsl` branches on them for a per-class
  falloff (aura soft, energy tight, sparks pinpoint, trails soft) while the 0–2
  core→halo interpolation is byte-identical for surface entities. Surface-only
  matches in `shapes.rs`/`ui.rs` fall the effect layers back to the outer band
  (they never reach that code). Tests cover distinct encodings, the surface
  predicate, and the intended size/brightness character of each effect class.
  No InstanceRaw layout change was needed — the existing `layer: f32` attribute
  carries the class.
- M7 (Audio & cursor as physics): done (engine). New `behavior/response.rs` adds
  two pure, tested mappings. `audio_response` splits speech by the ADR-012
  spring bandwidth: a bounded **expansion** (a uniform scale swell) rides the
  slow phrase envelope so it is spring-safe, while the fast syllable level stays
  in the (already-applied) brightness channel — silence is a no-op. `cursor_aim`
  turns the bounded `cursor_dir`/`cursor_proximity` into a **look-at lean**
  (a translation toward the pointer, scaled by proximity, screen-down flipped to
  world-up) plus an attention bias. The director stores cursor state (from the
  `SetPresenceState` adapter via `set_cursor`, clamped), eases the lean, and
  applies expansion (scale) + lean (centre translation) to the shell after the
  Behavior Graph — both no-ops at rest, so `default_active_builtins_match_the_director`
  and the tuned shell shape/tests are untouched (the lean is a translation, not
  a surface deformation). Tests cover the bandwidth split, boundedness, the lean
  direction/return-to-centre, and the speech swell cap. Remaining wiring (not in
  root CI): the `desktop-edge` OS cursor sampler that calls
  `PresenceBrain::set_cursor` — the Brain plumbing and wire fields already exist,
  so it is a low-rate pump reading the droplet window + global cursor.
- M8 (GPU compute substrate): deferred by design (ADR-014 + this plan). No GPU
  code was written; the CPU `FieldBehavior` remains the substrate. The port is
  kept mechanical on purpose: forces are plain data descriptors (`Attractor`,
  `Curl`, `Turbulence`, `Drift`, `SdfAttractor`), the integrator is a single
  semi-implicit Euler + damping + clamp loop, and morph is an SDF evaluated
  per particle — each maps directly to a WGSL compute pass over storage-buffer
  positions/velocities the renderer reads without a per-frame upload. Revisit
  only when particle counts or morph complexity make CPU integration the
  bottleneck.
