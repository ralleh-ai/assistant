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

7. ~~Phase 1 — standalone Rust prototype~~ **in progress** —
   `presence-prototype/` (`winit` + `wgpu` + `noise`, not Three.js — see
   ADR-010 revision); Idle (Presence Shell) + Loading (Resonance Plate)
   scenes per `PRESENCE_SCENES.md`; tune until the two are clearly
   distinguishable and Idle reads as calm. Points lie on parametric
   surfaces and the palette is a user setting (ADR-011). `thinking`,
   `speaking`, and `tool_use` moved into this phase as weighted terms on
   the same shell (ADR-012); `listening`, `error`, and `attention` need no
   geometry and sit on the same mode layer later.
8. Phase 2 — implement the ADR-013 window and process model: a separate
   presence process, a frameless transparent always-on-top droplet with
   click-through by default, a small local IPC channel from `desktop-edge`
   for signals, and shell-side persistence of settings (palette, quality
   tier, reduced-motion) IPC'd in on startup. First platform is Windows,
   because per-pixel alpha + click-through is where it is fussiest. Still
   open: the IPC transport/encoding, the launch/discovery model, and
   multi-monitor placement — see the "Not decided here" section of ADR-013.
9. Phase 3 — real signals: blocked on #13 below (VAD state machine) plus
   a new prerequisite this plan surfaced — `desktop-edge` does not yet
   embed `ralleh-ai-router`/`ralleh-tool-gateway` at all, needed for
   `thinking`/`tool_use` signal.
10. Phase 4 — perf budget pass before raising default point count; user
    settings (density/intensity/reduced-motion/color variant); edge-case hardening.

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
