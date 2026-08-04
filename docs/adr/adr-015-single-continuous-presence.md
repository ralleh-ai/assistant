# ADR-015: One Continuous Presence — A Scene Is A Blended Parameter State Of A Single Persistent Entity, Not A Stack Of Entities

**Status:** Proposed (decision pending review; milestones in
[`../PRESENCE_SINGLE_VIEW_PLAN.md`](../PRESENCE_SINGLE_VIEW_PLAN.md)).

ADR-011 put points on parametric surfaces. ADR-012 made modes compose
additively as weighted terms on *one shell* rather than selecting exclusive
shapes. ADR-013 gave the presence its own always-on-top droplet window.
ADR-014 split cognition from rendering and added a force-field substrate for
free-space entities.

The scene system built on top of those drifted away from ADR-012's central
idea. It grew a stack of up to four concurrent entities that are *presented*
and *dismissed*, each with its own disposition (overlay/replace), corner
placement, TTL, and point budget. That makes a visual change a discrete
composition event — a second object appears, crossfades against the first, and
is destroyed — when the identity of this product is a *single thing that
transforms*. This ADR returns the scene system to ADR-012's grain and extends
it from modes to form.

## Decision

There is exactly one presence entity, and everything that used to be "a scene"
is now a timed move of that entity's parameters.

1. **One entity, for the lifetime of the process.** The director owns a single
   `EntityInstance`. Its particle set is created once and is never destroyed
   and rebuilt to change what is shown; points *migrate*. Only a quality-tier
   change resizes the population.

2. **A scene is a bounded target parameter state, not a factory.** Today a
   scene template is a function that builds an entity with its own generator
   and behavior. It becomes declarative data: a set of form weights, material
   bias, and motion rates. The registry stores targets, not constructors.

3. **All visual change is timed interpolation.** There is no crossfade between
   two objects because there is only one object. Every change is the parameter
   set easing from where it is to where it was asked to be, over a duration,
   with a curve. This is the "controlled and timed manner" the single-view
   model requires.

4. **Form is a bounded, weighted, open-ended vocabulary — never an exclusive
   selection.** Form is carried as a length-capped list of `(target, weight)`
   pairs with weights in `[0,1]`, not a single choice and not a frozen set of
   four scalars. The target vocabulary is a versioned enum that starts at
   sphere / ring / helix / nebula and grows toward face, galaxy, tree, and bird
   without reshaping the wire type. This is ADR-012's additive composition rule
   applied to shape: the interesting states are the mixtures, and an exclusive
   selector would make the product's central claim untestable exactly as it
   would have for modes. A cap on the list length is what keeps the contract
   bounded (T9/T14) while leaving the vocabulary open.

5. **One behavior blends both simulation substrates per particle.** ADR-014
   point 5 kept the surface spring and the force field as substrates owned by
   *different entities*. With one entity they must coexist inside it: a
   `MorphBehavior` evaluates the surface-spring target and the field
   acceleration and blends them by form weight, skipping either term when its
   weight is ~0. At full droplet weight the result must be identical to today's
   `SurfaceBehavior`, which is the preservation test that keeps ADR-011 intact.

6. **Loading is a parameter state, not a second entity.** The loading ring
   stops being its own object and becomes a form the one entity takes while
   work is in flight.

7. **The Brain steers form in real time, over the existing bounded contract.**
   Form weights ride on `PresenceState` as bounded scalars alongside the
   cognitive ones. `Command::PresentScene`/`DismissScene` are retired — there
   is no present or dismiss, only a continuously updated target.

8. **Composition, placement, and lifetime machinery are retired.**
   `Disposition` (overlay/replace), `Placement`/`Anchor` (corner positioning),
   per-scene `ttl`, `MAX_LIVE_SCENES`, and the global budget splitter all go.
   The single entity's position is the droplet window's position (ADR-013).

9. **Layers, not entities, carry visual variety.** Splitting the organism into
   layers with *independent simulation parameters* — core, aura, mist, energy,
   highlights, sparks, trails — is how one body stays visually rich without a
   compositor. Today `Layer` only carries a material (size, brightness); under
   this ADR a layer also carries its own share of the form weights, field
   strengths, and motion rates, so the core can hold a stable shape while the
   aura drifts and sparks scatter. This is the structural replacement for the
   entity stack, and it is what makes the stack unnecessary rather than merely
   forbidden.

## Reason

**It is what the product actually is.** A believable presence is one body that
changes, not a compositor that fades objects over each other. Overlay/replace
is a slideshow model; it produces the "two things briefly coexisting" read that
never looks like transformation no matter how well each object is tuned.

**ADR-012 already decided this, for modes.** That ADR rejected exclusive shape
selection in favor of weighted terms on one shell, precisely because the
overlaps carry the meaning. Form is the same argument one level up. Keeping a
scene stack while modes compose additively is an inconsistency, and it is the
one that produced the worst-looking output.

**It deletes a whole class of defects rather than fixing them.** The load
stalls came from regenerating point sets and re-dividing a global budget
whenever the stack changed. With one permanent population there is nothing to
regenerate, nothing to split, and no crossfade to schedule — those code paths
stop existing rather than getting faster.

**The substrate for it already exists.** `sim/field` (composite forces,
integrator) and `MorphTarget`/`SdfAttractor` (sphere/ring/helix, pulled with
strength modulated by `focus`/`confidence`) were built in ADR-014's M4/M5
exactly so a cloud could take a shape without per-particle choreography. The
missing pieces are the single-entity collapse, the blend behavior, and the
interpolator — not new simulation math.

**Morphing the field, not the particle, still holds.** ADR-014 point 6 forbade
assigning per-particle destinations, and this ADR does not reintroduce them:
form weights move attractors and SDF targets, and the particles follow.

## Reconciling with the presence vision

**This is the refactor that unblocks GPU simulation.** The vision's rule is
"never upload particle positions every frame," and today we do exactly that:
the renderer takes particle slices each frame, and the point sets are rebuilt
whenever the scene stack changes. A single population that is created once and
never rebuilt is the precondition for keeping positions and velocities in GPU
storage buffers permanently. ADR-014 deferred GPU compute (M8) on CPU-first
grounds and that still holds, but after this ADR lands the port stops being an
optimization and becomes the natural shape of the system — and the churn that
made it awkward is gone.

**Layers are how the vision's richness survives the collapse to one entity.**
Removing the stack could read as removing capability. It does not: the vision
asks for an organism split into independent layers, which is strictly more
expressive than several whole entities crossfading, because the layers belong
to one body and move together.

**The Behavior Graph must actually be used.** ADR-014 introduced a `Behavior`
trait and a `BehaviorStack`, but the director still composes the mode layer and
cognition directly rather than through the stack. The vision's behavior
vocabulary (thinking spiral, listening compression, confidence stabilization,
curiosity drift, memory ripple, celebration burst, sleep/wake) only becomes
drop-in once the stack is the real composition path. Making the director run
the stack is part of this migration, not a later cleanup.

**Emotion becomes physics here, not before.** Today's cognition modulations
adjust intensity, warmth, and expansion — material-ish, and shallower than the
vision demands. The specific mappings it calls for (curiosity as curl and
exploratory tendrils, confidence as a stable core with reduced turbulence,
uncertainty as fragmentation and wandering wisps, thinking as compression with
internal turbulence) are field-strength modulations. They are only expressible
once the one entity can be field-driven, which is what `MorphBehavior` gives
us.

## Consequences

- `SceneDirector` loses `live_scenes`, `loading_ring` as a separate entity,
  and the budget allocator. `default_active_builtins_match_the_director`
  changes meaning (there is one builtin) and
  `global_budget_sum_stays_within_tier_ceiling_with_scenes` is deleted with the
  machinery it guards.
- `presence-ipc` gains bounded form weights on `PresenceState` and retires
  `PresentScene`/`DismissScene`; `VERSION` bumps, `MIN_SUPPORTED_VERSION`
  keeps one release of back-compat.
- ADR-014 point 5 is **amended**: the two substrates now coexist *within* one
  entity, blended by weight, rather than being selected per entity. ADR-011 is
  **not** amended — the surface spring remains the model for the solid shell,
  and the preservation test enforces that at droplet weight.
- Mid-morph frames pay for both substrates on the blended particles. The
  zero-weight short-circuit keeps the resting droplet at today's cost, and the
  free-space budget cap already established keeps the field end affordable.
- The dev panel's present/dismiss/anchor/disposition controls are replaced by
  form-weight sliders and a transition-duration control.
- ADR-012's spring bandwidth (~0.7 Hz) becomes a **floor on transition
  duration** for geometry terms: form cannot be moved faster than the spring
  can carry without reading as a teleport. Material terms (brightness, color)
  stay instant.

## Alternatives considered

**Keep the stack but always crossfade to a single visible scene.** Rejected:
it is the current model with a policy on top. Two point sets still exist, the
budget is still split and regenerated, and the transition is still a dissolve
between two objects rather than a transformation of one.

**Make everything SDF-driven, including the shell.** Conceptually the cleanest
single model, and rejected on ADR-011's measured grounds: projecting every
particle onto an implicit surface costs roughly four field evaluations plus
Newton steps per particle per step, which is far outside the frame budget at
these point counts. The parametric shell stays; the blend is how the two meet.

**Assign per-particle destinations for each form.** Rejected by ADR-014 point 6
and by `PRESENCE_VISUAL_ENTITY.md`'s warning about fragile point choreography:
it does not survive a change in point count, quality tier, or an interrupted
transition.

**Leave the loading ring as a permitted second entity.** Rejected: one
exception reintroduces the compositor, the budget split, and the crossfade
scheduler for a single case, and loading is well expressed as a form the one
body takes.
