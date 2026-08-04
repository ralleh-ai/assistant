pub mod behaviors;
pub mod field;
pub mod form;
pub mod generators;
pub mod morph;
pub mod noise;
pub mod shapes;
pub mod terms;
pub mod types;

/// Snaps a velocity that has decayed to nothing to exactly zero.
///
/// # Why this is not a micro-optimisation
///
/// A damped spring approaches rest asymptotically, so a settled particle's
/// velocity keeps halving forever. Once the components fall below about
/// `1.2e-38` they become *denormal*, and x86 handles denormal arithmetic in
/// microcode at something like a hundredth of normal speed. Every particle in a
/// settled body reaches that range at about the same time, so the simulation
/// does not degrade gradually — it falls off a cliff a few seconds after the
/// body stops moving, which reads as the presence mysteriously dropping frames
/// the longer it holds still.
///
/// The threshold is far above the denormal range and far below anything
/// visible: at this speed a particle would take a day to cross a pixel.
pub fn flush_to_rest(velocity: glam::Vec3) -> glam::Vec3 {
    if velocity.length_squared() < REST_SPEED_SQUARED {
        glam::Vec3::ZERO
    } else {
        velocity
    }
}

/// Squared speed below which a particle is considered stopped. See
/// [`flush_to_rest`].
const REST_SPEED_SQUARED: f32 = 1e-12;

pub use behaviors::PointBehavior;
pub use form::{FormTarget, FormTransition, FormWeights};
pub use generators::PointGenerator;
pub use morph::MorphBehavior;
pub use types::{EntityParams, Layer, Particle, PresenceSignals, ShellDrive};
