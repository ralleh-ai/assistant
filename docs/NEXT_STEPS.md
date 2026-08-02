# Next Steps — Prioritized Backlog

## Done recently

- Product setup UX (splash → settings gate → core shell + voice style);
  live mic / OS caps / station settings foundations underneath.

## High priority — Tauri desktop shell (Phase 1 continued)

1. ~~Scaffold `desktop-edge/`~~ **done**
2. ~~Wire health / echo IPC (`core_ping`)~~ **done**
3. ~~Embed voice core (mock pipeline via `voice_smoke`)~~ **done**
4. ~~Settings / onboarding UI~~ **done** — product Settings gate + voice style;
   core placeholder with settings gear.
5. ~~OS capabilities (clipboard first)~~ **done** — traits + mocks; screen/hotkey
   stubs; `clipboard_smoke` via policy + mock (optional `--features clipboard-os`).
6. ~~Optional live mic from the shell~~ **done** — `mic_smoke` (policy + clearance);
   **on by default** in `desktop-edge` (`mic` feature / `build.features`).
   Workspace audio-core stays mic-off for headless CI.

## Medium priority — Point cloud presence entity (design locked, Phase 1 in progress)

See [`PRESENCE_VISUAL_ENTITY.md`](./PRESENCE_VISUAL_ENTITY.md),
[`PRESENCE_SCENES.md`](./PRESENCE_SCENES.md),
[`PRESENCE_INTEGRATION_PLAN.md`](./PRESENCE_INTEGRATION_PLAN.md),
[ADR-010](./adr/adr-010-point-cloud-presence-entity.md),
[ADR-011](./adr/adr-011-surface-point-generation-and-palette-setting.md),
[ADR-012](./adr/adr-012-additive-mode-composition.md), and
[ADR-013](./adr/adr-013-presence-window-and-process-model.md) for the full plan.

7. ~~Phase 1 — standalone Rust prototype~~ **substantially complete
   (2026-08-02)** — `presence-prototype/` (`winit` + `wgpu` + `noise`);
   Idle (Presence Shell) + Loading (Resonance Plate) clearly
   distinguishable, activity modes `thinking`/`speaking`/`tool_use` as
   weighted terms on the same shell (ADR-012), material-only modes
   `listening`/`attention`/`error` on the same layer without new
   geometry, multi-entity hierarchy via `activity_scale`, reduced-motion
   preset, quality tiers with adaptive downshift, `SceneRegistry`
   productized. 79 unit tests, clippy clean, ~170–200 FPS at idle on 2
   cores. Remaining Phase 1 items are opportunistic rather than
   blocking Phase 2:
   - Very-long-run (30+ min) peripheral idle QA — currently informal.
   - Optional `High` quality tier (100k+) if a target machine warrants it.
   - GPU compute path for deformation — deferred per ADR-011's fallback
     ordering; not needed at current numbers.

8. **Phase 2 — window productization and desktop-edge integration**
   (design locked in [ADR-013](./adr/adr-013-presence-window-and-process-model.md);
   implementation is the next major body of work). Concrete tasks:

   1. **Refactor `presence-prototype/` toward a shippable crate.** Split
      the current binary into `presence-core` (renderer + simulation,
      pure library — no `winit`, no key handling, no debug overlay) and
      `presence-runtime` (the binary that opens a window and runs the
      loop). The dev debug panel becomes a `dev` feature on the runtime
      so a production build does not ship it. See open item #2 in
      `PRESENCE_INTEGRATION_PLAN.md` §7 — promote, don't rewrite.
   2. **Define the IPC surface.** A `presence-ipc` crate holding the
      wire type for `PresenceSignals` (already the same shape the dev
      panel drives) plus a settings-update message (palette, quality
      tier, reduced-motion, window bounds). Transport and encoding are
      still open per ADR-013's "Not decided here" — protobuf-over-
      length-prefixed-frames is the leading candidate; JSON is easy but
      wasteful for a 60 Hz signal stream. Enforce the T9/T14 constraint
      in the *type*: no field capable of holding raw audio, transcript,
      or prompt/completion content.
   3. **Frameless / transparent / always-on-top droplet (Windows first).**
      Per-pixel alpha, click-through by default, hover-hold or global
      hotkey to grab focus. macOS and Linux follow.
   4. **Position and layout persistence.** Presence-side store; the
      shell should not own window geometry. Multi-monitor placement is
      still open (ADR-013 §"Not decided here"); land single-monitor
      first, then add.
   5. **Launch and discovery.** How the shell finds / spawns the
      presence process. Options: shell-spawned child, user-launched
      alongside the shell, OS service. Not decided; prototype at least
      shell-spawned in this phase since it is the simplest and does
      not preclude the others.
   6. **`EdgeSettings.presence_*` fields** (D3 already asked for
      `presence_palette`; this phase adds `presence_quality_tier` and
      `presence_reduced_motion`). Validated like `voice_style`;
      unknown/missing values fall back to defaults, never blocking
      startup. On startup and change the shell IPC's the resolved
      values into the presence.
   7. **Still driven by synthetic signals** — this phase proves the
      window + IPC + settings mechanics, not real assistant state.

9. **Phase 3 — real signals** (blocked on #13 below plus a wiring
    prerequisite):
    1. Replace synthetic signals with the real VAD state machine
       (#13). `idle`/`listening` become real.
    2. Wire `ralleh-ai-router` and `ralleh-tool-gateway` (or a local
       stand-in) into the shell process so `thinking`/`tool_use` have
       a source. Neither is embedded in `desktop-edge` today.
    3. Route real audio level to `speaking`'s brightness and its
       phrase envelope to the pulse geometry (ADR-012 spring
       bandwidth constraint).
    4. Add sparse secondary events (scan sweeps, inbound streams).
    5. Map policy `Denied` / handler `Failed` outcomes to `error`.

10. **Phase 4 — hardening and options**:
    - 60 FPS budget confirmation on representative hardware (Phase 1's
      2-core measurement is a floor, not a target-machine test).
    - OS-level reduced-motion preference honored automatically
      alongside the runtime toggle.
    - Optional text status line for accessibility (present-tense
      "listening", "thinking", etc.).
    - Rapid-state-change stress test with the real signal path.
    - Extension-point documentation for new entity types
      (`PRESENCE_SCENES.md` §8 already covers scenes; add entities).

## Medium priority

11. **OIDC / device attestation** — when NestJS control plane exists (T1/T18).
12. Optional `allow_private_targets` for http-fetch internal APIs.
13. Live mic → VAD → STT path in the shell (beyond capture metrics).
14. Approval cryptographically bound to approver identity (T4).
15. Audit integrity / queryability beyond JSONL (T5).
16. Real screen capture / hotkey OS backends (still trait-only stubs).

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- MCP connectors — Phase 3.
- Native in-crate `piper-rs`.
- Mass-rename crates — deferred ([`CRATE_NAMING.md`](./CRATE_NAMING.md)).
