# Status — Last Validated Snapshot

**As of:** 2026-08-01 (product setup UX: splash → settings gate → core)

## Build/test state

```
cargo test --workspace → headless-safe (audio-core mic feature off)
desktop-edge: splash/settings/core UI; mic on by default; settings gate
presence-prototype (excluded from workspace): builds, 68 unit tests pass,
  runs and renders (winit + wgpu). See its own cargo test/run — not part
  of `cargo test --workspace`.
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
  desaturation. ~150-178 FPS at idle and ~109-171 in every activity mode, at
  2560×1600 with 80,000 points (plus 40,000 more while Loading shows) in a
  release build on 2 cores, so no GPU compute path is needed yet.

## Next up

Medium-priority backlog (OIDC when control plane exists, conversation UI,
etc.) — see NEXT_STEPS.md.
