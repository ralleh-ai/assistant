# Status — Last Validated Snapshot

**As of:** 2026-08-02 (Phase 2 §1–§6 complete; Phase 3 §1 (VAD →
Listening), §3.3 (`speaking` on TTS synthesis), and §3.5 (policy
denials → `error`) landed — the presence now reflects real assistant
outcomes rather than dev-panel toggles)

## Build/test state

```
cargo test --workspace → headless-safe (audio-core mic feature off)
desktop-edge: splash/settings/core UI; mic on by default; settings gate
presence-prototype (excluded from workspace): builds, 79 unit tests pass,
  runs and renders (winit + wgpu) at ~170–200 FPS at idle on 2 cores.
  See its own cargo test/run — not part of `cargo test --workspace`.
```

**Local toolchain note (this dev machine only):** this machine has a
second, incomplete Visual Studio install (`...\Visual Studio\18\...`)
that `rustc`'s default MSVC auto-detection sometimes picks over the
working `Visual Studio\2022\BuildTools` install, causing spurious
`LINK1104`/`C1083` (missing `msvcrt.lib`/`vcruntime.h`) errors unrelated
to any code change. If you hit this, use `scripts\tauri-dev.cmd` /
`scripts\presence-dev.cmd`, which load the VS2022 BuildTools environment
explicitly before invoking `npm`/`cargo`.

## Highlights

- **Product shell** — startup splash; Settings when critical fields missing;
  calm core placeholder with gear → Settings.
- **Voice style** — `calm` | `direct` | `warm` in `edge-settings.json`.
- Smoke IPC kept for developers; not shown on core home.
- **Point cloud presence — Phase 1 prototype** (`presence-prototype/`):
  standalone `winit`+`wgpu` binary, one presence shell always on +
  Loading (resonance plate) toggle, `egui` debug overlay. Not wired into
  `desktop-edge` yet — see `docs/PRESENCE_INTEGRATION_PLAN.md`.
  Points lie *on* parametric surfaces rather than filling volumes, giving
  the silhouette and fold creases a volume fill structurally cannot have
  (ADR-011); presence colour is a user setting
  (`teal`/`lime`/`ice`/`ember`) rather than a compile-time constant.
  `thinking`, `speaking`, and `tool_use` are weighted terms added to that
  one shell rather than separate shapes, so they compose and their
  transitions are weight lerps (ADR-012).
  Render path is HDR (`Rgba16Float`) → bloom → ACES tonemap + vignette, with
  per-particle core/body/halo layering and density-driven highlight
  desaturation. ~150-200 FPS at idle and ~109-171 in every activity mode, at
  2560×1600 with 80,000 points (plus 40,000 more while Loading shows) in a
  release build on 2 cores, so no GPU compute path is needed yet.
- **Presence — improvement-guidance pass (2026-08-02):**
  - Idle-calm: fold evolution and breath cadence halved; slow (~35s)
    crease-brightness rest modulator so folds appear to rest without the
    silhouette moving.
  - Three material-only modes on the shell: `listening` (N),
    `attention` (A), `error` (E). Error is subtractive — a denial
    visibly wilts an in-progress activity rather than adding another
    colour on top. `material_modes_never_reach_the_shell_drive` locks
    the invariant that these three never contribute to `ShellDrive`.
  - Multi-entity hierarchy: `SceneDirector.activity_scale` eases
    1.0 → 0.55 while Loading composites, so a running mode still shows
    through the plate but does not fight it for the eye.
  - Shared `TRANSITION_WINDOW_SECONDS` (300–900 ms) with a runtime
    debug assertion and a paired test that the entity-fade duration
    sits inside it.
  - Reduced-motion (R): shell animation clock to 0.12×, mode
    contributions to 0.4×. Springs still integrate at real dt.
  - Quality tiers (Q cycles): `Balanced` (80k/40k, stride 4) and
    `Low` (30k/15k, stride 8), with runtime `deform_stride` on
    `SurfaceBehavior`; particles regenerate on switch. App
    auto-downshifts once after 3s under 45 FPS; no auto-upshift.
  - `SceneRegistry` descriptors carry `entity_kind`, `priority`,
    `default_active`; a sync test fails if the registry and the
    director drift out of step.
  - **ADR-013** — window and process model **locked** (decision only;
    Phase 2 will implement): separate presence process, frameless
    transparent always-on-top droplet, click-through by default,
    shell-authoritative settings IPC'd in.

## Next up

- **Presence Phase 2 (in progress)** — crate split (`presence-core` /
  `presence-runtime`), `presence-ipc` wire crate, stdio transport,
  droplet chrome flag, shell-side `Presence` spawner, Tauri command
  surface (`presence_set_*`), React dev panel, and live mic pump
  (`MicPump` → `Command::SetSignalsScalars`) all landed. Remaining:
  per-pixel-alpha + click-through droplet on Windows, position/layout
  persistence, `EdgeSettings.presence_*` fields, and Phase 3's real
  `intensity` / `progress` signals from VAD / router. See
  `NEXT_STEPS.md` §8 and `PRESENCE_INTEGRATION_PLAN.md` §4 Phase 2.
- Medium-priority backlog (OIDC when control plane exists, live mic →
  VAD → STT beyond capture metrics, real screen/hotkey OS backends,
  etc.) — see NEXT_STEPS.md.
