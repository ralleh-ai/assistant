//! Point behavior — `docs/PRESENCE_SCENES.md` §5.3.
//!
//! Only the seam lives here. The built-in behavior is
//! `crate::sim::shapes::SurfaceBehavior`, which springs particles onto whatever
//! surface it is given and is therefore shared by every scene rather than
//! written once per scene; what differs between scenes is the `SurfaceShape`.
//!
//! `ViscousClusterBehavior`'s rise/elongate/thin/fall dynamics were removed
//! along with the volumetric generator they animated. Those dynamics are not
//! abandoned — `docs/PRESENCE_SCENES.md` §4.3 records them landing on Thinking
//! (lava-lamp rise) and ToolUse (oil-drip pendants) — but they belong on a
//! surface when they return, so the volumetric implementation would have had to
//! be rewritten rather than reused.

use crate::sim::types::{EntityParams, Particle, PresenceSignals};

/// How points evolve each frame.
pub trait PointBehavior {
    fn update(
        &mut self,
        particles: &mut [Particle],
        dt: f32,
        params: &EntityParams,
        signals: &PresenceSignals,
    );
}
