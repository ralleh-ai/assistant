//! Point Cloud Presence — library core.
//!
//! This crate owns the simulation (`sim`), the scene director + registry
//! (`scene`), the wgpu-based renderer (`render`), and the palette settings
//! (`palette`). It intentionally *doesn't* own a window, an event loop, or
//! a debug UI — those belong to `presence-runtime`.
//!
//! See `docs/PRESENCE_VISUAL_ENTITY.md`, `docs/PRESENCE_SCENES.md`, and
//! `docs/adr/adr-013-presence-window-and-process-model.md` in the main
//! repo for the design and rationale, and `docs/PRESENCE_INTEGRATION_PLAN.md`
//! for how this crate fits into the overall roadmap.

pub mod palette;
pub mod render;
pub mod scene;
pub mod sim;

#[cfg(feature = "ipc")]
pub mod ipc;

#[cfg(feature = "ipc")]
pub use presence_ipc;
