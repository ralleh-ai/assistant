# ADR-012: Modes Compose Additively On One Shell, Rather Than Selecting Exclusive Shapes

**Status:** Accepted (Phase 1 prototype implements `thinking`, `speaking`,
and `tool_use`).

ADR-011 established that presence points lie on a parametric surface. It left
open how the remaining states from `../PRESENCE_VISUAL_ENTITY.md` §5.1 would
be expressed, and `../PRESENCE_SCENES.md` §5.2 had begun answering it with a
"shape vocabulary" — one `SurfaceShape` per state. This ADR reverses that.

## Decision

There is one shape for the `AssistantCloud`, `PresenceShell`, and its radius
is a weighted sum of independent terms:

```
r(seed, t) = 1 + Σ wᵢ · termᵢ(seed, t)
```

- A `PresenceMode` (`Thinking`, `Speaking`, `ToolUse`) does not select a
  shape. It raises a weight in `ShellDrive` — `fold`, `lobes`, `pulse`,
  `neck` — which the director lerps.
- Modes are a **set**, not a slot. Nothing represents "the current mode".
- Any term whose weight falls below `ShellDrive::GATE` is skipped outright
  rather than evaluated and scaled to nothing.
- `fold` is the resting identity. It yields depth as the other terms rise,
  but never reaches zero in any mode.
- Idle is exactly `fold = 1` with everything else at zero, which is the
  regression guard: the resting shell is not a state the model has to
  reproduce, it is what the model degenerates to.

### Reason

**Concurrency is the normal case, not the exception.** An assistant narrates
a tool call while it is running it, and keeps thinking while it speaks. With
one exclusive shape per state, a director would spend most of its life
mid-transition between shapes that are all genuinely true at once, and every
pair of states that can overlap would need a blend written for it by hand —
quadratic work for a linear number of states. Raising two weights needs no
code of its own.

**Transitions stop being cross-fades.** Switching between two shapes means
one population fading out while another fades in, and there is a window in
which the entity is two half-drawn things. Weights leave the particle set
untouched: a state change moves numbers, and the same points follow the same
spring to a slightly different skin. This also means a transition can be
interrupted and reversed from wherever it actually is.

**Gating makes the cost proportional to what is live, not to what exists.**
The measured cost of two modes at once is close to the cost of one, because
the per-particle work counts live terms. Without a gate, every term added to
the shell would be a permanent tax on idle, and the model would get more
expensive with every state — which is exactly the pressure that produces one
shape per state instead.

**It follows §3.1's coherence rule.** The presence should read as one living
thing changing what it is doing, not as a set of different objects sharing a
name. Terms adding to a shell that is still recognisably itself is that rule
expressed as arithmetic.

### Consequences

- `../PRESENCE_SCENES.md` §5.2's shape vocabulary collapses from six shapes
  to one shell plus the plate. Adding a state is now adding a *term*, and the
  cost of the addition is paid only while that state is engaged.
- `FoldedShell` is renamed `PresenceShell` and its fold parameters move into
  a `FoldTerm`, because it is no longer the idle shape — it is the shell,
  with idle as one setting of it.
- The shell gains a hard radius band (`RADIUS_MIN`/`RADIUS_MAX`). Additive
  composition introduces one failure no single term can cause: several terms
  displacing the same spot outward at once. The band is a safety net for
  genuine coincidences, not the mechanism that keeps the shell in frame —
  that is the fold yielding depth as other terms rise.
- The shell's scale drops from 1.45 to 1.32. The margin has to be sized for
  the loudest state rather than for idle; a shell scaled to fill the frame at
  rest has nowhere to put a lobe or a pendant.
- `listening`, `error`, and `attention` need no geometry and will sit on the
  weight/lerp machinery without adding terms.

## Decision 2 — Speech drives brightness, not geometry, at syllable rate

The `speaking` state is split across two channels:

- **Geometry** follows a smoothed *phrase* envelope, capped near 1 Hz.
- **Brightness** follows the raw `signals.audio_level`, assigned directly in
  `SurfaceBehavior::update` and never sprung.

### Reason

`SurfaceBehavior` springs particles toward the skin at `spring_k = 14`, which
puts its natural frequency near 0.7 Hz. A second-order system passes roughly
two percent of a signal ten times its corner, so a shell driven at speech's
4–7 Hz syllable rate would sit visibly still while the debug panel insisted
it was speaking. This is a property of the spring, not a tuning failure of
the speaking term, and it is invisible in a screenshot — which is why it is
recorded here rather than left as a comment.

Raising `spring_k` to chase syllables would take roughly seventy times the
stiffness. The spring is shared by every mode and both shapes, so that trades
`../PRESENCE_VISUAL_ENTITY.md` §2.3's softness everywhere for one state's
responsiveness.

The general rule this generalizes to: **any signal-driven motion must sit
inside the spring's bandwidth, or be routed to a channel that isn't sprung.**
Brightness, colour, and point size are all assigned rather than integrated
and are therefore instant; position is not.

## Decision 3 — Tool-use pendants extend and retract rather than detaching

`../PRESENCE_SCENES.md` §4.3 described tool use as shedding a pendant that
detaches and travels away. Instead, one pendant per call extends from the
shell, holds, and retracts on completion.

### Reason

**Detachment is structurally inexpressible on a `SurfaceShape`.** The surface
is star-shaped about its centre — one radius per direction — so a detached
droplet is two surfaces on the same ray. This is not an awkward case to
handle; there is nowhere to put the second surface.

**Retraction is the better story anyway.** A call is a request *and* a
response. Reaching out and pulling back shows both, where shedding shows only
the outbound half and leaves completion invisible — and an indicator that
cannot show completion is showing activity rather than status, which is what
§2.6 rules out.

Detachment stays available later as a separate `DataStream` entity, which is
the right home for something that leaves the presence.

## Alternatives considered

**One `SurfaceShape` per state, as §5.2 originally proposed.** Rejected on
concurrency: it makes every overlapping pair a hand-written blend, and there
are enough overlapping pairs among `thinking`/`speaking`/`tool_use` alone to
make that the dominant cost of adding the fourth state.

**Curl noise for `thinking`, as `../PRESENCE_VISUAL_ENTITY.md` §6's usage
table specified.** Rejected as a consequence of ADR-011 rather than on its
own merits: curl displaces points *through* a volume, and after the surface
switch there is no volume for them to move through. Applied to a skin it
moves points off it, and the only visible result is that the shell goes
fuzzy. The intent — internal churn made visible — is preserved as bulges,
which is what internal churn looks like on a surface.

**A single active mode with a priority order.** Rejected because it discards
true information: an assistant that is both thinking and calling a tool would
have to show one and hide the other, and which one is hidden is arbitrary.

**Deferring the mode layer until real assistant signals exist.** Rejected
because the shell's shape is the thing being validated, and it cannot be
validated in a state nobody can reach. The debug toggles are a stand-in for
the signal, not for the geometry.
