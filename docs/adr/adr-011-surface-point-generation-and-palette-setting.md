# ADR-011: Presence Points Lie On Surfaces, Not Through Volumes — and the Palette Is a User Setting

**Status:** Accepted (Phase 1 prototype implements both).

Two decisions are recorded together because they landed together and share
one cause: the Phase 1 prototype reached the point where it could be judged
against the visual concept, and both the form model and the fixed palette
failed that judgement. ADR-010 is unaffected — it chose the Rust/`wgpu`
stack, and everything here is built on that stack unchanged.

## Decision 1 — Points are generated *on* parametric surfaces

Every entity's points sit on a deformable skin described by a
`SurfaceShape`, rather than being distributed through a volume. Concretely:

- A new `SurfaceShape` trait sits **alongside** the
  generator/behavior/scene split from `../PRESENCE_SCENES.md` §5 rather
  than replacing it. It answers "where is the skin right now"; one
  `SurfaceGenerator` seeds a population across its domain, and one
  `SurfaceBehavior` — an ordinary `PointBehavior` using the same damped
  spring as before — pulls particles onto it.
- `Particle` gains `normal` and `crease`; the per-instance GPU record
  grows from 32 to 48 bytes. The point shader gains a grazing/silhouette
  term from the normal and a brightness/tint term from the crease.
- Idle is `FoldedShell` (a ridged-noise-displaced sphere); Loading is
  `ResonancePlate` (a stationary sheet of sand resolving into Chladni
  figures, refactored onto the same trait).
- The volumetric `ClusterGenerator`, `ResonanceFieldGenerator`,
  `ViscousClusterBehavior`, and `ResonanceBehavior` are deleted, not
  deprecated in place.

### Reason

**A volume fill cannot read as scanned, and this is a model problem rather
than a tuning one.** A LiDAR return only ever comes from a surface. Three
consequences follow from the volume model directly, and each is something
the reference concepts have that a volume structurally cannot:

- **No silhouette.** A volume is brightest at its centre; a surface is
  brightest at its grazing rim, because that is where the skin's depth
  along the view ray is greatest. That rim is what makes a point cloud
  read as a solid object rather than a nebula.
- **No creases.** Fold filaments are surface curvature. A volume has no
  surface, so it has no folds to brighten.
- **Wrong point budget and size.** Covering a surface takes far more, far
  smaller points than filling a volume at the same apparent detail,
  because a volume hides most of its points behind its own front.

Months of tuning could not have produced the first two, which is why this
is an architecture decision and not a parameter change.

**Parametric rather than projection onto an implicit surface.** Projecting
each particle onto an SDF every frame needs a field gradient — roughly four
field evaluations plus Newton steps per particle, per step. At the point
counts this needs, that is far outside a 2-core budget. A parametric shell
evaluates displacement along a fixed radial seed in about three
evaluations with no iteration, and hands back an exact normal for free.

**The seed direction is the normal.** Finite-differencing the displacement
would cost roughly six extra noise evaluations per particle. For a
star-shaped radial surface the seed direction is exact at the limb, which
is precisely where the grazing term is read; fold-local normal error is
carried by the crease term instead.

**One ridge value, used twice.** Displacing by *ridged* noise and reusing
the same ridge value as the crease brightness makes creases land exactly
on folds by construction, at zero extra cost. Computing crease intensity
separately is both more expensive and guaranteed to drift out of alignment
with the geometry it describes.

**The three-way split of `SurfaceShape` is what makes the budget
affordable.** `frame` runs once per step, `deform` (the noise, and
essentially the entire simulation cost) refreshes for a rotating quarter of
the population each step, and `place` runs for everything every step but is
nearly free. Idle's folds reshape over tens of seconds, so refreshing a
given particle at 15 Hz rather than 60 Hz is far below what the motion can
resolve — it is invisible, and it is the difference between a budget of
~15k points and one of ~80k. A single `sample()` call is the obvious design
and it is the one that caps the budget at a quarter of what is needed.

### Consequences

- **Point budgets rise by roughly 7x**, from 12,000 to 80,000 for the idle
  shell and 40,000 for the loading plate. Measured at ~130–205 FPS at
  2560×1600 in a release build on 2 cores, so the margin over 60 FPS is
  comfortable and no GPU compute path is needed. This **supersedes** the
  "2,500–12,000" range in `../PRESENCE_VISUAL_ENTITY.md` §9 and the
  "rarely more than 8–12k" guidance in `../PRESENCE_SCENES.md` §8; both
  were written against the volume model.
- **Per-point brightness drops correspondingly.** These are additive
  contributions into an HDR target, and 7x the points multiplies the
  overlap at any given pixel. Keeping single-point energy low is what lets
  the near-white hotspots stay a property of genuine density rather than
  becoming the whole entity.
- **`core_density_bias` changes meaning.** Its volumetric sense — how
  radially centre-weighted the fill is — has no analogue on a surface,
  since there is no interior to weight. It now sets how much of the
  population sits exactly on the skin. The name is kept because the knob
  still answers the same question a user would ask of it.
- **`Core`/`Body`/`Halo` invert.** `Core` was a minority at the centre of
  a volume; it is now the skin itself and therefore the majority. `Halo`
  sits outside the skin only, never inside, since points behind the
  surface are occluded by the thing they are meant to be atmosphere
  around.
- **Idle's specified character changes**, and the viscous lava/oil-drip
  dynamics relocate to Thinking and ToolUse — see `../PRESENCE_SCENES.md`
  §4.1 and §4.3. They were the wrong default, not a wrong idea.
- **Adding a state is now a shape, not an engine change.** This is the
  point of refactoring the Chladni plate onto the trait rather than
  leaving it as a bespoke behavior: two shapes driven by one behavior is
  the minimum that demonstrates the abstraction holds.
- **`SurfaceBehavior` has no per-particle wander noise**, unlike the
  volumetric behavior it replaces. A volume needed it because its points
  had nothing else to do; a surface is already breathing, turning, and
  reshaping its folds under them, and noise on top only blurs the
  silhouette while costing as much as the entire shape evaluation.

## Decision 2 — Presence colour is a user setting, defaulting to teal

`PresencePalette` is runtime data selected by a `PaletteId` — a small
closed enum (`teal`, `lime`, `ice`, `ember`) — rather than a set of
compile-time constants read inside the renderer. The prototype's debug
panel carries a live selector; `EdgeSettings.presence_palette` is where it
lands in Phase 2 (`../PRESENCE_INTEGRATION_PLAN.md` §4).

### Reason

- The presence is the assistant's visual character, and which hue that
  character wears is reasonably the operator's choice, not a decision the
  build makes for them.
- ADR-010 recorded the brand reconciliation (teal over the source
  concept's LiDAR lime) as a *deviation*. Making the palette selectable
  turns that into a default rather than an exclusion — the concept's
  original lime is a first-class option again, and it is what the visual
  concept was designed around.
- The alternative was to bake teal in and revisit when the setting was
  scheduled (Phase 4's "color variant" line). That work would have had to
  be undone to ship the setting at all, and the plumbing — threading a
  palette value to the renderer each frame instead of reading constants
  inside it — is the entire cost. Doing it while the code is small is
  strictly cheaper than doing it later.

### Consequences

- **A closed enum, not free-form hex.** This lets the value round-trip
  through settings and be validated against a fixed list, mirroring how
  `EdgeSettings.voice_style` is already handled. Unlike `voice_style` it
  is **not** a critical field: colour is cosmetic, so an unrecognised
  persisted value degrades to the default rather than failing startup.
- **Stop names are a settings contract.** `teal`/`lime`/`ice`/`ember` are
  persisted strings and must not be renamed casually.
- **The `accent` stop is derived, not authored.** It is the `body` hue
  driven to full chroma, because creases are the same material catching
  more light rather than a different material. Deriving it guarantees
  every preset stays in family, including presets added later — a
  hand-picked sixth hex per preset would be a standing opportunity to get
  one wrong.
- **`ink` is shared across every preset.** The near-black field is the
  window's background, not part of the entity's identity.
- **Every stop is stored as pure chroma** (linear RGB rescaled so its
  largest channel is 1.0). Points are emissive, so lightness comes from
  the simulation's energy term; storing a dark stop as-is would dim every
  point using it and make hue and brightness impossible to tune
  independently. This constraint predates the setting and is unchanged by
  it — it now simply applies to four presets instead of one.
- `../PRESENCE_INTEGRATION_PLAN.md`'s Phase 4 "color variant" item moves
  into Phase 1 (prototype-proven) and Phase 2 (persisted in settings).

## Alternatives considered

- **Tune the volumetric model harder.** Rejected: no parameter produces a
  silhouette or fold creases from a volume fill, because neither exists in
  that model. This is the reason both are decisions rather than tuning.
- **Signed-distance field with per-frame projection.** More general, and
  it would express shapes a parametric surface cannot. Rejected on cost —
  see the reasoning above — and revisitable if a future shape genuinely
  cannot be expressed parametrically.
- **GPU compute for the simulation.** Would raise the budget much further.
  Deliberately not reached for: the staggered refresh brought the CPU cost
  inside budget with a comfortable margin, and a compute path would move
  the simulation away from the plain, testable Rust that
  `../PRESENCE_INTEGRATION_PLAN.md` §5 relies on for its automated
  coverage. Revisit only when measurement demands it.
- **Keep the palette compile-time and ship the setting in Phase 4.** See
  Decision 2's reasoning: the plumbing is the whole cost, and deferring it
  means writing code that has to be undone.
