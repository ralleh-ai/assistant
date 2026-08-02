# Point Cloud Scenes — Entity & Transition System

**Project**: Ralleh Assistant (`ralleh-ai/assistant`)
**Companion to**: [`PRESENCE_VISUAL_ENTITY.md`](./PRESENCE_VISUAL_ENTITY.md) (the main design doc)
**Status**: Design foundation → Phase 1 prototype scope (see
[`PRESENCE_INTEGRATION_PLAN.md`](./PRESENCE_INTEGRATION_PLAN.md))
**Origin**: Adapted from an external companion note
(`POINT_CLOUD_SCENES.md`), added to this repo one session after
`PRESENCE_VISUAL_ENTITY.md`. Material/palette references below are
reconciled to this repo's brand tokens exactly as `PRESENCE_VISUAL_ENTITY.md`
§3.1 already did — see the note in §2 below.
**Revision note**: `PRESENCE_VISUAL_ENTITY.md` was subsequently corrected
to a Rust-first (`winit`+`wgpu`) production target instead of a Three.js/
Tauri-webview one (see that doc's §7 and ADR-010). This document's
interfaces were originally written as TypeScript "because ADR-010 said
simulation stays in TS through Phase 1–3" — **that reasoning no longer
applies**. §5.1 below now gives the interfaces directly in Rust, matching
the main document's §7.3 and the actual Phase 1 prototype crate. The
Scene/Entity/Generator/Behavior/Director *separation of concerns* is
unchanged; only the implementation language is corrected.
**Surface revision (ADR-011)**: §4.1's Idle scene is now the folded shell,
its viscous dynamics having relocated to §4.3's Thinking and ToolUse;
§5.1-5.3 gain `SurfaceShape` and replace the volume-centric generator and
behavior lists; §8's point budget is revised against measurement. The
separation of concerns is again unchanged — `SurfaceShape` sits alongside
it rather than replacing any part of it.
**Additive-mode revision (ADR-012)**: §4.3's future scenes are now modes,
implemented as weighted terms on the one shell §4.1 shows at rest, and §5.2's
shape vocabulary collapses from six shapes to two — which reverses what that
section said in the surface revision. Adding a state is now adding a term.
Two specifics changed on the way: thinking's curl swirl does not survive the
move to a surface, and tool use's pendants extend and retract rather than
detaching.
**Last Updated**: 2026-08-02

---

## 1. Purpose

This document extends `PRESENCE_VISUAL_ENTITY.md` with a practical scene
system — the technical structure that actually *realizes* the modes
(`idle`/`listening`/`thinking`/...) as concrete, renderable visuals.

Goals:

- Define the initial concrete scenes (Idle, Loading, and the path to others)
- Provide a simple, robust foundation for multiple scenes and transitions
- Keep the architecture light enough that a developer can add new scenes
  without fighting the system
- Preserve the core metaphor: the window is a scanned viewport into internal processes

This is not a full engine specification. It is a clear, extensible foundation.

**Relationship to the main document:** `PRESENCE_VISUAL_ENTITY.md` defines
the overall vision, state system (the 7 modes), visual language, and
technical direction. This document defines *how concrete scenes are
structured* and how the first two (Idle, Loading) are realized. **When the
two conflict, `PRESENCE_VISUAL_ENTITY.md`'s core principles take
precedence** — this is a specialization, not a competing design.

## 2. Note on "Loading" vs. the 7-mode state system

`PRESENCE_VISUAL_ENTITY.md` §4.1 defines seven primary modes (`idle`,
`listening`, `thinking`, `speaking`, `tool_use`, `error`, `attention`).
This document's "Loading" scene is **not an eighth mode** — it's the
concrete scene a mode selects when a *bounded process with (or without)
known progress* is underway (e.g. `thinking` while a model loads, or
`tool_use` for a long-running job). Treat "Scene" as the rendering-layer
unit a mode maps to, not a parallel state machine. This mapping is
finalized during Phase 3 (real signals) once it's clear which real
assistant events are actually "bounded processes" vs. open-ended
`thinking`/`tool_use` — recorded as an open item in
`PRESENCE_INTEGRATION_PLAN.md` §7.

All material-language references in the original companion note ("lime/
yellow-green palette, black field") are superseded by the same brand
reconciliation already recorded in `PRESENCE_VISUAL_ENTITY.md` §3.1 — every
scene shares the reconciled teal/foam/mist/amber palette, not a separate
lime one.

**Scene vs. Entity System terminology:** `PRESENCE_VISUAL_ENTITY.md` §4
describes this same architecture as an "Entity System" with a richer
initial entity set (`AssistantCloud`, `LoadingRing`, `ProgressArc`,
`DataStream`, `SecondaryAgent`, `AttentionPulse`, `ErrorFragment`) that can
compose *simultaneously* (§4.3, "Multi-Entity Scenes"). This document's
"Scene" (§3, "at any moment the viewport shows exactly one active Scene")
is the narrower starting structure: think of a "Scene" here as a
convenient bundle of one-or-more `EntityInstance`s active at once,
identical to §4.3's multi-entity composition — not a competing model. In
particular, "Loading" in this document is realized as the `LoadingRing`
entity type composited *alongside* a subdued `AssistantCloud`, per
`PRESENCE_VISUAL_ENTITY.md` §4.3's first example ("primary assistant cloud
+ subtle loading ring for a background task"), rather than a scene that
fully replaces the idle cloud. The Phase 1 prototype implements it this
way.

---

## 3. Core Concepts

### Scene

A **Scene** is a complete visual configuration that occupies the viewport.
It consists of:

- One or more **Entities**
- Shared parameters (intensity, time scale, color bias, etc.)
- Optional secondary effects

At any moment the viewport shows exactly one active Scene (or a transition
between two).

### Entity

An **Entity** is a point-based form with:

- A **generator** (how points are initially distributed)
- A **behavior** (how points move and evolve each frame)
- Lightweight metadata (priority, point budget, etc.)

All entities share the same material language (soft points, the
brand-reconciled palette from `PRESENCE_VISUAL_ENTITY.md` §3.2, near-black field).

### Transition

A **Transition** moves the viewport from one Scene to another. Preferred
style is continuous morphing of density, forces, and structure rather than
hard cuts.

---

## 4. Initial Scenes

### 4.1 Idle — The Presence Shell At Rest

**Intent**: Long-running calm presence. Must feel alive but never demand attention.

Idle is not a scene the director selects; it is what the shell degenerates to
when no mode is engaged (ADR-012). Everything below describes the shell's
`fold` term, which is the only one live at rest — and which never switches
off, so it is also the thread of identity running through every mode in §4.3.

**Character**:
- A closed skin — a sphere displaced radially by ridged noise, so it reads
  as a folded shell or a closed rose
- Bright grazing rim: the silhouette is the brightest part of the form,
  which is what makes it read as a solid object rather than a haze
- Fold filaments tracing the creases across its face
- A slow turn on a tilted axis, and a slow breath
- The fold pattern itself reshapes over tens of seconds, independent of
  the turn, so the form is never quite the same twice
- Sparse halo drifting outside the skin

**Key parameters**:
- Fold depth (peak-to-trough displacement) and fold scale (petal count)
- Octave count for the ridged noise
- Spin speed, and fold-evolution speed as a separate rate
- Breathing amplitude and speed
- Crease threshold — the ridge value at which a fold starts to register
- The fold term's weight, which the mode layer lowers as other terms rise so
  the summed shell stays inside its radius band. It yields depth; it never
  reaches zero.

**Design notes**:
- This is the default Scene when the assistant has no significant activity —
  i.e. the concrete realization of the `idle` mode.
- **This scene was originally specified as a viscous cloud** (lava/oil-drip
  dynamics: clusters that rise, elongate, thin, and fall). Those dynamics
  are not discarded — they were the wrong *default*, not a wrong idea, and
  they have relocated to Thinking and ToolUse in §4.3. Idle is the state
  the user sees for hours, and rising/falling clusters read as activity,
  which contradicts §2.3's "calm by default" and wastes the vocabulary on
  the one state that should not be expressing anything.
- **The whole effect rests on one thing**: displace by *ridged* noise, then
  reuse the same ridge value as the crease brightness. Creases then land
  exactly on folds by construction and at zero extra cost. Computing crease
  intensity separately is both more expensive and guaranteed to drift out of
  alignment with the geometry it describes.
- Fold depth has to be genuinely deep. A shallow displacement is
  indistinguishable from a sphere once the points are small, and then none
  of the surface machinery shows: the silhouette is a circle and the
  creases have nothing to trace.
- The crease threshold has to be low enough that creases cover a real
  fraction of the skin. Set high, the filaments technically exist but sit
  almost entirely on the limb, where the grazing term is already saturated
  — so the face of the shell stays empty and the form reads as a hollow
  bubble instead of a folded one.
- Rotate the finished surface point, not the noise input. Rotating the
  noise makes folds travel across a stationary point set, which reads as a
  shimmer rather than as an object turning.

### 4.2 Loading — Resonance Field (Chladni-style)

**Intent**: Communicates that the system is occupied with a bounded process
(model load, long tool, background job, etc.).

**Character**:
- A stationary square sheet with sand on it, facing the viewer
- The sheet never moves. **All** motion is the sand rearranging
- The drive holds a resonance for a few seconds; the grains sit still on its
  nodal lines, then the frequency steps and they slide into an entirely
  different figure over about a second
- Low-to-mid order modes preferred
- Which figures get visited is driven by progress or intensity
- Still rendered as soft points so it remains part of the scanned aesthetic

**Key parameters**:
- The mode table — the `(m, n)` resonances the drive steps through
- Dwell (how long each resonance is held)
- Nodal migration limit (how far a grain may travel to reach a line)
- Pile width (how thick the ridge of sand along a line is)
- Softness / noise mix (prevents it from looking like a pure mathematical diagram)

**Design notes**:
- Keep evolution relatively slow. Rapid high-order mode cycling becomes fatiguing.
- **The sheet does not rotate, and this is a rule rather than a preference.**
  A form turning on its own axis is a loading spinner, and a spinner is
  decoration standing in for status (§6's "informative over decorative"): it
  looks exactly the same whether work is progressing or the process is
  wedged, so it carries no information at all. Everything the viewer sees
  here has to come from the sand. Any future planar entity inherits this.
- **Mode numbers are integers held for seconds, not floats drifting
  continuously.** The prototype drifted them continuously at first, and the
  result never resolves: a plate between resonances has no stable figure, so
  the sand churns permanently and the scene reads as generic motion. Real
  plates jump between discrete resonances, and the *stillness* between jumps
  is what makes each figure legible. A figure nobody has time to see is
  indistinguishable from noise.
- **The standing wave is a superposition, not a single product.** The first
  version used `v = cos(m·x)·cos(n·y)`, which only ever draws a grid:
  changing the mode numbers changes the grid's spacing and nothing else, so
  stepping the frequency produced no new *shapes*, which is the entire
  content of the scene. A free square plate's wave is a mode superposed with
  its transpose — `v = cos(nπx)cos(mπy) − cos(mπx)cos(nπy)` — and the
  subtraction is what puts nodal lines along the diagonals and gives the
  crosses, rosettes, and lattices a Chladni plate is recognised by.
- **The plate is square, not round.** The iconic figures are square-plate
  figures; a circular plate's modes are Bessel patterns, which are a
  different look and much more expensive to evaluate. Taper the point
  density to nothing at the rim so the sheet dissolves rather than ending on
  four straight edges — §10 rules out hard geometric shapes that never
  dissolve, and a literal square outline is exactly that.
- **Sand piles have width.** Landing every grain exactly on the zero set
  renders the figure as a one-pixel wireframe, which reads as a *diagram* of
  a Chladni plate rather than sand on one. Scatter each grain across the
  ridge by a fixed per-grain offset, so it keeps its place instead of
  shimmering.
- **Nodal proximity means "did this grain reach a line", not "how large is
  the field where it started".** Measuring the field at a grain's original
  position leaves most of the sand in the figure unlit, since a grain that
  migrated in from an anti-node still has a large field value back where it
  came from.
- The scene's clock runs only while it is showing, so the sequence always
  opens on the simplest figure. That is deliberate: the first figure is the
  one that says "this just started", and starting mid-sequence would waste
  it.
- Realized as the `LoadingRing` entity (`PRESENCE_VISUAL_ENTITY.md` §4.1),
  layered alongside a subdued `AssistantCloud` rather than replacing it —
  see the terminology note in §2 above. The Phase 1 prototype toggles it
  on/off independently of the primary cloud's mode to prove multi-entity
  composition.
- Clear visual escalation above Idle.
- **The plate faces the viewer; it is not horizontal.** A real Chladni plate
  lies flat, and the prototype built it that way first. Two problems: a
  horizontal plane reads as exactly the ground plane that
  `PRESENCE_VISUAL_ENTITY.md` §2.2's first principle rules out, and since the
  camera sits nearly level with the origin it is seen edge-on and collapses to
  a line — hiding the modal pattern that is the entire content of the scene.
  Orient any future planar entity to face the viewer for the same reasons.
- **Nodal drift must be a bounded displacement of each grain's rest position,
  not a force added to its acceleration.** The prototype's first version used
  a force proportional to the gradient of `v²`. That is unbounded in
  principle, not merely badly tuned: its magnitude scales with the mode index
  and is computed from the fixed rest position, so it does not weaken as a
  grain strays. Grains whose force outran the restoring spring left the plate
  permanently and drifted toward the camera as blown-out foreground blobs.
  Displacing the target keeps the field planar and bounded by construction,
  with the sheet's extent as a hard limit.
- **Displace by one Newton step, not a fixed distance.** The distance from a
  grain to the nearest nodal line is `|v| / |∇v|` to first order, so stepping
  by exactly that lands it *on* a line and is self-limiting — grains already
  on a line barely move. A fixed step scaled by `|v|` (the prototype's second
  attempt) overshoots whenever the line spacing is smaller than the step,
  which is most of the mode table: grains sail past one line toward the next,
  the anti-nodal regions never empty out, and the figure stays a suggestion
  instead of resolving. Cap the step to keep the bound above, but the cap
  should be a safety net for the degenerate case where `∇v` vanishes, not the
  mechanism.
- Because it composites over the shell, this scene needs enough grains to
  actually draw its nodal lines — the prototype uses 40,000. That is not
  far below the shell's own 80,000 despite the plate being a single visible
  face: the plate is wider and flat, so its points spread over several
  times the area, and grains migrating onto nodal lines only concentrate
  them where the pattern already is rather than making the plate as a whole
  denser. Below roughly this count the nodal lines resolve as dotted rather
  than drawn.
- **Loading reduces the shell rather than compositing over a full-strength
  one.** This is §2.2's "hierarchy of attention" applied to simultaneous
  entities, and it is load-bearing, not polish: at full strength the two
  entities are the same hue at similar scale and simply sum into a denser
  blob, so the modal pattern that distinguishes Loading is not merely
  harder to see, it is not visible at all. The prototype takes the shell to
  0.45 presence. Not lower — the shell has to stay legible as the thing
  Loading is happening *to*, or it reads as the presence having been
  replaced rather than occupied, which is the wrong story for a transient
  state.
- Nodal proximity is reported through the same `crease` channel the folded
  shell uses for its fold filaments. That is not convenient reuse of a
  spare field: a nodal line and a fold crease are the same thing to a
  viewer — structure emerging on a surface — so they should render
  identically.

### 4.3 Activity Modes — Terms On The Shell

These are **not** separate scenes with their own shapes. Per ADR-012, each
is a weighted term added to the same `PresenceShell` the Idle scene shows at
rest, so several can be true at once and a transition is a weight ramp
rather than a cross-fade. `thinking`, `speaking`, and `tool_use` are
implemented in the Phase 1 prototype.

| Mode      | Typical Use          | Term    | Motion Character |
|-----------|----------------------|---------|------------------|
| Thinking  | Model inference      | `lobes` | **Lava-lamp rise**: 2–4 bulges that gather, swell, migrate along the skin, thin, and are reabsorbed. Crease marks each bulge's shoulder |
| Speaking  | Audio output         | `pulse` | A wave travelling across the skin on a phrase envelope, plus syllable-rate brightness |
| ToolUse   | Active external work | `neck`  | **Oil-drip pendants**: the skin necks down and extends a pendant, one per call, which retracts on completion |

Still future work, and none of them need geometry: `listening`, `error`, and
`attention` are colour, brightness, and framing changes, so they will sit on
the same weight/lerp machinery without adding terms. `ProgressArc` and
`MultiAgent` remain separate entities.

**Where the viscous dynamics went.** Thinking and ToolUse are bolded above
because they inherit the rise/fall/elongate/thin vocabulary that §4.1
originally assigned to Idle. Recording this is the part that makes the
surface model a *framework* rather than one scene, so it is worth being
explicit about the mapping:

- **Thinking — lava-lamp rise.** The lobe motion is the same viscous
  timing the original Idle spec described (slower near the extremes, more
  fluid mid-cycle), but expressed as a deformation of the skin rather than
  as free-floating clusters. Thinking is where it belongs because it is
  the state §10 allows the most internal complexity, and because rising
  and falling internal structure is a legible picture of computation.

  Note this also replaces `PRESENCE_VISUAL_ENTITY.md` §6's "strong curl
  swirl" for thinking, and as a direct consequence of ADR-011 rather than a
  change of mind: curl displaces points *through* a volume, and after the
  surface switch there is no volume for them to move through. Applied to a
  skin it moves points off it, and the shell goes fuzzy.
- **ToolUse — oil-drip pendants.** A pendant necking off the skin is
  discrete and countable in a way a swirl is not, which is what makes it
  the right vocabulary for discrete external actions. One pendant per
  tool call keeps §6's "informative over decorative" honest.

  It **extends and retracts** rather than detaching, revising this
  document's earlier "sheds a pendant that detaches and travels". A
  `SurfaceShape` is star-shaped about its centre — one radius per direction
  — so a detached droplet is two surfaces on the same ray and there is
  nowhere to put the second one. Reaching out and pulling back also maps to
  a call's request-and-response better than shedding does, because it makes
  completion visible; an indicator that cannot show completion is showing
  activity rather than status. Detachment stays available later as a
  separate `DataStream` entity.

**Speech cannot move geometry at syllable rate.** `SurfaceBehavior`'s spring
sits near 0.7 Hz, so speech's 4–7 Hz syllable rhythm arrives at the skin
attenuated to roughly two percent. Speaking is therefore split by what each
channel can carry: geometry follows a smoothed *phrase* envelope, and
syllable-rate response goes to brightness, which is assigned rather than
integrated and so lands within one step. This generalizes — see
`PRESENCE_VISUAL_ENTITY.md` §6.

---

## 5. Foundation for Custom Scenes

The system should make it straightforward to add a new scene without
modifying core rendering code.

### 5.1 Minimal Interfaces

These are Rust traits, matching `PRESENCE_VISUAL_ENTITY.md` §7's corrected
Rust-first production target and ADR-010 (revised). An earlier revision of
this document translated these into TypeScript interfaces under the
assumption that simulation would live in the Tauri webview — that
assumption no longer holds; the Phase 1 prototype crate implements these
traits directly:

```rust
/// How points are created / reset for an entity.
trait PointGenerator {
    fn generate(&self, count: usize, params: &EntityParams) -> Vec<Particle>;
}

/// How points evolve each frame.
trait PointBehavior {
    fn update(&self, particles: &mut [Particle], dt: f32, params: &EntityParams, signals: &PresenceSignals);
}

/// The deformable skin points live on (ADR-011). This sits *alongside* the
/// generator/behavior split rather than replacing it: a shape answers
/// "where is the skin right now", a generator seeds a population across
/// it, and one shared `SurfaceBehavior` (which is an ordinary
/// `PointBehavior`) springs particles toward it.
trait SurfaceShape {
    /// Values shared by every particle this frame — rotation matrices,
    /// breathing scale, mode indices.
    type Frame;

    /// The parameter space this shape's seeds are drawn from.
    fn domain(&self) -> SurfaceDomain;

    fn frame(&self, params: &EntityParams) -> Self::Frame;

    /// The expensive, slowly-varying part: the noise.
    fn deform(&self, seed: Vec3, frame: &Self::Frame) -> SurfaceDeform;

    /// Rigid motion and breathing only. Runs for every particle every step.
    fn place(&self, seed: Vec3, local: Vec3, frame: &Self::Frame, params: &EntityParams)
        -> SurfaceSample;
}

struct SurfaceSample { position: Vec3, normal: Vec3 }
struct SurfaceDeform { local: Vec3, crease: f32 }

/// A complete scene definition.
struct Scene {
    id: SceneId,
    entities: Vec<EntityInstance>,
    /// Optional shared modifiers applied to the whole scene.
    global_params: SceneParams,
}

struct EntityInstance {
    generator: Box<dyn PointGenerator>,
    behavior: Box<dyn PointBehavior>,
    point_budget: usize,
    priority: u8, // for hierarchy when multiple entities are present
}
```

These can be simplified further (e.g. data-driven configs + a small set of
built-in shapes/behaviors) if a fully trait-based approach feels heavy.
The important part is the separation: **generation**, **behavior**,
**scene composition** — and, since ADR-011, **shape**.

`SurfaceShape` splits three ways because a shape's work divides cleanly by
how often it needs doing, and the per-particle loop runs tens of thousands
of times per step. `frame` runs once. `deform` is the noise, which is
essentially the whole simulation cost but varies slowly, so the behavior
refreshes it for a rotating fraction of the population each step rather
than all of it. `place` runs for every particle every step and is nearly
free. Collapsing these into one `sample` call is the obvious design and it
caps the point budget at roughly a quarter of what the references need,
because it forces the noise to be re-evaluated at the frame rate for
motion that takes seconds to develop.

### 5.2 Shape Vocabulary — Two Shapes, Not Six

The first revision of this section listed a volume-centric *generator* set
(`VolumetricCloudGenerator`, `ClusterGenerator`, `ResonanceFieldGenerator`,
`RingGenerator`, `ArcGenerator`). ADR-011 replaced those with a shape per
state, since points belong on skins rather than through volumes. ADR-012
then collapsed the per-state shapes into one, so the vocabulary is now:

| Shape            | Used by          | Character |
|------------------|------------------|-----------|
| `PresenceShell`  | Every mode       | Radius is a weighted sum of terms: `fold` (the resting identity), `lobes` (thinking), `pulse` (speaking), `neck` (tool use) |
| `ResonancePlate` | Loading          | Stationary viewer-facing sheet; sand redraws as the drive frequency steps between resonances |

The `RisingLobes` / `DrippingShell` / `PulsingShell` entries this table used
to carry are gone. Each was going to be a whole shape whose only difference
from the idle shell was one extra deformation, and a state that combined two
of them would have needed a third shape written by hand. As terms on one
shell they compose for free — see ADR-012 for the full reasoning.

`ProgressArc` and any multi-agent secondaries remain separate *entities*
rather than terms, because they are additional objects in the scene rather
than things the presence is doing.

**Adding a state is now adding a term, not a shape.** A term is a function
of the seed direction and the frame, gated on its weight so it costs nothing
while its mode is disengaged. Keep it to a handful of operations per
particle: `deform` runs on a quarter-rate stagger, but `place` runs for
every particle every step, which is why `speaking` — the smallest term — is
the most expensive state.

Seeding is shape-independent — it depends only on the shape's *domain*
(unit directions for shells, unit-square coordinates for sheets), so one
`SurfaceGenerator` covers every shape. Keeping the generator shape-free
means the shape's noise state has exactly one owner, so the skin a
particle is sprung toward can never disagree with the skin it was seeded
on.

### 5.3 Built-in Behaviors (starting set)

- `SurfaceBehavior` — springs points onto whatever `SurfaceShape` it holds.
  This is the only behavior Phase 1 needs, for both scenes.
- `PulseBehavior` — radial or axial waves (speaking), if amplitude response
  turns out to need to bypass the shape's refresh cadence.
- `DriftBehavior` — low-energy noise drift (fallback calm).

`ViscousClusterBehavior` and `ResonanceBehavior` from the first revision
are gone: what distinguished them was their *geometry*, and geometry now
lives in the shape. One behavior driving many shapes is the payoff of the
split, and it is why adding Thinking or ToolUse is a term on the shell, not
an engine change.

`PulseBehavior` stayed hypothetical for the same reason. Speaking turned out
not to need to bypass the shape's refresh cadence but to sit in the shape's
`place` step, which already runs at full rate — one line of the existing
shape rather than a parallel behavior.

New states are preferably created by adding a term to `PresenceShell` (§5.2),
then by writing a new `SurfaceShape` and reusing `SurfaceBehavior`, and only
then by writing an entirely new behavior. A new behavior is warranted only
when the *motion model itself* differs — not when the form does.

### 5.4 Scene Registration

A simple registry allows both built-in and user-supplied scenes:

```rust
struct SceneRegistry {
    scenes: HashMap<SceneId, SceneDefinition>,
}

impl SceneRegistry {
    fn register(&mut self, id: SceneId, definition: SceneDefinition) {
        self.scenes.insert(id, definition);
    }

    fn get(&self, id: &SceneId) -> Option<&SceneDefinition> {
        self.scenes.get(id)
    }
}
```

Future scenes register here rather than requiring changes to the renderer
or the Scene Director.

---

## 6. Transitions

### 6.1 Principles

- Prefer continuous parameter and density morphing over instantaneous swaps.
- Duration typically 400–1200 ms depending on how different the source and
  target scenes are (consistent with `PRESENCE_VISUAL_ENTITY.md`'s
  300–900ms mode-transition guidance and the shell's existing 420ms
  crossfade convention).
- Maintain approximate point count during the transition to avoid popping.
- Secondary entities can fade in/out independently of the primary morph.

### 6.2 Simple Transition Model

```rust
struct Transition {
    from: SceneId,
    to: SceneId,
    progress: f32,   // 0.0 -> 1.0
    duration: f32,
    easing: Easing,
}
```

During a transition the system can:

1. Lerp shared parameters (intensity, color bias, noise scale, etc.).
2. Cross-fade or spatially blend the two entity sets.
3. Gradually change force fields from the source behavior toward the target behavior.

Hard cuts are reserved for urgent attention events only.

### 6.3 Scene Director Responsibilities

The Scene Director:

- Receives high-level state from the assistant (mode, loading, progress, errors, etc.).
- Selects the target Scene.
- Initiates and manages Transitions.
- Enforces visual hierarchy (primary entity remains dominant).
- Applies global signals (intensity, audio level, etc.) to the active scene.

It should remain a relatively thin layer. In the Phase 1 prototype, "high-
level state from the assistant" comes from dev controls (keyboard
shortcuts / an `egui` debug panel), not real assistant-core signals — that
wiring is Phase 2/3 (`PRESENCE_INTEGRATION_PLAN.md` §4).

---

## 7. Signals That Drive Scenes

```rust
struct PresenceSignals {
    intensity: f32,                  // 0.0-1.0
    audio_level: f32,                // 0.0-1.0
    progress: Option<f32>,           // 0.0-1.0 when applicable
    confidence: Option<f32>,
    direction: Option<Vec3>,
    /// Free-form key/value for advanced or custom scenes.
    custom: HashMap<String, f32>,
}
```

This is the same `PresenceSignals`/`PresenceState` shape already recorded
in `PRESENCE_VISUAL_ENTITY.md` §5.2 (with the same privacy constraint:
derived scalars only, never raw audio/transcript content). Built-in scenes
map a subset of these signals. Custom scenes may use the `custom` map or
ignore signals they do not need.

---

## 8. Extension Guidelines (for future developers)

First decide whether you are adding a **mode** or a **scene**. A mode is
something the presence is *doing*, and can be true at the same time as other
modes; a scene is a different arrangement of entities. Most additions are
modes, and modes are much cheaper.

To add a new mode (§4.3):

1. Decide the intent and the visual character (keep it inside the scanned point language).
2. Add a `PresenceMode` variant with its profile: which term it raises, and its intensity/cool/expand targets and attack/release.
3. Add the term to `PresenceShell`, gated on its weight, and resolve its per-frame values in `frame` so the per-particle work stays small.
4. If the term responds to a live signal, check the signal's rate against the surface behavior's spring bandwidth first (`PRESENCE_VISUAL_ENTITY.md` §6) — anything much above 1 Hz has to drive brightness rather than position.
5. Confirm the shell still stays inside its radius band when the new term runs alongside the existing ones.

To add a new entity kind (Phase 4 addendum):

An `EntityKind` (`presence-core/src/scene/entity.rs`) is a stable
identity tag for a distinct kind of visual object — different from
a *scene* (which is a way of composing entities) and different from
a *mode* (which is state layered on top of the shell). Most new
work is a scene or a mode. Add a new kind when the *motion model*
or the *point-generator contract* actually differs from anything
that already exists — e.g. a physically-simulated splash, a
volumetric fog layer, a text glyph swarm. Reuse the two existing
kinds (`AssistantCloud`, `LoadingRing`) whenever the new visual
can be expressed as a `SurfaceGenerator` + `SurfaceBehavior`
pair; the shell is already tuned around that pair's cost profile.

1. Confirm the new kind actually needs its own type. If the motion
   is a surface deform with a spring settle, it is
   `SurfaceBehavior` in different clothing — write a new
   `SurfaceShape` (path above) instead, and stop here.
2. Implement `PointGenerator` (`presence-core/src/sim/generators.rs`)
   for the new kind. `generate(budget, params) -> Vec<Particle>`
   must be deterministic on `(budget, params)` so the quality-tier
   path can regenerate without a re-seed. Keep the per-particle
   work bounded — see §8 "Point budgets" above.
3. Implement `PointBehavior`
   (`presence-core/src/sim/behaviors.rs`) for the new kind. `place`
   runs every particle every step; `deform` runs on a quarter-rate
   stagger. Anything faster than ~15 Hz belongs in `place`;
   everything else in `deform`. This is the single biggest
   performance decision in a new kind.
4. Add the variant to `EntityKind` (`scene/entity.rs`) and give it
   a label in `EntityKind::label`. The label is what appears in
   the debug overlay and in future telemetry — pick something
   grep-able.
5. Instantiate one in `SceneDirector::new` (`scene/director.rs`)
   alongside `assistant_cloud` / `loading_ring`. Pick a `priority`
   (higher wins on the crowding rules — see §4.3). Wire it into
   `set_quality_tier` so a tier switch regenerates its points at
   the new budget the same way the existing entities do.
6. If the new kind is engaged by a mode or a signal (rather than
   always-on like the assistant shell), map the trigger in the
   same file's mode-toggle / signal-consumer paths.
7. Register a `SceneId` for at least one scene that uses the new
   kind (`scene/registry.rs::SceneRegistry::with_builtin_scenes`).
   The `builtins_match_the_scene_director` test guards against
   registry drift; leave it to fail first as the reminder.
8. Add a test that the new kind's `update` stays inside the shell
   radius band under representative signal input, and add a
   screenshot to `presence-prototype/README.md`. If the kind can
   coexist with another entity, add a `*_dampens_active_modes_*`
   pattern test too.
9. Document the new kind's cost profile: how many noise
   evaluations `deform` does, how many term multiplications
   `place` does, and its measured FPS at the Balanced budget on
   the reference machine. `PRESENCE_VISUAL_ENTITY.md` §9 is where
   these numbers live.

To add a new scene:

1. Decide the intent and the visual character.
2. Write a new `SurfaceShape` in `presence-prototype/src/sim/shapes.rs`
   (§5.2). Reuse `SurfaceBehavior` (§5.3) unless the *motion model*
   genuinely differs — a new form does not need a new behavior.
3. Add an `EntityKind` variant in `scene/entity.rs` if the scene is a new
   kind of thing rather than a variant of an existing one. Purely visual
   variants of an existing kind can reuse it.
4. Register the scene in `scene/registry.rs::SceneRegistry::with_builtin_scenes`
   with a stable `SceneId`, its `EntityKind`, `priority`, and `default_active`.
   The `builtins_match_the_scene_director` test in `scene/director.rs`
   will fail until step 5 is done — that is intentional, the failing test
   is the reminder.
5. Instantiate the scene in `scene/director.rs::SceneDirector::new` as a
   sibling of `assistant_cloud` / `loading_ring`. Point the entity's
   `SurfaceGenerator` at the new shape's `domain()`, wrap the shape in
   `SurfaceBehavior`, and pass it through the `EntityInstance` constructor.
   Wire the quality tier's point budget/deform stride the same way the
   existing entities do (see `set_quality_tier` for the runtime path).
6. Map the relevant assistant modes or signals to the scene in the same
   file, alongside the existing `set_ring_wanted` / mode toggles.
7. Define at least one reasonable transition path back to Idle — for
   most scenes this is the `presence` fade already provided by
   `EntityInstance` and driven by the `TRANSITION_SECONDS` constant.
8. Add a test that engaging the scene subdues siblings appropriately
   (see `loading_dampens_active_modes_without_stopping_them` for the
   pattern), and add a screenshot to `presence-prototype/README.md`.

**Point budgets.** This section previously said a complex scene should
rarely need more than ~8–12k points. That number was written against a
volumetric fill and does not survive the move to surfaces: a volume hides
most of its points behind its own front, while a surface concentrates
every point where it is individually visible, so the same count that reads
as a dense volume reads as countable dots on a skin. The measured budget
is 40,000–80,000 per entity — see `PRESENCE_VISUAL_ENTITY.md` §9 for the
measurement and the hardware it was taken on. Budgets are still
*measured*, not assumed; that part of the original guidance stands.

A new shape's cost is dominated by how many noise evaluations its `deform`
needs, since `place` is nearly free. Keep `deform` to a small fixed number
of evaluations with no iteration — this is exactly why shapes are
parametric rather than implicit surfaces projected onto per frame.

A new *term*'s cost is dominated by which half of the trait it sits in.
`deform` runs on a quarter-rate stagger; `place` runs for every particle
every step. This is why `speaking` — a dot product and a sine, the smallest
term on the shell — is the most expensive state to be in, and why a term
belongs in `place` only when it has to move faster than roughly 15 Hz.

Document any new custom signals introduced so other scenes do not collide with them.

---

## 9. Recommended Phase 1 Prototype Scope

This supersedes the more general "at least four states" scope originally
sketched in `PRESENCE_INTEGRATION_PLAN.md` §4 Phase 1 — see that document
for the up-to-date version:

1. **Idle** — the Presence Shell at rest (§4.1; originally specified as the
   viscous cloud, which relocated to Thinking/ToolUse — see §4.3)
2. **Loading** — Resonance Field (Chladni-style)
3. Basic Scene Director that switches between them on dev-control input
4. One solid transition style (parameter + density morph)
5. Clear registration point (`SceneRegistry`) so additional scenes can be
   added later without touching the renderer
6. **Thinking / Speaking / ToolUse** as weighted terms on the shell from
   (1), with a mode layer holding the engaged set and its eased ramps. These
   were planned for a later phase; ADR-012 pulled them forward because as
   terms they cost a fraction of what a shape each would have.

`MultiAgent` and `ProgressArc` still follow the entity pattern rather than
the term pattern, and remain later phases. `listening`, `error`, and
`attention` need no geometry at all.

---

## 10. Design Rules Recap

- All scenes share the same material (soft points, the brand-reconciled
  palette, near-black field).
- Idle must remain the calmest state.
- Active scenes must be clearly distinguishable from Idle and from each other.
- Custom scenes should not break the scanned-viewport metaphor.
- Prefer composition over new code.
- Transitions should feel continuous.

---

*This is the working foundation for the scene and transition system. Keep
it simple, keep it extensible, and let real usage drive additional entity
types. Update alongside `PRESENCE_VISUAL_ENTITY.md` and
`PRESENCE_INTEGRATION_PLAN.md` when scenes, generators, or behaviors change.*
