//! Surface seeding — `PRESENCE_ADAPTIVE_SCENES` Phase A.
//!
//! Ambient scenes feel foreign when their points ignore the shell's surface and
//! material model. The fix is to *birth* a fraction of an ambient scene's points
//! from the current `PresenceShell` skin: they inherit the skin's outward
//! `normal` (grazing rim) and `crease` (accent-hue filaments), then migrate to
//! their ambient role. This is the snapshot-at-birth model (ADR: chosen over a
//! live spring-to-shell): a one-time capture taken when the scene is presented,
//! so there is no per-frame coupling to the deforming shell and the perf path
//! (staggered deform/place) is untouched.
//!
//! Samples are stored in *shell-local* space (relative to the cloud's center and
//! scale), not world space, so the mechanism is placement-agnostic: a `Center`
//! scene emerges from the real shell, while a corner scene buds a shell-shaped
//! source in the corner — coherent either way.

use glam::Vec3;

use crate::sim::types::{Layer, Particle};

/// Upper bound on captured samples. The realizer indexes into these to birth
/// points, so a few thousand is ample variety while keeping the snapshot cheap
/// to clone into a generator/behavior.
pub const MAX_SEEDS: usize = 4096;

/// One captured point on the shell skin, in shell-local space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SurfaceSeed {
    /// Position relative to the cloud center, divided by cloud scale — so
    /// `center + local * scale` reconstructs a world position at any placement.
    pub local: Vec3,
    /// Inherited outward surface normal (unit).
    pub normal: Vec3,
    /// Inherited fold-crease intensity `0..1`.
    pub crease: f32,
}

/// Capture on-skin samples from a live shell population.
///
/// Only `Core`/`Body` layers with a real normal are kept — those are the points
/// that actually sit on the skin (`Halo` is atmosphere and carries no crease).
/// Sampling is strided (not the first N) so the capture covers the whole
/// surface rather than one seed-order region, and it is deterministic for a
/// given input, which keeps scene generation reproducible.
pub fn snapshot(particles: &[Particle], center: Vec3, scale: f32) -> Vec<SurfaceSeed> {
    if particles.is_empty() || scale <= f32::EPSILON {
        return Vec::new();
    }
    let inv_scale = 1.0 / scale;
    let eligible = particles.iter().filter(|p| {
        matches!(p.layer, Layer::Core | Layer::Body) && p.normal.length_squared() > 1e-8
    });

    // Stride so the cap spreads across the whole population.
    let approx = particles.len();
    let stride = (approx / MAX_SEEDS).max(1);
    let mut out = Vec::with_capacity(MAX_SEEDS.min(approx));
    for p in eligible.step_by(stride) {
        out.push(SurfaceSeed {
            local: (p.position - center) * inv_scale,
            normal: p.normal.normalize_or_zero(),
            crease: p.crease.clamp(0.0, 1.0),
        });
        if out.len() >= MAX_SEEDS {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skin_particle(pos: Vec3, normal: Vec3, crease: f32, layer: Layer) -> Particle {
        Particle {
            position: pos,
            normal,
            crease,
            layer,
            ..Default::default()
        }
    }

    #[test]
    fn snapshot_maps_to_shell_local_space() {
        let center = Vec3::new(1.0, 2.0, 0.0);
        let scale = 2.0;
        let particles = vec![skin_particle(
            center + Vec3::new(2.0, 0.0, 0.0),
            Vec3::X,
            0.5,
            Layer::Core,
        )];
        let seeds = snapshot(&particles, center, scale);
        assert_eq!(seeds.len(), 1);
        assert!((seeds[0].local - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-6);
        assert_eq!(seeds[0].crease, 0.5);
    }

    #[test]
    fn snapshot_skips_halo_and_normalless_points() {
        let center = Vec3::ZERO;
        let particles = vec![
            skin_particle(Vec3::X, Vec3::X, 0.2, Layer::Halo),
            skin_particle(Vec3::Y, Vec3::ZERO, 0.2, Layer::Core),
            skin_particle(Vec3::Z, Vec3::Z, 0.2, Layer::Body),
        ];
        let seeds = snapshot(&particles, center, 1.0);
        assert_eq!(seeds.len(), 1, "only the on-skin Body point qualifies");
    }

    #[test]
    fn snapshot_is_capped_and_deterministic() {
        let center = Vec3::ZERO;
        let particles: Vec<Particle> = (0..50_000)
            .map(|i| {
                let a = i as f32 * 0.001;
                let dir = Vec3::new(a.cos(), a.sin(), 0.3).normalize();
                skin_particle(dir, dir, 0.1, Layer::Core)
            })
            .collect();
        let a = snapshot(&particles, center, 1.0);
        let b = snapshot(&particles, center, 1.0);
        assert!(a.len() <= MAX_SEEDS);
        assert_eq!(a, b);
    }
}
