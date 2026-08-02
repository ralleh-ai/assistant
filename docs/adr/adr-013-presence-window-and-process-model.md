# ADR-013: The Presence Runs In Its Own Process, As A Frameless Always-On-Top Droplet

**Status:** Accepted. Phase 2 will implement the window and IPC scaffolding;
the Phase 1 prototype is unchanged.

`PRESENCE_IMPROVEMENT_GUIDANCE.md` §P0.2 flagged the window and process
model as decisions to make *before* heavy integration work — the shape of
the presence's relationship to the rest of the shell determines what the
signal path looks like and what a "restart" means, both of which are much
cheaper to pick up front than to unwind after either side has grown code
against the other. This ADR records the four choices and the reasoning.

## Decisions

### 1. Process model — separate process

The presence renderer lives in its own OS process, communicating with
`desktop-edge` over a small local IPC channel (unix domain socket on
macOS/Linux, named pipe on Windows). Signals travel from the shell to the
presence; the presence never talks back except for lifecycle heartbeats.

### 2. Presentation — independent always-on-top droplet

The presence draws to its own window, always on top of the desktop, at a
position the user picks once and the process remembers. It is not
composited into desktop-edge's layout and is not a child of any shell
window.

### 3. Chrome — frameless, transparent, click-through by default

The window has no title bar or borders, a transparent alpha-blended
background, and passes clicks through to whatever is beneath it in the
default state. A hover-hold or a global hotkey brings controls back and
switches it out of click-through for as long as the user is interacting
with it.

### 4. Settings — authoritative in desktop-edge, IPC'd to the presence

Palette, quality tier, reduced-motion, density, and any other persisted
preference lives in the shell's settings store (already the source of
truth for the rest of the product). The presence process reads its
initial values on startup over IPC and applies runtime changes as they
arrive.

## Rationale

**Why a separate process rather than a thread inside the Tauri app.**
The presence is a real-time wgpu renderer with a fixed simulation
timestep and an accessibility mode of its own, and any stall in the
webview event loop would show up as a visible hitch — the shell would
appear to *breathe irregularly*, which is exactly the failure a live
presence must not have. A separate process keeps the two clocks
independent, lets the presence outlive shell restarts (so a user who
reloads their assistant does not see the presence blink), and lets each
process crash without taking the other down. The IPC overhead is
negligible against a 60 Hz signal stream, and IPC boundaries force
signal serialisation, which is a stronger contract than an in-process
API where the presence would end up reading shell state directly and
coupling to its internals.

**Why a droplet rather than a sibling or embedded window.** A sibling
window inherits the shell's lifecycle — closing the shell closes the
presence, which contradicts the "always-on instrument" framing in
`PRESENCE_VISUAL_ENTITY.md` §2.1. An embedded panel makes the presence
one of the shell's views rather than a distinct thing, and the whole
argument for its existence is that it is *not* another widget. A
droplet the user positions once and the OS remembers is the shape that
matches the intent.

**Why frameless + transparent + click-through.** Frameless and
transparent are what let the presence read as an entity in the space
rather than a window sitting on top of the desktop. Click-through by
default is the important one: an always-on-top window that steals
clicks is hostile, and the presence has no click-first interactions
anyway. Hover-hold and a hotkey are enough for the rare cases where
the user needs to grab it.

**Why settings in desktop-edge.** The shell already has a settings
surface, persistence, and a place for the user to change themes and
preferences; adding a second one in the presence would fragment the
UX. The runtime debug panel stays for development, but it is not
where a shipped user changes their palette.

## Consequences

- Phase 2 will introduce an IPC schema and versioning discipline. The
  natural first candidate is protobuf-over-length-prefixed-frames or a
  small hand-rolled binary format; JSON is easy but wasteful for a
  60 Hz signal stream. Not decided here.
- Windows-specific work will lead: click-through and per-pixel alpha
  are the platform's fussiest surface. macOS and Linux support both
  more cleanly.
- The prototype's dev panel becomes a development-only fallback,
  reachable only when the shell is not running or when a hidden env
  var is set. The panel is not what a user sees in production.
- Positioning persistence and multi-monitor placement is now a
  presence-process concern, since the shell no longer owns the
  window. A small "layout store" at the presence side is sufficient;
  it does not need to be aware of the shell.
- The signal layer described in
  `PRESENCE_INTEGRATION_PLAN.md` Phase 3 is the thing that crosses
  the IPC boundary. Its schema is where "listening", "attention",
  and "error" become wire types rather than debug hotkeys.

## Not decided here

- The specific IPC transport and encoding.
- How the presence is discovered/launched (spawned by the shell,
  installed as a service, or user-launched).
- Whether the droplet has a "docked to shell" secondary mode.
- Reduced-motion as an OS-level preference vs a shell-level toggle;
  guide §5.5 argues for both, and both are cheap to support.
