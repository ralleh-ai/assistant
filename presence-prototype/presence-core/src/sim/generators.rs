//! Point generation — `docs/PRESENCE_SCENES.md` §5.2.
//!
//! Only the seam lives here. The built-in generator is
//! `crate::sim::shapes::SurfaceGenerator`, which seeds a population across a
//! `SurfaceShape`'s parameter space; it sits next to the shapes it seeds for
//! rather than here, because a generator and the surface it draws from are
//! meaningless apart.
//!
//! The earlier volume-filling `ClusterGenerator` was removed rather than kept
//! alongside: distributing points *through* a sphere is the model the surface
//! framework exists to replace, and keeping a second, contradictory generation
//! model in the tree invites new scenes to be built on the one that cannot
//! read as scanned. See `crate::sim::shapes` for the argument and
//! `docs/adr/adr-011-surface-presence-generation.md` for the decision.

use crate::sim::types::{EntityParams, Particle};

/// How points are created / reset for an entity.
pub trait PointGenerator {
    fn generate(&self, count: usize, params: &EntityParams) -> Vec<Particle>;
}
