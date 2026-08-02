# ADR-010: Point Cloud Presence Entity — Rust-First Renderer (`winit` + `wgpu`), Not Three.js

**Status:** Accepted (planning; Phase 1 prototype in progress). **Revised**
— the original version of this ADR chose an in-Tauri-webview Three.js
renderer. That decision is reversed below after the source design concept
was itself corrected to a Rust-first production target.

## Decision

The assistant's always-on visual presence is a **deformable point cloud**
(and, longer-term, a small set of composable point-based entities — see
[`../PRESENCE_VISUAL_ENTITY.md`](../PRESENCE_VISUAL_ENTITY.md) §4)
rendered with a **pure Rust stack**: `winit` for windowing and `wgpu` for
GPU rendering, with the `noise` crate (Simplex + curl noise) driving the
particle simulation. This replaces the earlier decision to render via
Three.js inside the existing `desktop-edge` Tauri webview.

Full design (visual language, entity system, state system, tunables): see
[`../PRESENCE_VISUAL_ENTITY.md`](../PRESENCE_VISUAL_ENTITY.md). Concrete
Scene/Entity/Generator/Behavior/Director architecture: see
[`../PRESENCE_SCENES.md`](../PRESENCE_SCENES.md). Phased rollout: see
[`../PRESENCE_INTEGRATION_PLAN.md`](../PRESENCE_INTEGRATION_PLAN.md).

## Reason

- **Rust end-to-end, not a JS rendering layer** — this repo's assistant
  core (`ralleh-policy-core`, `ralleh-audio-core`, `ralleh-ai-router`,
  `ralleh-tool-gateway`) is already Rust, and `desktop-edge/src-tauri`
  already embeds `ralleh-audio-core` in-process (ADR-002's "Rust-first
  edge core," proven today by `mic.rs`). A `wgpu` renderer driven directly
  by the same process removes an entire interop layer (Rust → Tauri IPC →
  JS → Three.js) for what needs to be a **high-frequency** particle
  simulation (thousands of points updated every frame). This is a
  deliberate exception to the general ADR-001 polyglot split ("UI/display
  logic is TypeScript") — the split still holds for the rest of
  `desktop-edge` (settings, onboarding, chrome), but this specific surface
  is simulation-and-rendering-heavy enough, and tightly-enough coupled to
  live Rust-side state, that Rust-first is the better fit than the general rule.
- **Performance and control** — `wgpu` gives direct control over the
  render pipeline (instanced billboards + soft-falloff shader, see
  `PRESENCE_VISUAL_ENTITY.md` §7.4) without a browser engine's overhead,
  and avoids the DOM/WebGL abstraction layers Three.js sits on top of.
- **Cleaner window behavior** — frameless, always-on-top, and transparency
  requirements (documented as future extensions, not Phase 1 scope) are
  native `winit`/OS-window concerns; a webview adds an extra layer of
  indirection for the same behavior.
- **Smaller, more predictable runtime footprint** — no bundled/webview
  rendering surface duplicated just for this one widget.

Three.js is not discarded from the toolbox entirely — it remains valid for
a throwaway motion-language spike if ever useful — but it is explicitly
**not** the path this repo is taking. The Phase 1 prototype goes straight
to `winit`+`wgpu`, skipping the JS spike, per the source design's own
"production-aligned path" option and this project's stated preference to
do this work in Rust.

## Consequences

- **New, non-workspace Cargo project**: `presence-prototype/` at the repo
  root (sibling to `desktop-edge/`), added to the root `Cargo.toml`
  `[workspace] exclude` list — mirroring the existing precedent for
  `desktop-edge/src-tauri` ("needs a window/GPU surface; not headless
  CI"). This keeps `cargo test --workspace` unaffected and headless-safe,
  consistent with `HEADLESS.md`'s rule for hardware/display-dependent code.
- New Rust dependencies (in `presence-prototype/`, not the main workspace):
  `winit`, `wgpu`, `noise`, `glam` (vector math), `bytemuck` (GPU buffer
  casts), `pollster` (blocking on `wgpu`'s async init), `egui`/`egui-wgpu`/
  `egui-winit` (dev-only debug overlay + mode controls, per
  `PRESENCE_VISUAL_ENTITY.md` §9's "simple debug overlay" requirement).
- **Window/embedding question deferred to Phase 2.** Phase 1 is
  deliberately a fully standalone binary — it does not attempt to answer
  whether the eventual production surface is (a) a Tauri-managed window
  with a raw `wgpu` surface attached (bypassing the webview for that one
  window while staying inside the same app/process), or (b) a fully
  separate `winit` window/process alongside the Tauri app. Both are
  compatible with everything decided here; neither is decided yet. The
  original in-window-vs-floating-widget question from the first version
  of this ADR is subsumed by this and will be resolved in Phase 2 once the
  prototype has proven the visual language.
- Palette deviates from the source concept's literal lime/yellow-green
  LiDAR hues in favor of this repo's existing brand tokens (`--teal`,
  `--foam`, `--mist`, `--amber`) — unchanged from the first revision of
  this ADR, recorded in `PRESENCE_VISUAL_ENTITY.md` §3.1.
- Real signal wiring (mode changes from live VAD/ai-router/tool-gateway
  state) remains blocked on the same prerequisites identified before:
  `NEXT_STEPS.md` #13 (live VAD in the shell) and the fact that
  `desktop-edge` does not yet embed `ralleh-ai-router`/`ralleh-tool-gateway`
  at all. Unaffected by this revision — see `PRESENCE_INTEGRATION_PLAN.md` §3 (D5).
