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
    /// A torus (ring) **facing the camera** — it lies in the x–y plane, with
    /// its axis along z, `major` from centre to tube centre and `minor` the
    /// tube radius.
    ///
    /// The obvious definition is the x–z plane (axis up), and that is what this
    /// was. It is also invisible: the camera sits at `(0, 0.3, 5.2)` looking
    /// down −z, so a ring lying flat is seen edge-on and renders as a bar. A
    /// shape the presence cannot be seen holding is not a shape it can hold, so
    /// the orientation is part of the target's definition rather than something
    /// left to whoever places it.
    Ring { major: f32, minor: f32 },
    /// A helix winding around the y axis: `radius` from the axis, `pitch` the
    /// vertical rise per radian, `thickness` the tube radius.
    Helix {
        radius: f32,
        pitch: f32,
        thickness: f32,
    },
    /// A heart, facing the camera in the x–y plane like the [`Ring`](Self::Ring)
    /// and for the same reason.
    ///
    /// Unlike its neighbours here this one is *parametric*: it is defined by
    /// where its surface is, not by a distance to it. See
    /// [`has_distance_field`](Self::has_distance_field) — the classic heart
    /// curve has no closed-form distance, and iterating for one per particle
    /// per step is exactly the per-particle projection cost ADR-011 rejected.
    /// Its stations carry it instead, which is the same way the droplet's skin
    /// works.
    Heart {
        /// Half-width, in the target's local frame.
        size: f32,
        /// Half-depth along z, as a share of `size`.
        depth: f32,
    },
}

/// The classic heart curve, normalized to roughly `[-1, 1]` in x and centred
/// on the origin in y, at parameter `t` radians.
///
/// `t = 0` is the dimple between the lobes and `t = π` the point at the bottom.
fn heart_outline(t: f32) -> Vec2 {
    let (sin, cos) = t.sin_cos();
    let x = sin * sin * sin;
    let y = (13.0 * cos - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos()
        + HEART_Y_CENTRE)
        / 16.0;
    Vec2::new(x, y)
}

/// Offset that centres the heart curve's `[-17, 5.2]` y range on zero before
/// it is scaled. Off-centre, the shape orbits its own corner when the body
/// turns.
const HEART_Y_CENTRE: f32 = 5.9;

impl MorphTarget {
    /// Whether this target can answer [`distance_and_normal`](Self::distance_and_normal).
    ///
    /// Implicit targets are defined by a distance and can pull a particle from
    /// anywhere toward the nearest part of themselves. Parametric ones are
    /// defined by where their surface *is*: they can place a particle exactly,
    /// but cannot cheaply say how far away one currently is. A shape worth
    /// holding is not always a shape with a closed-form distance, and the
    /// interesting ones — a heart, and anything sampled off a model — are
    /// mostly the second kind, so the attractor accommodates both rather than
    /// limiting the vocabulary to what happens to be expressible implicitly.
    pub fn has_distance_field(&self) -> bool {
        !matches!(self, MorphTarget::Heart { .. })
    }

    /// Signed distance from `p` (local frame) to the surface: negative inside,
    /// zero on it, positive outside.
    ///
    /// Parametric targets have none, and report zero — callers must check
    /// [`has_distance_field`](Self::has_distance_field) first.
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
                let q = Vec2::new(Vec2::new(p.x, p.y).length() - major, p.z);
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
            MorphTarget::Heart { .. } => 0.0,
        }
    }

    /// Signed distance *and* unit surface normal at `p`, in one pass.
    ///
    /// # Why analytic, and why together
    ///
    /// These were central finite differences, which was six extra `sdf` calls
    /// per query and cost 4.5 ms per step on the shell's 80k points — for
    /// derivatives these shapes have in closed form. The analytic gradients
    /// below are both several times cheaper and exact, so this is not a
    /// quality-for-speed trade.
    ///
    /// Returned as a pair because every caller needs both, and the distance is
    /// already most of the work of the normal — asking for them separately
    /// computed the shared part twice.
    pub fn distance_and_normal(&self, p: Vec3) -> (f32, Vec3) {
        match *self {
            MorphTarget::Sphere { radius } => (p.length() - radius, p.normalize_or_zero()),

            // d(|q|)/dp where q = (|p.xy| - major, p.z): the radial component
            // of q spreads back over x and y by their share of |p.xy|.
            MorphTarget::Ring { major, minor } => {
                let r = Vec2::new(p.x, p.y).length();
                let q = Vec2::new(r - major, p.z);
                let len = q.length();
                let distance = len - minor;
                if len <= 1e-6 || r <= 1e-6 {
                    return (distance, Vec3::Z);
                }
                let radial = q.x / len;
                let normal = Vec3::new(radial * p.x / r, radial * p.y / r, q.y / len);
                (distance, normal.normalize_or_zero())
            }

            // d = p - curve(p.y). The x/z components are just d̂; the y
            // component also picks up how the curve itself slides as y moves,
            // which is the `-c'(y)` term.
            MorphTarget::Helix {
                radius,
                pitch,
                thickness,
            } => {
                let theta = if pitch.abs() > 1e-6 { p.y / pitch } else { 0.0 };
                let (sin, cos) = theta.sin_cos();
                let d = p - Vec3::new(radius * cos, p.y, radius * sin);
                let len = d.length();
                let distance = len - thickness;
                if len <= 1e-6 {
                    return (distance, Vec3::Y);
                }
                let dy = if pitch.abs() > 1e-6 {
                    (d.x * radius * sin - d.z * radius * cos) / (pitch * len)
                } else {
                    0.0
                };
                (
                    distance,
                    Vec3::new(d.x / len, dy, d.z / len).normalize_or_zero(),
                )
            }

            MorphTarget::Heart { .. } => (0.0, Vec3::ZERO),
        }
    }

    /// Unit surface normal at `p` — the gradient of [`sdf`](Self::sdf),
    /// normalized.
    pub fn gradient(&self, p: Vec3) -> Vec3 {
        self.distance_and_normal(p).1
    }

    /// The nearest point on the surface to `p`: step from `p` against the
    /// gradient by the signed distance. Exact for the sphere; a good first
    /// iterate for the others (Newton step on the level set).
    pub fn project(&self, p: Vec3) -> Vec3 {
        let (distance, normal) = self.distance_and_normal(p);
        p - normal * distance
    }

    /// A point on the surface addressed by two `[0, 1)` coordinates, spread
    /// evenly over the shape.
    ///
    /// # Why a target needs this at all
    ///
    /// A signed distance says how far a point is from the surface and nothing
    /// about *where along* it that point belongs, so an attractor built only on
    /// the distance moves every particle to whatever part of the shape it
    /// happened to be nearest. The resulting body is on the surface — the
    /// distance is genuinely zero — but its coverage is inherited from whatever
    /// shape it was holding before. Arriving from the droplet's even sphere
    /// that is invisible; arriving from the helix, which is wound around one
    /// axis, it piles the whole body into a couple of clumps and the shape is
    /// unrecognisable despite being dimensionally correct.
    ///
    /// Giving each particle a station addressed by its own fixed seed makes
    /// coverage a property of the target rather than of the route taken to it.
    pub fn surface_point(&self, a: f32, b: f32) -> Vec3 {
        let a = a.clamp(0.0, 1.0);
        let b = b.clamp(0.0, 1.0);
        let tau = std::f32::consts::TAU;
        match *self {
            // Equal-area: z uniform, not the polar angle, or the points bunch
            // at the poles.
            MorphTarget::Sphere { radius } => {
                let z = 2.0 * a - 1.0;
                let r = (1.0 - z * z).max(0.0).sqrt();
                let (sin, cos) = (tau * b).sin_cos();
                Vec3::new(r * cos, r * sin, z) * radius
            }
            MorphTarget::Ring { major, minor } => {
                let (sin_az, cos_az) = (tau * a).sin_cos();
                let (sin_tube, cos_tube) = (tau * b).sin_cos();
                let radial = Vec3::new(cos_az, sin_az, 0.0);
                radial * (major + minor * cos_tube) + Vec3::Z * (minor * sin_tube)
            }
            MorphTarget::Helix {
                radius,
                pitch,
                thickness,
            } => {
                // Spread along the same span of turns the body actually
                // occupies, so the stations cover the visible curve rather than
                // an arbitrary stretch of an infinite one.
                let theta = (a * 2.0 - 1.0) * HELIX_TURNS * tau;
                let y = theta * pitch;
                let (sin, cos) = theta.sin_cos();
                let axis = Vec3::new(cos, 0.0, sin);
                // Around the tube, in the plane normal to the curve's own run.
                let (sin_tube, cos_tube) = (tau * b).sin_cos();
                Vec3::new(radius * cos, y, radius * sin)
                    + axis * (thickness * cos_tube)
                    + Vec3::Y * (thickness * sin_tube)
            }

            // The sphere's construction with its circular cross-section
            // replaced by the heart outline: sweep the outline from the front
            // pole to the back one, shrinking it toward each. That closes the
            // surface, keeps the silhouette an exact heart when seen from the
            // camera, and inherits the sphere's even spacing.
            MorphTarget::Heart { size, depth } => {
                let w = 2.0 * a - 1.0;
                let shrink = (1.0 - w * w).max(0.0).sqrt();
                let outline = heart_outline(tau * b) * (size * shrink);
                Vec3::new(outline.x, outline.y, w * depth * size)
            }
        }
    }
}

/// Turns either side of centre that [`MorphTarget::surface_point`] spreads a
/// helix's stations over. The curve itself is unbounded in y; this is the part
/// of it the presence is asked to be.
const HELIX_TURNS: f32 = 0.75;

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
    /// Share of the pull aimed at the particle's own station on the surface
    /// (see [`MorphTarget::surface_point`]) rather than at the nearest part of
    /// it.
    ///
    /// The distance term alone leaves coverage inherited from the previous
    /// shape, which is how a body can be exactly on a ring and still not look
    /// like one. This term is what spreads it. It is a share rather than the
    /// whole pull because a station is a fixed point and pulling only toward
    /// fixed points is the surface spring again, not a field — the distance
    /// term is what keeps the arrival loose and flowing.
    pub spread: f32,
}

impl SdfAttractor {
    pub fn new(target: MorphTarget, strength: f32) -> Self {
        Self {
            target,
            strength,
            coherence_floor: 0.15,
            spread: DEFAULT_SPREAD,
        }
    }
}

/// Enough station-seeking to guarantee even coverage from any starting shape,
/// while the distance term still carries most of the motion.
const DEFAULT_SPREAD: f32 = 0.45;

impl ForceField for SdfAttractor {
    fn force(&self, sample: &FieldSample) -> Vec3 {
        let center = sample.params.center;
        let scale = sample.params.scale.max(1e-4);
        // Into the target's local frame.
        let local = (sample.position - center) / scale;

        // A parametric target has no distance to descend, so its stations carry
        // the whole pull rather than a share of it.
        let implicit = self.target.has_distance_field();
        let spread = if implicit {
            self.spread.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let (distance, normal) = if implicit {
            self.target.distance_and_normal(local)
        } else {
            (0.0, Vec3::ZERO)
        };

        let focus = sample.params.focus.clamp(0.0, 1.0);
        let confidence = sample.params.confidence.clamp(0.0, 1.0);
        let coherence = self.coherence_floor + (1.0 - self.coherence_floor) * focus;
        // Confidence tightens: an unsure presence morphs loosely (neutral 0.5
        // → 0.75× pull), a certain one snaps to the shape.
        let tightness = 0.5 + 0.5 * confidence;

        // Downhill toward the zero level set (`-normal·distance`).
        let to_surface = -normal * distance;

        // And toward this particular particle's station on it. The two seeds
        // are decorrelated from the one the sample carries so that a station's
        // two coordinates do not march together, which would lay the body along
        // a diagonal of the surface instead of over it.
        let pull = if spread > 1e-4 {
            let (a, b) = station_seeds(sample.seed01);
            let station = self.target.surface_point(a, b);
            to_surface * (1.0 - spread) + (station - local) * spread
        } else {
            to_surface
        };

        // Back into world units via `scale`.
        pull * (self.strength * coherence * tightness * scale)
    }
}

/// Two decorrelated `[0, 1)` coordinates from one seed.
///
/// The multipliers are large irrationals so that neighbouring seeds land far
/// apart in both coordinates rather than on a lattice, which would show up as
/// visible banding across the surface.
pub(crate) fn station_seeds(seed: f32) -> (f32, f32) {
    let a = (seed * 1.618_034 + 0.137).fract();
    let b = (seed * 97.131_4 + 0.371).fract();
    (a.abs(), b.abs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::field::{CompositeField, Curl, FieldBehavior, FieldCloudGenerator};
    use crate::sim::types::{EntityParams, Particle, PresenceSignals};
    use crate::sim::{PointBehavior, PointGenerator};

    /// A heart is only a heart because of two features: the cleft between the
    /// lobes at the top and the point at the bottom. A shape that has neither
    /// is a lopsided ball, and since the camera looks straight down −z it is
    /// the x–y silhouette that has to carry them.
    #[test]
    fn the_heart_has_a_cleft_at_the_top_and_a_point_at_the_bottom() {
        let heart = MorphTarget::Heart {
            size: 1.0,
            depth: 0.42,
        };
        // The outline itself is the widest sweep, at the equator.
        let outline: Vec<Vec2> = (0..2_000)
            .map(|i| {
                let p = heart.surface_point(0.5, i as f32 / 2_000.0);
                Vec2::new(p.x, p.y)
            })
            .collect();

        let highest_in = |lo: f32, hi: f32| {
            outline
                .iter()
                .filter(|p| p.x.abs() >= lo && p.x.abs() <= hi)
                .map(|p| p.y)
                .fold(f32::MIN, f32::max)
        };
        let centre_top = highest_in(0.0, 0.05);
        let lobe_top = highest_in(0.3, 0.7);
        assert!(
            lobe_top > centre_top + 0.05,
            "no cleft: the middle rises to {centre_top} against the lobes' {lobe_top}"
        );

        // The point: the lowest place on the outline sits on the centre line.
        let bottom = outline
            .iter()
            .min_by(|a, b| a.y.total_cmp(&b.y))
            .expect("outline is not empty");
        assert!(
            bottom.x.abs() < 0.05,
            "the point is off to one side at x {}",
            bottom.x
        );

        // And it is symmetric, or it reads as a damaged heart rather than one.
        let centroid_x = outline.iter().map(|p| p.x).sum::<f32>() / outline.len() as f32;
        assert!(centroid_x.abs() < 0.02, "lopsided: centroid x {centroid_x}");
    }

    /// Parametric targets have no distance to descend, so anything that reaches
    /// for one must not silently read a zero distance as "already arrived".
    #[test]
    fn the_heart_reports_itself_as_parametric() {
        let heart = MorphTarget::Heart {
            size: 1.0,
            depth: 0.42,
        };
        assert!(!heart.has_distance_field());
        assert!(MorphTarget::Sphere { radius: 1.0 }.has_distance_field());

        // Still reaches for it: a station-only pull is a real force, not the
        // no-op a zero distance would produce through the implicit path.
        let params = EntityParams::new(Vec3::ZERO, 1.0);
        let signals = PresenceSignals::default();
        let attractor = SdfAttractor::new(heart, 6.0);
        let sample = FieldSample {
            position: Vec3::new(3.0, 2.5, 1.0),
            velocity: Vec3::ZERO,
            seed01: 0.37,
            time: 0.0,
            params: &params,
            signals: &signals,
        };
        let force = attractor.force(&sample);
        assert!(
            force.length() > 1e-3,
            "a parametric target exerted no pull: {force:?}"
        );
        assert!(
            force.dot(-sample.position) > 0.0,
            "the pull did not point back toward the shape: {force:?}"
        );
    }

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

    /// The analytic gradients replaced central finite differences for speed.
    /// This is the guard that the derivations are actually right — it holds
    /// them to the numeric answer they were derived from.
    /// A ring lying flat is invisible from the camera's position — it renders
    /// as a bar. This pins the orientation to the screen plane.
    #[test]
    fn the_ring_faces_the_camera() {
        let ring = MorphTarget::Ring {
            major: 1.05,
            minor: 0.28,
        };

        // Left/right and up/down are the tube centre: deepest inside the shape.
        for on_ring in [Vec3::new(1.05, 0.0, 0.0), Vec3::new(0.0, 1.05, 0.0)] {
            assert!(
                (ring.sdf(on_ring) + 0.28).abs() < 1e-5,
                "expected the tube centre across the screen plane at {on_ring:?}"
            );
        }

        // Toward the camera there is no ring at all — only its thickness.
        assert!(
            ring.sdf(Vec3::new(0.0, 0.0, 1.05)) > 0.5,
            "the ring extends along the view axis, so it will be seen edge-on"
        );
    }

    #[test]
    fn analytic_normals_agree_with_finite_differences() {
        let numeric = |target: &MorphTarget, p: Vec3| {
            let eps = 1e-3;
            let dx = target.sdf(p + Vec3::X * eps) - target.sdf(p - Vec3::X * eps);
            let dy = target.sdf(p + Vec3::Y * eps) - target.sdf(p - Vec3::Y * eps);
            let dz = target.sdf(p + Vec3::Z * eps) - target.sdf(p - Vec3::Z * eps);
            Vec3::new(dx, dy, dz).normalize_or_zero()
        };

        for target in [
            MorphTarget::Sphere { radius: 1.0 },
            MorphTarget::Ring {
                major: 1.05,
                minor: 0.28,
            },
            MorphTarget::Helix {
                radius: 0.75,
                pitch: 0.34,
                thickness: 0.16,
            },
        ] {
            for p in [
                Vec3::new(2.0, 0.5, 0.0),
                Vec3::new(-1.5, -0.8, 0.7),
                Vec3::new(0.4, 0.1, -1.1),
                Vec3::new(0.9, 1.4, 0.6),
                Vec3::new(-0.3, -1.7, -0.9),
            ] {
                let analytic = target.gradient(p);
                let fd = numeric(&target, p);
                assert!(
                    (analytic - fd).length() < 2e-2,
                    "gradient mismatch for {target:?} at {p:?}: analytic {analytic:?} vs fd {fd:?}"
                );
                assert!(
                    (analytic.length() - 1.0).abs() < 1e-3,
                    "gradient not unit length for {target:?}: {analytic:?}"
                );
            }
        }
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
            seed01: 0.5,
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
