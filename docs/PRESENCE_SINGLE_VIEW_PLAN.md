# Single Continuous Presence — Migration Plan

Implements [ADR-015](./adr/adr-015-single-continuous-presence.md): collapse the
present/dismiss scene stack into one persistent entity whose parameters the
Brain manipulates in real time.

This is a refactor of machinery we already have, not new simulation math. The
force-field substrate, the SDF morph targets, the cognition modulations, and
the bounded `PresenceState` channel all stay; what changes is that there is one
body instead of a stack, and every visual change becomes a timed parameter
move.

## Scope freeze

For the duration of this plan the security and integration surface is **frozen**:
`ralleh-policy-core`, `ralleh-audit-store`, `ralleh-tool-gateway`,
`ralleh-mcp-server`, and the egress allowlist are maintained (kept building,
kept green) but not extended. They are well built and none of them move the
believability goal. Effort goes to the presence engine until the single-view
model ships.

## Principles

- **Every milestone ships.** The app is runnable and the dev panel works at the
  end of each one, as with the ADR-014 roadmap.
- **The droplet must not change.** At full droplet weight with everything else
  at zero, the shell is numerically identical to today. This is enforced by a
  preservation test, the same way `ModeBehavior` was proven identical to
  `ModeLayer` in M3.
- **Nothing is regenerated to change what is shown.** If a code path rebuilds a
  particle set on a form change, the design is wrong.
- **Geometry respects the spring bandwidth** (ADR-012, ~0.7 Hz); material terms
  stay instant.
- Per milestone: `cargo fmt`, `clippy --workspace --all-targets -- -D warnings`,
  and tests, via `scripts\presence-dev.cmd` (prototype) and
  `scripts\cargo-dev.cmd` (root).

## Milestones

**S0 — ADR + plan.** ADR-015 and this document. No code change.

**S1 — Form weights and the interpolator.** Add `FormWeights` — a length-capped
list of `(target, weight)` pairs over a versioned target vocabulary (sphere /
ring / helix / nebula to start, extensible toward face, galaxy, tree, bird) —
and a parameter interpolator that eases a current parameter set toward a target
over a duration with a curve and a per-field rate limit. Pure and headless;
unit-tested standalone. Nothing is wired in yet, so there is no behavior change.

*Tests:* the list is capped and weights stay bounded; adding a target to the
vocabulary does not change existing behavior; a transition completes within its
window; an interrupted transition continues from the current value rather than
snapping (the `reversing_mid_transition` property `ModeLayer` already has);
geometry terms cannot be driven faster than the spring bandwidth floor.

**S2 — `MorphBehavior`.** One `PointBehavior` that owns both substrates and
blends them per particle by form weight: the surface-spring target from
`sim/shapes`, the field acceleration from `sim/field`, each short-circuited
when its weight is ~0.

*Tests:* **preservation** — at droplet weight the output is identical to
`SurfaceBehavior` for the same inputs; determinism; boundedness across the full
weight range; continuity — sweeping the weight produces no position
discontinuity; the zero-weight short-circuit actually skips the unused term.

**S3 — Single-entity director, composing through the Behavior Graph.**
`SceneDirector` keeps one `EntityInstance` driven by `MorphBehavior`. Loading
becomes a form/parameter state rather than a second entity. Retire
`live_scenes`, `Disposition`, `Placement`/`Anchor`, per-scene `ttl`,
`MAX_LIVE_SCENES`, and the budget splitter.

This is also where the director stops calling `modes.apply()` and
`cognition.apply()` directly and starts running the `BehaviorStack` ADR-014
introduced but never wired up. Without that, every behavior the vision names is
a special case instead of a drop-in. The dev panel keeps working throughout.

*Tests:* the stack produces output identical to today's direct composition
(same preservation discipline as S2); rewrite
`default_active_builtins_match_the_director` for one builtin; delete
`global_budget_sum_stays_within_tier_ceiling_with_scenes` with the machinery it
guards; the mode/cognition/audio/cursor tests must all survive unchanged in
meaning; loading still subdues and restores the shell.

**S4 — Scenes as declarative targets.** The registry stores target bundles
(form weights, material bias, motion rates) instead of entity constructors.
`present_scene`/`dismiss_scene` are replaced by "set target, with duration."

*Tests:* a target round-trips through the registry; an unknown target is
rejected without disturbing the current state; targets are clamped on entry.

**S5 — Contract.** Form rides on `PresenceState` as the length-capped
`(target, weight)` list from S1; `VERSION` bumps; `MIN_SUPPORTED_VERSION` keeps
one release of back-compat; `Command::PresentScene`/`DismissScene` are retired.
The `presence-core` adapter maps incoming form onto director targets.

*Tests:* round-trip, field-omission defaults, over-long lists rejected, and
clamping (NaN/out-of-range) — matching the `PresenceState` and
`active_modes_dedupes_and_rejects_overlong_lists` suites already in place.

**S6 — Brain drives form.** `presence-brain` derives form intent from
cognition and lifecycle (thinking, tool use, speaking, loading) and emits it
continuously. The dev panel gets form-weight sliders and a transition-duration
control so the morph can be driven by hand exactly as the Brain drives it.

*Tests:* headless Brain tests for the lifecycle → form mapping, in the style of
the existing `presence-brain` suite.

**S7 — Per-layer simulation parameters.** Give each layer its own share of the
form weights, field strengths, and motion rates, so the core can hold a shape
while the aura drifts and sparks scatter. `Layer` currently carries only a
material (size, brightness); this is what makes one body as expressive as the
entity stack it replaced, and it is where the vision's core / aura / mist /
energy / highlights / sparks / trails split actually lands.

*Tests:* a layer's parameters do not leak into its neighbours; the resting
droplet is unchanged when every layer holds default parameters.

**S8 — Cleanup.** Delete the dead scene machinery, refresh the docs and ADR
statuses (ADR-014 point 5 amended, ADR-015 accepted), and recapture the
manual-verify screenshots.

## After this plan

With one permanent population that is never rebuilt, the ADR-014 M8 GPU compute
port stops being an optimization and becomes the natural next step: positions
and velocities can live in storage buffers the renderer reads directly, which
is the vision's "never upload particle positions every frame." That is a
separate decision to take once this plan lands, not part of it.

Two smaller gaps against the vision are also worth picking up separately:
`PresenceState` has no `excitement` scalar (it has confidence, curiosity,
uncertainty, focus, task complexity, memory activity, and emotional tone), and
the cognition modulations are still material-level rather than the field-level
physics the vision describes — the latter becomes expressible after S2.

## Risks

- **Mid-morph cost.** Blended particles pay for both substrates. Mitigated by
  the zero-weight short-circuit (the resting droplet costs what it costs today)
  and the free-space budget cap already in place.
- **The droplet regressing.** This is the real risk of the refactor, and the
  preservation test in S2 is the guard. If it cannot be made to pass, the blend
  design is wrong and should be reworked before S3 lands.
- **Transitions that read as teleports.** Handled by the spring-bandwidth floor
  on transition duration in S1, not by tuning each target by hand.

## Status log

- S0: this document and ADR-015 written; awaiting review.
