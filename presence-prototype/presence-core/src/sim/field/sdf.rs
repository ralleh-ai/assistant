//! Morph via signed distance fields — coalescing a free-space cloud onto a
//! shape without ever handing a particle a per-particle destination.
//!
//! # Why an SDF and not target points
//!
//! The naive way to morph a cloud into a sphere is to assign every particle a
//! spot on the sphere and spring it there. That bakes the point count and the
//! correspondence into the data, fights the field motion, and looks like an
//! assembly line. An SDF instead defines the shape *implicitly*: at any
//! position it returns the signed distance to the surface (negative inside),
//! and its gradient is the surface normal. Pulling each particle "downhill"
//! toward the zero level set makes the whole cloud find the shape
//! collectively — points flow onto it from wherever they are, and the curl and
//! drift forces keep circulating *along* it. This is the ADR-014 M5 substrate,
//! and the same projection is what an M8 compute shader would evaluate per
//! particle.
//!
//! # Coherence is cognition
//!
//! The morph is not all-or-nothing. [`SdfAttractor`] scales its pull by the
//! entity's `focus` (how hard it snaps to the shape) and `confidence` (how
//! tightly), both copied onto `EntityParams` from the Behavior Graph's
//! cognitive state. A diffuse, low-focus presence is a loose nebula that only
//! suggests the shape; a focused, confident one condenses onto it.

use glam::{Vec2, Vec3};

use crate::sim::field::{FieldSample, ForceField};

/// An implicit shape a free-space cloud can morph onto. All targets are defined
/// in the entity's **local** frame (centred at the origin, unit-scaled); the
/// [`SdfAttractor`] transforms particle positions into that frame before
/// sampling, so a target's dimensions are in the same units as
/// `FieldCloudGenerator::radius`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MorphTarget {
    /// A hollow sphere of the given radius.
    Sphere { radius: f32 },
    /// A torus (ring) lying in the x–z plane, `major` from centre to tube
    /// centre and `minor` the tube radius.
    Ring { major: f32, minor: f32 },
    /// A helix winding around the y axis: `radius` from the axis, `pitch` the
    /// vertical rise per radian, `thickness` the tube radius.
    Helix {
        radius: f32,
        pitch: f32,
        thickness: f32,
    },
}

impl MorphTarget {
    /// Signed distance from `p` (local frame) to the surface: negative inside,
    /// zero on it, positive outside.
    ///
    /// The sphere and ring are exact SDFs. The helix is an *approximate* one —
    /// distance to the point on the curve at the same height rather than the
    /// true nearest point — which is all an attractor target needs: it is
    /// monotone toward the curve, and the finite-difference gradient turns it
    /// into a usable pull.
    pub fn sdf(&self, p: Vec3) -> f32 {
        match *self {
            MorphTarget::Sphere { radius } => p.length() - radius,
            MorphTarget::Ring { major, minor } => {
                let q = Vec2::new(Vec2::new(p.x, p.z).length() - major, p.y);
                q.length() - minor
            }
            MorphTarget::Helix {
                radius,
                pitch,
                thickness,
            } => {
                let theta = if pitch.abs() > 1e-6 { p.y / pitch } else { 0.0 };
                let curve = Vec3::new(radius * theta.cos(), p.y, radius * theta.sin());
                (p - curve).length() - thickness
            }
        }
    }

    /// Unit surface normal at `p` — the gradient of [`sdf`](Self::sdf),
    /// normalized. Analytic for the sphere; central finite differences for the
    /// ring and helix, which keeps every target on one code path a reader can
    /// trust without re-deriving a gradient per shape.
    pub fn gradient(&self, p: Vec3) -> Vec3 {
        if let MorphTarget::Sphere { .. } = self {
            return p.normalize_or_zero();
        }
        let eps = 1e-3;
        let dx = self.sdf(p + Vec3::X * eps) - self.sdf(p - Vec3::X * eps);
        let dy = self.sdf(p + Vec3::Y * eps) - self.sdf(p - Vec3::Y * eps);
        let dz = self.sdf(p + Vec3::Z * eps) - self.sdf(p - Vec3::Z * eps);
        Vec3::new(dx, dy, dz).normalize_or_zero()
    }

    /// The nearest point on the surface to `p`: step from `p` against the
    /// gradient by the signed distance. Exact for the sphere; a good first
    /// iterate for the others (Newton step on the level set).
    pub fn project(&self, p: Vec3) -> Vec3 {
        p - self.gradient(p) * self.sdf(p)
    }
}

/// Pulls each particle toward a [`MorphTarget`]'s surface, with a strength that
/// rises with the entity's cognitive `focus` and `confidence`. This is the M5
/// morph force: replace the point [`Attractor`](crate::sim::field::Attractor)
/// with this and the cloud coalesces onto a shape instead of a dot.
pub struct SdfAttractor {
    pub target: MorphTarget,
    /// Base spring constant toward the surface (1/s²) at full coherence.
    pub strength: f32,
    /// Coherence floor so the cloud still holds a loose shape at zero focus,
    /// rather than dispersing. `coherence = floor + (1 - floor)·focus`.
    pub coherence_floor: f32,
}

impl SdfAttractor {
    pub fn new(target: MorphTarget, strength: f32) -> Self {
        Self {
            target,
            strength,
            coherence_floor: 0.15,
        }
    }
}

impl ForceField for SdfAttractor {
    fn force(&self, sample: &FieldSample) -> Vec3 {
        let center = sample.params.center;
        let scale = sample.params.scale.max(1e-4);
        // Into the target's local frame.
        let local = (sample.position - center) / scale;

        let distance = self.target.sdf(local);
        let normal = self.target.gradient(local);

        let focus = sample.params.focus.clamp(0.0, 1.0);
        let confidence = sample.params.confidence.clamp(0.0, 1.0);
        let coherence = self.coherence_floor + (1.0 - self.coherence_floor) * focus;
        // Confidence tightens: an unsure presence morphs loosely (neutral 0.5
        // → 0.75× pull), a certain one snaps to the shape.
        let tightness = 0.5 + 0.5 * confidence;

        // Downhill toward the zero level set (`-normal·distance`), back into
        // world units via `scale`.
        -normal * distance * (self.strength * coherence * tightness * scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::field::{CompositeField, Curl, FieldBehavior, FieldCloudGenerator};
    use crate::sim::types::{EntityParams, Particle, PresenceSignals};
    use crate::sim::{PointBehavior, PointGenerator};

    #[test]
    fn sphere_sdf_signs_and_surface() {
        let s = MorphTarget::Sphere { radius: 1.0 };
        assert!(s.sdf(Vec3::ZERO) < 0.0, "centre should be inside");
        assert!(
            s.sdf(Vec3::new(2.0, 0.0, 0.0)) > 0.0,
            "outside should be positive"
        );
        assert!(
            s.sdf(Vec3::new(1.0, 0.0, 0.0)).abs() < 1e-6,
            "surface should be ~0"
        );
    }

    #[test]
    fn sphere_gradient_points_outward() {
        let s = MorphTarget::Sphere { radius: 1.0 };
        let g = s.gradient(Vec3::new(0.5, 0.0, 0.0));
        assert!(g.x > 0.99, "gradient should point radially outward: {g:?}");
    }

    #[test]
    fn projection_lands_on_the_surface() {
        for target in [
            MorphTarget::Sphere { radius: 1.0 },
            MorphTarget::Ring {
                major: 1.0,
                minor: 0.3,
            },
        ] {
            for p in [
                Vec3::new(2.0, 0.5, 0.0),
                Vec3::new(-1.5, -0.8, 0.7),
                Vec3::new(0.2, 0.1, 0.05),
            ] {
                let projected = target.project(p);
                assert!(
                    target.sdf(projected).abs() < 1e-2,
                    "projection off-surface for {target:?} at {p:?}: {}",
                    target.sdf(projected)
                );
            }
        }
    }

    fn morph_params(focus: f32, confidence: f32) -> EntityParams {
        let mut params = EntityParams::new(Vec3::ZERO, 1.0);
        params.focus = focus;
        params.confidence = confidence;
        params
    }

    fn sample_for(position: Vec3, params: &EntityParams) -> Vec3 {
        let signals = PresenceSignals::default();
        let attractor = SdfAttractor::new(MorphTarget::Sphere { radius: 1.0 }, 2.0);
        attractor.force(&FieldSample {
            position,
            velocity: Vec3::ZERO,
            time: 0.0,
            params,
            signals: &signals,
        })
    }

    #[test]
    fn attractor_pulls_toward_the_surface_from_both_sides() {
        let params = morph_params(1.0, 1.0);
        // Outside the sphere → pulled inward (toward origin).
        let outside = sample_for(Vec3::new(2.0, 0.0, 0.0), &params);
        assert!(outside.x < 0.0, "outside point not pulled in: {outside:?}");
        // Inside the sphere → pushed outward (toward the shell).
        let inside = sample_for(Vec3::new(0.3, 0.0, 0.0), &params);
        assert!(inside.x > 0.0, "inside point not pushed out: {inside:?}");
    }

    #[test]
    fn focus_and_confidence_strengthen_the_pull() {
        let p = Vec3::new(2.0, 0.0, 0.0);
        let weak = sample_for(p, &morph_params(0.0, 0.0)).length();
        let strong = sample_for(p, &morph_params(1.0, 1.0)).length();
        assert!(
            strong > weak * 2.0,
            "coherence did not scale the pull: {weak} -> {strong}"
        );
        // Even at zero focus the floor keeps some cohesion.
        assert!(weak > 0.0, "coherence floor gave no pull at zero focus");
    }

    fn morph_behavior(seed: u32) -> FieldBehavior {
        let field = CompositeField::new()
            .push(Box::new(SdfAttractor::new(
                MorphTarget::Sphere { radius: 1.0 },
                6.0,
            )))
            .push(Box::new(Curl::new(seed, 0.8, 0.15, 0.3)));
        FieldBehavior::new(field, 2.0, 3.0)
    }

    fn mean_abs_sdf(particles: &[Particle], params: &EntityParams) -> f32 {
        let target = MorphTarget::Sphere { radius: 1.0 };
        let scale = params.scale.max(1e-4);
        let sum: f32 = particles
            .iter()
            .map(|p| target.sdf((p.position - params.center) / scale).abs())
            .sum();
        sum / particles.len() as f32
    }

    #[test]
    fn a_focused_cloud_condenses_onto_the_shape() {
        let signals = PresenceSignals::default();
        let gen = FieldCloudGenerator::new(0xABCD, 2.0);

        let run = |focus: f32, confidence: f32| {
            let params = morph_params(focus, confidence);
            let mut behavior = morph_behavior(0xABCD);
            let mut particles = gen.generate(300, &params);
            let start = mean_abs_sdf(&particles, &params);
            for _ in 0..600 {
                behavior.update(&mut particles, 1.0 / 60.0, &params, &signals);
            }
            (start, mean_abs_sdf(&particles, &params))
        };

        let (focused_start, focused_end) = run(1.0, 1.0);
        assert!(
            focused_end < focused_start * 0.5,
            "focused cloud did not condense onto the sphere: {focused_start} -> {focused_end}"
        );

        // Focused converges tighter than an unfocused (floor-only) cloud.
        let (_, unfocused_end) = run(0.0, 0.0);
        assert!(
            focused_end < unfocused_end,
            "focus did not tighten the morph: focused {focused_end} vs unfocused {unfocused_end}"
        );
    }

    #[test]
    fn morph_is_deterministic() {
        let signals = PresenceSignals::default();
        let params = morph_params(1.0, 1.0);
        let gen = FieldCloudGenerator::new(5, 2.0);
        let run = || {
            let mut behavior = morph_behavior(5);
            let mut particles = gen.generate(120, &params);
            for _ in 0..120 {
                behavior.update(&mut particles, 1.0 / 60.0, &params, &signals);
            }
            particles
        };
        let a = run();
        let b = run();
        for (pa, pb) in a.iter().zip(b.iter()) {
            assert_eq!(pa.position, pb.position);
        }
    }
}
