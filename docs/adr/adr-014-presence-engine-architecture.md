# ADR-014: The Presence Is An Engine — A Shell-Side Brain Emits Bounded State, A Generic Behavior/Simulation/Render Pipeline Consumes It

**Status:** Accepted (decision + incremental implementation; see
[`../PRESENCE_ENGINE_ROADMAP.md`](../PRESENCE_ENGINE_ROADMAP.md) for milestones).

ADR-010 chose a Rust `winit`+`wgpu` renderer. ADR-011 put points on
parametric surfaces. ADR-012 made modes compose additively as weighted terms
on one shell. ADR-013 put the presence in its own OS process, driven from the
`desktop-edge` shell over IPC. This ADR names the architecture those four
imply and commits to evolving toward it: the presence is the *physical body*
of the assistant, driven by abstract cognition, never by the renderer and
never by raw model output.

## Decision

The presence is one directed pipeline split across the two processes ADR-013
established:

```
LLM / audio / cursor  ──►  Presence Brain  ──►  PresenceState  ──►(IPC)──►  Behavior Graph
                            (desktop-edge)      (bounded, immutable)         (presence process)
                                                                                   │
                                                                                   ▼
                              Render ◄── Particle Simulation ◄── Force Fields ◄── SimulationParameters
```

1. **Cognition never touches particles; rendering never sees cognition.** The
   two ends communicate only through immutable, bounded `PresenceState`
   (Brain → engine) and `SimulationParameters` (Behavior Graph → simulation).
   This is already true structurally — `render` imports only `sim::Particle`
   and `palette`, and `ModeLayer` already resolves abstract state into
   parameters — so this ADR ratifies and extends an existing boundary rather
   than inventing one.

2. **The Presence Brain lives shell-side, in `desktop-edge`.** It is the only
   component that knows the AI exists. It maps real lifecycle events
   (completion routing, streaming, tool dispatch, VAD, TTS amplitude, cursor)
   into `PresenceState`. It is **event-driven**: it updates on state change,
   not per frame, plus a small number of low-rate scalar pumps
   (`audio_level`, cursor) that are already the pattern today.

3. **`PresenceState` is the contract, and the privacy boundary is a type
   invariant.** Every field is a bounded scalar (`[0,1]` or a small enum) or a
   direction vector — never raw audio, transcript, or prompt/completion text
   (T9/T14). This is the same rule as ADR-013 D4, widened from six modes and
   three scalars to the richer cognitive vocabulary the engine now needs
   (confidence, curiosity, uncertainty, attention, focus, task complexity,
   memory activity, emotional tone). It is versioned like every other IPC
   message (`presence-ipc` `VERSION`/`MIN_SUPPORTED_VERSION`).

4. **Behaviors are stackable and blend; they emit contributions, not
   animations.** The Behavior Graph generalizes `ModeLayer`: each behavior
   reads `PresenceState` and contributes weighted terms to
   `SimulationParameters`. There are no `playThinking()`-style clips. This
   preserves ADR-012 exactly — additive composition, gating, eased ramps —
   and makes "add a state" mean "add a behavior."

5. **Two simulation substrates coexist.** The surface spring model (ADR-011)
   remains the *only* model for the solid scanned shell. A new **force-field
   substrate** is added for **free-space entities only** (nebula, orbit,
   morphing forms, data streams), where particles integrate velocity from
   sampled fields. Fields are the right model for flow through open space and
   the wrong model for a skin — so each entity picks the substrate that fits.

6. **Morphing morphs the field, not the particle.** Free-space forms
   (sphere/ring/helix/face/…) are reached by moving attractors and
   signed-distance targets, so no per-particle destination is ever assigned.
   This realizes `PRESENCE_VISUAL_ENTITY.md`'s "temporary coherent forms that
   form and dissolve" without the fragile per-point choreography that section
   warns against.

7. **Simulation stays on the CPU for now; GPU compute is deferred, not
   abandoned.** ADR-011 measured 150–200 FPS at 80k points and ordered GPU
   compute last, behind noise-octave/stride/rayon fallbacks. That ordering
   stands. The force-field and morph interfaces are made **data-driven** (field
   descriptors, not Rust closures) specifically so the eventual move of the
   integrator into a WGSL compute pass is mechanical rather than a rewrite.

## Reason

**The separation is already the codebase's grain.** The renderer is generic,
cognition is a resolver, and the wire type is bounded. Fighting that grain
with a rewrite would throw away the exact thing that makes the vision
reachable. Every point of this ADR is "make the implicit boundary explicit and
push more capability through it," which is why it can be done in shippable
milestones.

**Believability is physics, not color.** The vision's core demand — that
emotion changes *how the thing moves*, not merely its hue — is already how the
shell works (a mode raises a geometry term). Extending that to confidence
(stable core, less turbulence), curiosity (drift, lean), and uncertainty
(jitter, fragmentation) is more terms on the same machinery, provided they
respect ADR-012's spring-bandwidth rule.

**Free-space fields resolve the ADR-011 curl tension instead of reopening
it.** ADR-011 and ADR-012 both rejected curl noise because it pushes points
*off a skin*. That objection is specific to surfaces. In free space, curl's
divergence-free vortices are exactly what a living nebula is made of, and
`sim/noise.rs::curl` — implemented and currently dead code — is finally used
for the entity type it was retained for (`DataStream`/free-space), not
retrofitted onto the shell.

**The Brain belongs shell-side because that is where the AI is.** The LLM,
router, tool gateway, VAD, and TTS all live in `desktop-edge`. Putting the
Brain there keeps the IPC payload a *summary* (satisfying T9/T14 by
construction) and keeps the presence process a pure, testable engine that a
dev panel can drive identically to the real Brain.

## Consequences

- `crates/presence-ipc` gains `PresenceState` and a `SetPresenceState`
  command; `VERSION` bumps and `MIN_SUPPORTED_VERSION` keeps old commands
  valid. The existing `Signals`/`SetMode` path stays as a compatibility layer.
- `ModeLayer` becomes an implementation detail behind a `behavior` module, or
  is ported into it. The invariant tests (`material_modes_never_reach_the_shell_drive`,
  `default_active_builtins_match_the_director`) must survive the port
  unchanged in meaning.
- `desktop-edge` grows a `presence_brain` module that subsumes the scattered
  `hold_mode`/`pulse_*` calls into one owner of `PresenceState`.
- A new free-space entity family appears alongside the shell and plate, using
  `sim/field` rather than `sim/shapes`. The `SceneSpec`/realizer path is the
  registration mechanism.
- Points can eventually be simulated on the GPU without changing the Brain,
  the Behavior Graph, `PresenceState`, or the render boundary — that is the
  test of whether this boundary was drawn correctly.

## Alternatives considered

**Put the Brain in the presence process.** Rejected: it would push AI
lifecycle semantics (and the temptation to send richer, less-bounded data)
across the IPC boundary, weakening the T9/T14 type invariant and coupling the
engine to the assistant. The engine should be drivable by a dev panel and by
the real Brain with no difference.

**Replace the surface spring with a global force-field model.** Rejected on
ADR-011 grounds: forces move points off a skin, and the solid scanned read is
the whole identity of the shell. Fields are added *alongside* the spring for
entities that live in open space, not as a replacement.

**Migrate to GPU compute first.** Rejected per the user's CPU-first choice and
ADR-011's fallback ordering: the current numbers do not require it, and doing
it first would front-load the largest, riskiest change before the architecture
it should serve exists. It is the last milestone, and the interfaces are shaped
to make it mechanical.

**Rewrite into the six crates the vision sketches
(`presence-brain/behavior/simulation/render/audio/ai`) up front.** Rejected as
a big-bang migration that would break "every milestone ships." Crates are
extracted only when a boundary has earned one; the work starts module-first.
