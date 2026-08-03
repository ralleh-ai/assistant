# Point Cloud Presence — Live System Scanner

**Project**: Ralleh Assistant (`ralleh-ai/assistant`)
**Component**: Always-on desktop visual presence
**Status**: Design locked → Phase 1 Rust prototype in progress (see
[`PRESENCE_INTEGRATION_PLAN.md`](./PRESENCE_INTEGRATION_PLAN.md))
**Origin**: Adapted from an external concept note. This is the **second,
corrected revision** of this design doc — it supersedes the first pass
(originally titled "Visual Entity Design") after the source concept was
itself revised to make the production path **Rust-first** (`winit` +
`wgpu`) rather than a Three.js/Tauri-webview target. Section 3.1 (palette)
is the one deliberate deviation from the source concept, kept from the
first revision — see the note there.
**Surface revision (ADR-011)**: §3.1, §3.2, §3.3, §4.1, §7.3, and §9 were
revised after the Phase 1 prototype established that points must lie *on*
surfaces rather than fill volumes. Everything else — the vision, the
principles, the state system, the entity system, the motion engine —
stands unchanged; the surface model is how those get realized, not a
change to what they are.
**Additive-mode revision (ADR-012)**: §5.1 and §6 were revised after
`thinking`, `speaking`, and `tool_use` landed. The modes in §5.1 are still
the modes; what changed is that they compose as weighted terms on one shell
instead of selecting between shapes, and that §6's curl-noise
recommendations no longer apply to a surface. §6 also gains the spring
bandwidth rule, which constrains any future signal-driven motion.
**Last Updated**: 2026-08-02

---

## 1. Vision & Positioning

Most desktop AI assistants reduce themselves to a glowing orb, a waveform,
or a vibrating circle. These are status indicators. They are not presence.

**Ralleh takes a different approach.**

The always-on visual is a **live LiDAR-style viewport into the system's
internal processes**. It does not show a character or a logo. It shows an
abstract, continuously scanned representation of what the intelligence
layer is actually doing — perceiving, computing, speaking, using tools,
loading, coordinating, or waiting.

The core metaphor:

> You are looking through a real-time scanner at the living state of the assistant.

When executed well this becomes a genuine differentiator in the
Jarvis-class space: the interface stops being decoration and starts being
a transparent window onto computation and agency.

### What the user should feel

- At a glance they understand the current mode and relative intensity.
- Over a few seconds they can read the *nature* of the activity (listening
  vs deep thinking vs tool execution vs loading).
- The visual never feels random. Every change is grounded in real system state.
- Idle is calm and alive. Activity is informative and energetic without becoming fatiguing.

The window is not the assistant's face. It is the assistant's
**instrument panel rendered as living geometry**.

---

## 2. Design Principles

1. **Viewport, not avatar**
   The window is a scanned volume. Entities appear inside it the way
   objects appear in a LiDAR feed. There is no ground plane, no fixed
   stage, no humanoid form.

2. **State is the only driver**
   Visual change must map to real assistant or system state. Aesthetic
   motion that is not informative is noise.

3. **Calm by default, legible under load**
   Idle and light listening must remain gentle. High activity is allowed
   to become denser and more structured, but the system must return
   cleanly to calm.

4. **One coherent visual language**
   Every entity (main cloud, loading ring, progress structure, secondary
   agent, data stream) is made of the same point material and governed by
   the same noise + force rules. This keeps the "scanned" aesthetic consistent.

5. **Procedural and continuous**
   No animation clips. Entities are dynamical systems whose parameters are
   driven by live signals.

6. **Informative over decorative**
   A user watching for 10–15 seconds should be able to infer mode and
   relative intensity without reading text.

7. **Hierarchy of attention**
   The primary entity dominates. Secondary entities are sparser or lower
   intensity so the view never becomes visual noise.

8. **Native to the existing shell's motion language**
   Transition timing and the "no hard cuts except urgent attention"
   principle should read as consistent with `desktop-edge`'s existing
   420ms crossfade convention, even though this entity is rendered by a
   completely different pipeline than the React UI.

---

## 3. Core Visual Language

### 3.1 Material

All entities are composed of soft points on a near-black field.

**Palette — a user setting, defaulting to brand-reconciled teal.** The
source concept specifies a classic LiDAR lime → yellow-green primary
palette. This repo already has an established dark-teal brand identity
(`--ink #0e1614`, `--teal #1f8a7a`, `--teal-deep #146257`, `--mist #d7e4df`,
`--foam #f3f7f5`, `--amber #c4a574` — see `desktop-edge/src/App.css`). A
lime-green scanner sitting inside a teal-branded product would read as an
unrelated overlay, not "the entity," so **teal is the default**.

It is not, however, the only option. The presence is the assistant's
visual character, and which hue that character wears is the operator's
choice — so the palette is runtime data selected from a named preset, not
a compile-time constant (ADR-011). The prototype ships four:

| Preset | Character |
|---|---|
| `teal` (default) | The brand reconciliation described above |
| `lime` | The source concept's original LiDAR lime/yellow-green |
| `ice` | Cool blue-white, colder and more clinical |
| `ember` | Warm amber, the shell's "needs attention" accent as a whole scheme |

A preset is five stops — `calm`, `body`, `cool`, `hot`, `accent` — plus
the shared `ink` field colour, which every preset holds in common because
the field is the window's background rather than part of the entity's
identity. The `accent` stop is *derived* from `body` rather than
hand-picked, by driving it to full chroma: creases are the same material
catching more light, not a different material, so deriving it keeps every
preset in family, including presets added later.

The design intent behind the table below (warm/calm ↔ cool/intense colour
temperature shift, sparing secondary hues for state signalling) is a
property of the *stops*, not of any particular hue, so it survives a
preset change intact. The rows are written in brand-teal terms since that
is the default:

| State bucket | Hue | Notes |
|---|---|---|
| Idle / calm | `--foam` at low brightness, faint `--teal` undertone | Reads as "the same living thing," not a different object |
| Listening | `--foam` → `--mist`, brighter halo | Gentle lift, no hue shift yet |
| Thinking / heavy compute | Shifts toward `--teal` / `--teal-deep` (cooler), occasional near-white hotspots at the densest points | "Cooler under compute" intent preserved with colors already in the palette |
| Speaking | `--foam`/`--mist` with amplitude-driven brightness pulses | No hue shift — legibility of the audio-sync pulse matters more than color here |
| Error / attention | Brief, desaturated shift toward `--amber` | Reuses the existing amber accent (already the shell's "needs attention" color) instead of red/orange |

**Two independent color axes.** The table above mixes them, and conflating
them produces a wrong implementation, so read them separately:

1. **State** — where a mode sits between calm (`--foam` with a faint
   `--teal` undertone) and heavy compute (cooler, toward `--teal-deep`).
   This is the axis the table's rows describe.
2. **Density** — the near-white hotspots. These belong to *any* state and
   must emerge from accumulated point density, never be assigned as a
   colour. Driving hue from energy alone makes idle render in the thinking
   state's colour and leaves the cool shift with nowhere to go.

The second axis has a hard implementation consequence: with additive
blending, summing a teal tint only ever yields a more saturated teal, which
a tonemap clips to vivid green — it can never reach white. Whitening must be
applied to the *accumulated* value, after blending. The prototype does this
as highlight desaturation in its composite pass.

Beyond color, the material rules are unchanged from the source concept:

- Points have soft falloff, size attenuation, and optional very light glow.
- No hard edges, no solid meshes, no textures that break the scanned aesthetic.
- Point tints carry hue only, normalized so their brightest channel is 1.0.
  Points are emissive, so lightness has to come from the energy term alone;
  using `--teal-deep` directly as a tint dims every point that uses it by
  ~7x and makes brightness and hue impossible to tune independently.

**Points sit on surfaces, not through volumes (ADR-011).** A LiDAR return
only ever comes from a skin, so a volume fill can never read as scanned no
matter how it is tuned. Two material terms follow from the surface model
and belong here rather than in any one entity's description, because every
entity gets them:

- **Grazing / silhouette.** A point's brightness rises as its surface
  normal turns away from the view direction, so the rim of a form is its
  brightest part. This is what makes a point cloud read as *solid* rather
  than as a haze. A volume has the opposite profile — brightest at its
  centre — which is why the first prototype read as a nebula.
- **Crease.** A `0..1` fold intensity per point, lifting brightness and
  pulling tint toward `accent`. This draws the bright filaments where a
  surface folds, and it is the term that carries fine structure. Only the
  points actually on the skin report a crease; the layers floating off it
  (§3.3) report none, or the filaments smear into a glow.

### 3.2 Form Vocabulary

| Property            | Calm / Idle                  | Active / Structured                     |
|---------------------|--------------------------------|-------------------------------------------|
| Density             | Diffuse, soft core            | Higher core density, tighter packing    |
| Motion              | Slow drift + breathing        | Curl-driven swirl, directional flow, ordered rotation |
| Point size          | Smaller, consistent           | Variable; core or structure points larger |
| Brightness          | Soft                           | Higher, with controlled hotspots        |
| Color temperature   | Warm (`--foam`/`--teal`)      | Cooler (`--teal-deep`) under heavy compute |
| Structure           | Amorphous cloud                | Temporary coherent forms (rings, arcs, filaments, lattices) that form and dissolve |

"Amorphous cloud" describes the *impression* idle should leave, not its
construction: idle is a folded shell whose silhouette changes slowly enough
to read as soft (§4.1). Point size is the one row measurement moved
against the original text — a surface needs far more, far smaller points
than a volume at the same apparent detail, so sizes are tight and
near-uniform across all states.

### 3.3 Layering (within a single entity)

- **Core** — on the skin. The main population and the layer that carries
  the silhouette and the creases.
- **Body** — a thin scatter just off the skin, inside and out.
- **Halo / Aura** — sparse points drifting outside the skin only, giving
  atmosphere without blurring the silhouette.

These are density and behavior gradients within one point population, not
separate meshes.

The surface model inverts what `Core` means. Volumetrically it was a
minority of points at the centre of the fill; on a skin it is the skin
itself, so it is the *majority* — typically 60% or more. `Halo` sits
outside the skin only, never inside, since points behind the surface are
occluded by the thing they are meant to be the atmosphere around.

---

## 4. Entity System (The Standout Capability)

The presence is not a single fixed cloud. It is a **scene of entities**
living inside the scanned volume.

The main assistant cloud is the default entity. The system can replace it,
morph into other forms, or display multiple entities simultaneously. This
is what turns the window into a true internal-process viewport — and it's
the reason this repo's companion doc is called
[`PRESENCE_SCENES.md`](./PRESENCE_SCENES.md): the Scene/Entity/Generator/
Behavior/Director architecture is what actually implements this section.

### 4.1 Entity Types (Initial Set)

| Entity                | Purpose                                      | Visual Character                                      |
|-----------------------|-----------------------------------------------|----------------------------------------------------------|
| `AssistantCloud`      | Default presence                             | Folded shell — a closed, creased skin with a bright grazing rim and fold filaments across its face |
| `LoadingRing`         | Application or long-running process loading  | A stationary sheet of sand resolving into Chladni figures, redrawn each time the driving frequency steps |
| `ProgressArc`         | Quantified progress                          | Arc or spiral that fills or travels                   |
| `DataStream`          | Inbound or outbound information flow         | Directed particle streams from edges toward core      |
| `SecondaryAgent`      | Multi-agent or background task               | Smaller, simpler cloud or satellite form              |
| `AttentionPulse`      | Explicit wake or focus event                 | Brief high-energy expansion then settle               |
| `ErrorFragment`       | Attention-needed or recoverable error        | Jittery, partially desaturated, possibly broken form  |

Additional entity types can be added later as long as they obey the same
material and motion rules. **Phase 1 prototype scope is `AssistantCloud` +
`LoadingRing` only** — see `PRESENCE_INTEGRATION_PLAN.md` §4.

Two rows changed against the source concept, and in both cases the name is
now slightly wrong while the entity's role is not.

`AssistantCloud` is a shell, not a cloud. The name is kept because it is
the entity's identity across every mode, and the shape it wears is a
per-mode property (`PRESENCE_SCENES.md` §5.2) rather than part of what the
entity *is*. "Folded shell" is idle's shape; `thinking` and `tool_use`
will wear others.

`LoadingRing` is neither a ring nor rotating. **Rotation was removed
deliberately**: a form turning on its own axis is a loading spinner, and a
spinner is decoration standing in for status rather than status itself
(§6, "informative over decorative"). It carries no information — it looks
identical whether work is progressing or wedged. The Chladni figure
carries real information in its stead: the figure's complexity tracks
load, and it visibly *redraws* each time the drive steps, which a stalled
system cannot fake.

### 4.2 Entity Lifecycle

Each entity has:

- A **generator** — how points are initially distributed (cloud volume, ring, arc, etc.)
- A **behavior controller** — the forces, noise parameters, and rules that update it each frame
- **Transition rules** — how it appears, morphs, and disappears

Preferred transitions:

- Density morph / cross-fade (preferred)
- Spatial hand-off (one entity dissolves while another coalesces in place)
- Hard cuts only for urgent attention events

### 4.3 Multi-Entity Scenes

The viewport can contain more than one entity at a time:

- Primary assistant cloud + subtle loading ring for a background task
- Main cloud + inbound data streams during heavy tool use
- Multiple smaller agent clouds when multi-agent work is active

A simple priority and intensity hierarchy keeps the view readable.
Secondary entities should generally use fewer points and lower brightness.

---

## 5. State & Signal System

### 5.1 Primary Modes (`AssistantCloud`)

| Mode         | Intent                                      | Visual Signature                                                                 |
|--------------|-----------------------------------------------|--------------------------------------------------------------------------------------|
| `idle`       | Waiting                                     | Slow breathing, soft noise drift, the shell's resting fold, calm color            |
| `listening`  | Microphone / VAD active                     | Gentle expansion, micro-jitter, slight directional bias, brighter halo           |
| `thinking`   | Model inference                             | Bulges that gather, rise, thin, and are reabsorbed; cooler shift                 |
| `speaking`   | Audio output                                | A wave travelling across the skin, plus syllable-rate brightness                 |
| `tool_use`   | External action executing                   | One pendant per call, extending and retracting; higher overall energy            |
| `error`      | Recoverable problem                         | Desaturation or brief amber tint, elevated jitter, possible contraction          |
| `attention`  | Explicit wake or focus                      | Bright pulse + expansion, then settle into listening or thinking                 |

**Modes are a set, not a slot (ADR-012).** They compose: an assistant
narrates a tool call while it is running it, and keeps thinking while it
speaks, so "the current mode" is not a thing the system represents. Each mode
raises a weighted term on one `PresenceShell` rather than selecting a
different shape, which is why concurrency needs no special case and why the
list above describes *contributions* rather than alternatives.

`idle` is the absence of the others rather than one of them, and is exactly
what the shell degenerates to when every other weight is zero. `listening`,
`error`, and `attention` need no geometry at all — they are colour,
brightness, and framing changes — so they sit on the same weight machinery
without adding shell terms.

Transitions are per-weight lerps, eased, typically 300–900ms. Because the
terms are additive, a transition never cross-fades one population into
another: the particle set is untouched and the same points follow the same
spring to a slightly different skin. A transition interrupted mid-flight
reverses from where it actually is.

`thinking`'s signature is the one entry revised from the first edition, which
read "core densifies, strong curl swirl". See §6 for why curl no longer
applies after ADR-011, and `PRESENCE_SCENES.md` §4.3 for what replaced it.

### 5.2 Continuous Signals

These modulate any active entity:

- `intensity` (0–1) — overall energy and turbulence
- `audio_level` (0–1) — input or output amplitude
- `progress` (0–1) — for loading and quantified work
- `confidence` (optional) — affects coherence vs noise
- `direction` (optional vec3) — listening orientation or focus bias

**Privacy constraint (unchanged from the first revision):** these signals
must only ever be derived scalars. Raw audio samples, transcript text, or
prompt/completion content must never flow into this system — see
`PRESENCE_INTEGRATION_PLAN.md` §6 (threat-model cross-references, T9/T14).

### 5.3 Scene-Level Events

Higher-level events that the Scene Director reacts to:

- Assistant mode change
- Tool start / end
- Long-running process start / progress / complete
- Multi-agent activity
- Explicit user attention / wake
- Error conditions

---

## 6. Noise & Motion Engine

Organic movement is produced by a combination of:

- **Simplex / OpenSimplex noise** — base displacement, density variation, slow drift, breathing modulation
- **Curl noise** — primary velocity field for fluid, swirling, divergence-free motion (especially important for `thinking` and high-intensity states)
- **FBM (multi-octave)** — adds natural multi-scale detail under higher intensity
- Light **domain warping** (optional, advanced) for extra organic folding

### Recommended usage by context

| Context              | Primary driver                          | Notes |
|----------------------|--------------------------------------------|-------|
| Idle                 | Ridged multi-octave Simplex on the shell   | Displacement doubles as the crease value |
| Listening            | Elevated micro-jitter + mild expansion     | Weak directional bias allowed |
| Thinking             | Frame-resolved gaussian bulges             | Highest internal complexity; no noise per particle |
| Speaking             | Deterministic wave + brightness            | Rhythm must stay readable — see the bandwidth rule below |
| LoadingRing          | Discrete modal figures + light noise       | Still feels scanned, not geometric |
| Tool / Data streams  | Localized radial reach + waist             | Clear sense of flow |

**Curl noise no longer applies to any of these, and that is a consequence of
ADR-011 rather than a change of taste.** Curl is valuable precisely because
it produces divergence-free vortices *through a volume* — and after the
switch to surfaces there is no volume for points to move through. Applied to
a skin it simply moves points off it, and the only visible result is that the
form goes fuzzy. `NoiseField::curl` is retained for a future `DataStream`
entity, where flow through open space is the actual subject.

### The spring's bandwidth is a hard constraint on signal-driven motion

Anything that moves a particle's *position* passes through
`SurfaceBehavior`'s damped spring, which sits near 0.7 Hz. A second-order
system passes roughly two percent of a signal ten times its corner, so a
signal much faster than about 1 Hz will not reach the skin at all — the shell
sits still while the state insists something is happening. This is invisible
in a screenshot and looks like a tuning failure, so it is worth stating as a
rule:

> Signal-driven motion must sit inside the spring's bandwidth, or be routed
> to a channel that isn't sprung.

Brightness, colour, and point size are assigned rather than integrated and
are therefore instant; position is not. `speaking` is the case that forced
this out into the open: its geometry follows a smoothed phrase envelope while
its 4–7 Hz syllable rhythm goes to brightness. Raising the spring's stiffness
to chase syllables instead would take roughly seventy times the stiffness,
and the spring is shared by every mode and both shapes — it would trade
§2.3's softness everywhere for one state's responsiveness.

---

## 7. Technical Architecture (Rust-First)

**This is the key correction from the first revision of this document,**
which had assumed a Three.js/Tauri-webview production target. The
production path is Rust, end to end.

### 7.1 Recommended Production Stack

- **Windowing**: `winit`
- **Rendering**: `wgpu`
- **Noise**: `noise` crate (or equivalent Simplex implementation) + a curl-noise implementation
- **Integration**: direct from the existing Rust assistant core (shared state, channels, or interior mutability as appropriate)

This path is preferred over a JavaScript/Three.js frontend for the
presence surface because:

- Zero interop cost for high-frequency state and particle updates
- Superior performance and control for the simulation loop
- Tight coupling with the rest of the Rust codebase (this repo's assistant
  core — policy/audio/ai-router/tool-gateway — is already Rust; ADR-002
  already established a Rust-first edge core)
- Cleaner handling of frameless, always-on-top, and transparency requirements
- Smaller and more predictable runtime footprint

Three.js remains useful only for **rapid visual prototyping** of motion
language if ever needed as a throwaway spike — it is explicitly **not**
the target for this repo's Phase 1 prototype. See ADR-010 (revised) and
`PRESENCE_INTEGRATION_PLAN.md` for why the Rust path was chosen directly,
skipping the JS spike entirely.

### 7.2 High-Level Data Flow

```
Rust Assistant Core + System Events
              │
              ▼
     Scene Director
              │
              │  (active entities + parameters)
              ▼
   Entity Controllers (generators + behaviors)
              │
              ▼
   Particle Simulation (positions, velocities, forces)
              │
              ▼
   wgpu Render Pipeline (points + soft shaders)
```

### 7.3 Core Concepts (Rust)

```rust
enum EntityKind {
    AssistantCloud { mode: PresenceMode },
    LoadingRing { progress: f32, speed: f32 },
    ProgressArc { value: f32 },
    DataStream { direction: Vec3, intensity: f32 },
    SecondaryAgent { id: AgentId },
    // ...
}

struct PresenceSignals {
    intensity: f32,
    audio_level: f32,
    progress: Option<f32>,
    confidence: Option<f32>,
    direction: Option<Vec3>,
}

struct Particle {
    position: Vec3,
    velocity: Vec3,
    /// The particle's fixed seed on its shape's surface — a unit direction
    /// for shells, a disk coordinate for plates. Stable for the particle's
    /// life, so it keeps its identity on the skin as the skin deforms.
    base_offset: Vec3,
    /// Outward surface normal, driving §3.1's grazing term.
    normal: Vec3,
    /// 0..1 fold intensity, driving §3.1's crease term.
    crease: f32,
    size: f32,
    brightness: f32,
    // entity ownership or layer tags as needed
}
```

`normal` and `crease` are the surface model's cost at the render boundary
(ADR-011): the per-instance record grows from 32 to 48 bytes. That is the
whole price, and it buys the two terms §3.1 describes.

Unlike the Rust pseudocode in `PRESENCE_SCENES.md`'s first revision (which
was a conceptual stand-in for what would actually be TypeScript), these
are the **literal** shapes the Phase 1 prototype implements — see
`PRESENCE_INTEGRATION_PLAN.md` §4 and the prototype crate itself.

A small number of large point buffers (or one buffer with entity tagging)
is updated each frame and uploaded to the GPU. Simulation stays
lightweight: noise evaluation + simple forces + damping. Full physics is
unnecessary.

### 7.4 Rendering Notes

- Point list with soft circular falloff (shader)
- Size attenuation
- Optional very light bloom (use sparingly)
- Near-black clear color (brand `--ink`, not pure `#000`)
- No shadows, no complex lighting model

Target: solid 60 fps on the hardware the assistant is expected to run on,
even with 2–3 concurrent entities. `wgpu` does not expose a portable
`gl_PointSize`-equivalent across all backends, so the renderer uses
instanced billboarded quads (one small quad mesh + a per-particle instance
buffer), not the `PointList` primitive topology — see
`PRESENCE_INTEGRATION_PLAN.md` for the concrete rendering approach.

---

## 8. Scene Orchestration

The **Scene Director** is the component that decides what the viewport shows.

Responsibilities:

- Receive high-level events from the assistant core and system
- Decide the set of active entities and their parameters
- Manage transitions (morph, cross-fade, hand-off)
- Enforce visual hierarchy so the primary entity remains dominant
- Keep secondary entities from overcrowding the volume

Example behaviors:

- Assistant goes into deep thinking → `AssistantCloud` intensifies, temporary micro-structures appear
- A long tool or external process starts → `LoadingRing` or `ProgressArc` appears (or the main cloud morphs toward a more ordered form)
- Multiple background agents become active → small `SecondaryAgent` clouds appear at lower intensity
- Process completes → ordered form dissolves back into the calm cloud

The Director should prefer continuous parameter changes over abrupt entity
swaps whenever possible.

---

## 9. Development Keys & Tunables

Expose these early (especially in development builds):

```rust
struct PresenceConfig {
    point_count: u32,              // 40,000-80,000 for primary. See the
                                    // note below — this range is set by
                                    // measurement and by the surface
                                    // model, and it replaces the original
                                    // 2,500-12,000 guidance entirely.
    core_density_bias: f32,        // how much of the population sits
                                    // exactly on the skin (§3.3)
    noise_scale: f32,
    noise_speed: f32,
    breathing_amplitude: f32,
    breathing_speed: f32,
    damping: f32,
    max_speed: f32,
    curl_strength: f32,
    fbm_octaves: u32,

    // Shape (ADR-011) — the parameters of the surface the points live on.
    // Grouped per term (ADR-012), since a mode raises a term's weight
    // rather than swapping the shape out.
    fold: FoldTerm,                // depth, scale, evolution,
                                    // crease_threshold — the resting shell
    lobes: LobeTerm,               // depth, width, period, travel
                                    // (thinking)
    pulse: PulseTerm,              // depth, wavenumber, speed, axis, floor
                                    // (speaking)
    neck: NeckTerm,                // reach, tip_width, pinch, waist_at,
                                    // waist_width (tool_use)
    spin_speed: f32,               // revolutions/sec of the slow turn

    // Material terms the surface model adds (§3.1)
    grazing_boost: f32,
    crease_boost: f32,

    // Colour (ADR-011)
    palette: PaletteId,            // teal (default) | lime | ice | ember
    calm_undertone: f32,           // how far the calm stop is pulled
                                    // toward the signature hue
    // per-mode and per-entity multipliers...
}
```

**On `point_count`.** The original text here read "typical range
2500-12000," and `PRESENCE_SCENES.md` §8 said a complex scene should
rarely need more than 8-12k. Both numbers were written against a
volumetric fill and do not survive the move to surfaces. A volume hides
most of its points behind its own front; a surface concentrates every
point where it is individually visible, so the same count that reads as a
dense volume reads as countable dots on a skin. The prototype measures
80,000 for the shell and 40,000 for the loading plate at 2560×1600 in a
release build on **2 cores**: ~150-178 FPS at idle, and ~109-171 across
`thinking`, `speaking`, `tool_use`, and `thinking + tool_use` together — a
comfortable margin over 60 FPS in every state, with no GPU compute path.
Target-hardware confirmation (2026-08-02) held a steady 200 FPS at the
same budget and resolution, closing Phase 4 §1 of
`PRESENCE_INTEGRATION_PLAN.md` — the 2-core dev-machine numbers above
are the floor, not the ceiling, and target hardware sits well above the
60 FPS target for every state we exercised.
The cost driver is the CPU-side noise evaluation, not the draw; see
`presence-prototype/README.md` for what bought that headroom and for why
`speaking` is the most expensive state despite being the smallest term.

Note that a mode does *not* raise the point count. Additive terms deform the
same population, and gating (ADR-012) means an idle presence pays for the
fold term alone however many terms the shell grows.

`core_density_bias` survives the change but means something different. Its
volumetric meaning — how radially centre-weighted the fill is — has no
analogue on a surface, since there is no interior to weight. It now sets
how much of the population sits exactly on the skin: low reads hazy and
soft-edged, high reads as a hard scanned shell with almost no atmosphere.
The halo keeps a floor at either extreme, because an empty layer silently
turns its material gradient into dead code.

Also expose:

- Active modes (a set, not one — ADR-012) and active entities
- The resolved per-term weights, since the terms are what the modes
  actually do and a weight that never leaves zero is a mode that is wired
  up but doing nothing
- Raw signal values
- Simple debug overlay (development only — the prototype implements this via `egui`)

Per-mode toggles in the overlay should be **checkboxes, not a radio group**.
The modes compose, and the overlaps are the part worth judging by eye; a
single-select control makes the composition unreachable by hand and so
untestable.

Color ranges and transition durations should be configurable, and should
draw from the palette presets in §3.1, not raw hex literals scattered
through shader code. The palette is the one tunable here that is **not**
development-only — it is a user setting, and the debug panel's live
selector exists to prove the plumbing the settings UI will use
(`PRESENCE_INTEGRATION_PLAN.md` §4, Phase 1/2).

The prototype additionally groups the render-path tunables the design above
implies but does not enumerate: point size/glow and depth-haze in a
`PointMaterial`, and bloom threshold/knee/intensity/radius, exposure,
vignette, and highlight-desaturation thresholds in a `PostSettings`. These
belong in the debug overlay for the same reason the simulation values do —
the correct settings for §3.2's "controlled hotspots" depend on the point
budget and brightness budget, so they cannot be fixed constants.

---

## 10. Information Design Rules & Anti-Patterns

**Rules**

- Mode identity is carried by motion character + density + color temperature.
- Intensity is carried by energy, brightness, and turbulence.
- Speaking is the only mode that should show clear outward rhythmic pulses tied to audio.
- Thinking is allowed the most internal complexity.
- Ordered entities (rings, arcs) must still feel scanned — light noise and soft points are mandatory.
- New entity types must introduce a distinguishable combination of the existing visual dimensions.

**Anti-Patterns**

- Constant high-speed chaos (fatiguing)
- Hard geometric shapes that never dissolve
- Over-reliance on color alone
- Too many concurrent entities at high intensity
- Theatrical transitions that lag real state
- Making the viewport compete with the user's actual work

---

## 11. Why This Can Be a Game Changer

Most Jarvis-style interfaces stop at "pretty status". This design makes
the interface a **continuous, abstract, truthful view of the system's
internal activity**.

When the assistant is thinking, you see computation. When a process is
loading, you see structured progress. When tools are firing or agents are
coordinating, you see flow and multiplicity. When it is simply present,
you see calm, living readiness.

Executed with discipline — calm baseline, strict mapping to real state,
coherent material language, and a capable Scene Director — this becomes
more than a visual flourish. It becomes one of the defining
characteristics of the product.

---

## 12. Future Extensions (Post-MVP — not scheduled)

- Reactive "look-at" toward cursor or focused window (subtle directional bias)
- Multiple linked clouds for multi-agent views (formalized by `SecondaryAgent`)
- Very sparse "minimal" mode that still carries state
- Export of short visual clips for debugging or sharing
- Frameless / always-on-top companion window behavior, position
  persistence, opacity/density user settings — these are Phase 2/4 scope
  per `PRESENCE_INTEGRATION_PLAN.md`, not Phase 1.

---

*This document is the source of truth for the Point Cloud Presence / Live
System Scanner. Update it when entities, signals, motion rules, or core
architectural decisions change. For "how/when this gets built against the
actual `ralleh-ai/assistant` codebase," see
[`PRESENCE_INTEGRATION_PLAN.md`](./PRESENCE_INTEGRATION_PLAN.md). For the
concrete Scene/Entity implementation architecture, see
[`PRESENCE_SCENES.md`](./PRESENCE_SCENES.md).*
