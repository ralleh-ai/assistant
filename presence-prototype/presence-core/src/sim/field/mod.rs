//! Force-field substrate — a CPU integrator for **free-space** entities.
//!
//! # Scope, and why this is not the shell
//!
//! ADR-011 rejected force/curl advection for the scanned presence shell: the
//! shell is a *surface*, and forces push points off a skin. That decision
//! stands. This module is the other half ADR-014 carves out — entities that
//! have no skin (nebulae, orbits, morphing clouds) and for which "a point is
//! wherever the field carries it" is exactly right. `noise.rs::curl` —
//! divergence-free, and dead code until now — is finally the correct tool
//! here.
//!
//! # Shape of the substrate
//!
//! A [`ForceField`] returns an acceleration at a sample point; a
//! [`CompositeField`] sums several; [`FieldBehavior`] is the
//! [`PointBehavior`](crate::sim::PointBehavior) that integrates the composite
//! over the particle set with velocity damping and a speed clamp. The forces
//! are plain data (centres, strengths, frequencies) rather than closures, so
//! the M8 port that moves this integration to a WGSL compute pass is a
//! mechanical translation of a handful of parameters, not a rewrite.

use glam::Vec3;

use crate::sim::noise::NoiseField;
use crate::sim::types::{hash01, EntityParams, Layer, Particle, PresenceSignals};
use crate::sim::PointBehavior;

pub mod sdf;
pub use sdf::{MorphTarget, SdfAttractor};

/// Everything a [`ForceField`] may read about the point it is being sampled
/// for. Borrowed, so sampling allocates nothing on the hot path.
pub struct FieldSample<'a> {
    pub position: Vec3,
    pub velocity: Vec3,
    /// The behavior's own accumulated, time-scaled clock — the 4th dimension
    /// the noise fields drift along, so the whole field evolves coherently.
    pub time: f32,
    pub params: &'a EntityParams,
    pub signals: &'a PresenceSignals,
}

/// A contribution to a free-space particle's acceleration.
pub trait ForceField {
    /// Acceleration at `sample`, in world units per second squared.
    fn force(&self, sample: &FieldSample) -> Vec3;
}

/// A linear (Hooke) pull toward a point. Bounded by construction — the force
/// grows with distance rather than blowing up near the centre — so a cloud
/// under it settles into a damped orbit instead of collapsing to a singularity
/// the way an inverse-square attractor would. This is also the seed of M5's
/// morph: swap `target` for a per-particle SDF-projected point and the same
/// spring coalesces the cloud onto a shape.
pub struct Attractor {
    pub target: Vec3,
    /// Spring constant (1/s²). Higher pulls harder / orbits tighter.
    pub strength: f32,
}

impl ForceField for Attractor {
    fn force(&self, sample: &FieldSample) -> Vec3 {
        (self.target - sample.position) * self.strength
    }
}

/// Smooth low-frequency wander from the noise field's vector potential. Not
/// divergence-free (see [`NoiseField::drift`]) — which does not matter against
/// a spring, and is why it is three noise evaluations rather than curl's
/// eighteen. Gives the cloud its unhurried, alive-at-rest breathing.
pub struct Drift {
    field: NoiseField,
    /// Spatial frequency: larger = finer, more local variation.
    pub frequency: f32,
    pub strength: f32,
    /// How fast the field itself evolves in the time dimension.
    pub time_scale: f32,
}

impl Drift {
    pub fn new(seed: u32, frequency: f32, strength: f32, time_scale: f32) -> Self {
        Self {
            field: NoiseField::new(seed),
            frequency,
            strength,
            time_scale,
        }
    }
}

impl ForceField for Drift {
    fn force(&self, sample: &FieldSample) -> Vec3 {
        self.field.drift(
            sample.position * self.frequency,
            sample.time * self.time_scale,
        ) * self.strength
    }
}

/// Divergence-free swirl — the curl of the noise potential. This is the
/// source/sink-free vortex motion ADR-014 wants for free space: points circulate
/// without the field ever pumping them all outward or sucking them all in, so a
/// nebula stays a nebula.
pub struct Curl {
    field: NoiseField,
    pub frequency: f32,
    pub strength: f32,
    /// Finite-difference step for the curl. Larger smooths the swirl.
    pub eps: f32,
    pub time_scale: f32,
}

impl Curl {
    pub fn new(seed: u32, frequency: f32, strength: f32, time_scale: f32) -> Self {
        Self {
            field: NoiseField::new(seed),
            frequency,
            strength,
            eps: 0.25,
            time_scale,
        }
    }
}

impl ForceField for Curl {
    fn force(&self, sample: &FieldSample) -> Vec3 {
        self.field.curl(
            sample.position * self.frequency,
            sample.time * self.time_scale,
            self.eps,
        ) * self.strength
    }
}

/// Multi-octave curl — the same divergence-free swirl summed at rising
/// frequency and falling amplitude, adding fine, fast eddies on top of the
/// broad circulation. Kept separate from [`Curl`] so a template can dial the
/// large-scale flow and the fine detail independently.
pub struct Turbulence {
    field: NoiseField,
    pub frequency: f32,
    pub strength: f32,
    pub octaves: u32,
    pub eps: f32,
    pub time_scale: f32,
}

impl Turbulence {
    pub fn new(seed: u32, frequency: f32, strength: f32, octaves: u32, time_scale: f32) -> Self {
        Self {
            field: NoiseField::new(seed),
            frequency,
            strength,
            octaves: octaves.max(1),
            eps: 0.2,
            time_scale,
        }
    }
}

impl ForceField for Turbulence {
    fn force(&self, sample: &FieldSample) -> Vec3 {
        let mut freq = self.frequency;
        let mut amp = self.strength;
        let mut sum = Vec3::ZERO;
        let t = sample.time * self.time_scale;
        for _ in 0..self.octaves {
            sum += self.field.curl(sample.position * freq, t, self.eps) * amp;
            freq *= 2.0;
            amp *= 0.5;
        }
        sum
    }
}

/// A set of fields whose accelerations add. Composition is a sum because
/// forces superpose — an attractor and a swirl acting at once is just their
/// vector sum, which needs no special case.
#[derive(Default)]
pub struct CompositeField {
    fields: Vec<Box<dyn ForceField>>,
}

impl CompositeField {
    pub fn new() -> Self {
        Self { fields: Vec::new() }
    }

    pub fn push(mut self, field: Box<dyn ForceField>) -> Self {
        self.fields.push(field);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

impl ForceField for CompositeField {
    fn force(&self, sample: &FieldSample) -> Vec3 {
        self.fields
            .iter()
            .fold(Vec3::ZERO, |acc, f| acc + f.force(sample))
    }
}

/// Integrates a [`CompositeField`] over a free-space particle set: semi-implicit
/// Euler, exponential velocity damping, and a hard speed clamp.
///
/// The damping is exponential (`v *= e^(-damping·dt)`) rather than a
/// per-frame multiply so the decay is frame-rate independent — the same real
/// time bleeds the same fraction of speed whether the sim runs at 30 or 144 Hz.
/// The speed clamp is the safety rail that keeps a mis-tuned or hostile field
/// from launching points to infinity; combined with damping it also gives the
/// motion a terminal velocity, which reads as the medium having a viscosity.
pub struct FieldBehavior {
    field: CompositeField,
    /// Velocity decay rate (1/s). `0.0` is frictionless.
    pub damping: f32,
    /// Hard cap on per-particle speed (world units / s).
    pub max_speed: f32,
    time: f32,
}

impl FieldBehavior {
    pub fn new(field: CompositeField, damping: f32, max_speed: f32) -> Self {
        Self {
            field,
            damping: damping.max(0.0),
            max_speed: max_speed.max(0.0),
            time: 0.0,
        }
    }

    /// The default free-space "nebula": a gentle spring toward the entity
    /// centre so the cloud holds together, broad divergence-free circulation,
    /// finer turbulent eddies on top, and a slow wander so it never reads as
    /// looping. Deterministic from `seed`.
    pub fn nebula(seed: u32) -> Self {
        let field = CompositeField::new()
            .push(Box::new(Attractor {
                target: Vec3::ZERO,
                strength: 1.6,
            }))
            .push(Box::new(Curl::new(seed, 0.8, 0.55, 0.35)))
            .push(Box::new(Turbulence::new(
                seed.wrapping_add(101),
                1.6,
                0.22,
                3,
                0.5,
            )))
            .push(Box::new(Drift::new(seed.wrapping_add(202), 0.3, 0.18, 0.2)));
        Self::new(field, 0.9, 1.6)
    }

    /// A morphing free-space cloud: an SDF attractor toward `target` (its pull
    /// scaled by the entity's focus/confidence — M5) with broad circulation and
    /// a slow wander so it lives on the shape rather than freezing onto it. At
    /// rest (zero focus) the attractor's coherence floor keeps it a loose,
    /// suggestive version of the shape; focus condenses it. Deterministic from
    /// `seed`.
    pub fn morph(seed: u32, target: MorphTarget) -> Self {
        let field = CompositeField::new()
            .push(Box::new(SdfAttractor::new(target, 6.0)))
            .push(Box::new(Curl::new(seed, 0.8, 0.35, 0.3)))
            .push(Box::new(Drift::new(seed.wrapping_add(202), 0.3, 0.15, 0.2)));
        Self::new(field, 1.4, 2.4)
    }
}

impl PointBehavior for FieldBehavior {
    fn update(
        &mut self,
        particles: &mut [Particle],
        dt: f32,
        params: &EntityParams,
        signals: &PresenceSignals,
    ) {
        if dt <= 0.0 {
            return;
        }
        self.time += dt * params.time_scale;
        let damp = (-self.damping * dt).exp();
        for particle in particles.iter_mut() {
            let sample = FieldSample {
                position: particle.position,
                velocity: particle.velocity,
                time: self.time,
                params,
                signals,
            };
            let accel = self.field.force(&sample);
            let mut velocity = (particle.velocity + accel * dt) * damp;
            let speed = velocity.length();
            if speed > self.max_speed && speed > 0.0 {
                velocity *= self.max_speed / speed;
            }
            particle.velocity = velocity;
            particle.position += velocity * dt;
        }
    }
}

/// Seeds a free-space population uniformly through a ball around the entity
/// centre. This is the volume fill ADR-011 forbids for the *shell* and ADR-014
/// permits here: a free-space entity has no surface to seed onto, and a hollow
/// shell of points would read as exactly the skin this entity is meant not to
/// be. Deterministic from `seed` — the same seed and count reproduce the same
/// cloud, which the M8 GPU port and the visual-regression tests both rely on.
pub struct FieldCloudGenerator {
    pub seed: u32,
    /// Ball radius in the entity's local units, before `EntityParams::scale`.
    pub radius: f32,
}

impl FieldCloudGenerator {
    pub fn new(seed: u32, radius: f32) -> Self {
        Self { seed, radius }
    }
}

impl crate::sim::PointGenerator for FieldCloudGenerator {
    fn generate(&self, count: usize, params: &EntityParams) -> Vec<Particle> {
        let seed = self.seed as f32;
        (0..count)
            .map(|i| {
                let fi = i as f32;
                // Three decorrelated unit hashes per point.
                let u = hash01(Vec3::new(fi * 1.13 + seed, 2.3, 7.7));
                let v = hash01(Vec3::new(fi * 0.71, seed + 3.1, 1.9));
                let w = hash01(Vec3::new(fi * 1.77 + 2.0, 5.5, seed * 0.37 + 4.2));

                // Uniform direction on the sphere.
                let theta = std::f32::consts::TAU * u;
                let z = 2.0 * v - 1.0;
                let r_xy = (1.0 - z * z).max(0.0).sqrt();
                let dir = Vec3::new(r_xy * theta.cos(), r_xy * theta.sin(), z);

                // Cube-root radius → uniform density through the ball (not
                // clustered at the centre, which a linear radius would give).
                let frac = w.cbrt();
                let radius = self.radius * frac;
                let position = params.center + dir * radius * params.scale;

                // Layer by depth: the dense, coherent core inward; the free
                // halo at the rim. Matches how the shapes tag surface points so
                // the shader's per-layer material reads the same on both.
                let layer = if frac < 0.45 {
                    Layer::Core
                } else if frac < 0.8 {
                    Layer::Body
                } else {
                    Layer::Halo
                };

                Particle {
                    position,
                    base_offset: dir,
                    layer,
                    size: 1.0,
                    brightness: params.intensity.clamp(0.0, 1.5),
                    color_bias: params.cool.clamp(0.0, 1.0),
                    ..Default::default()
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::PointGenerator;

    fn sample_at(time: f32) -> (EntityParams, PresenceSignals) {
        let mut params = EntityParams::new(Vec3::ZERO, 1.0);
        params.time = time;
        (params, PresenceSignals::default())
    }

    fn force_of(field: &dyn ForceField, position: Vec3) -> Vec3 {
        let (params, signals) = sample_at(0.0);
        field.force(&FieldSample {
            position,
            velocity: Vec3::ZERO,
            time: 0.0,
            params: &params,
            signals: &signals,
        })
    }

    #[test]
    fn attractor_points_toward_its_target() {
        let attractor = Attractor {
            target: Vec3::ZERO,
            strength: 2.0,
        };
        let f = force_of(&attractor, Vec3::new(3.0, 0.0, 0.0));
        // Pull is toward the origin (negative x) and scales with distance.
        assert!(f.x < 0.0);
        assert_eq!(f.y, 0.0);
        assert_eq!(f.z, 0.0);
        // Twice as far → twice the pull (linear spring).
        let near = force_of(&attractor, Vec3::new(1.0, 0.0, 0.0));
        assert!((f.x - near.x * 3.0).abs() < 1e-5);
    }

    #[test]
    fn curl_force_is_deterministic_and_finite() {
        let curl = Curl::new(7, 0.8, 0.5, 0.3);
        let a = force_of(&curl, Vec3::new(0.3, 0.6, -0.2));
        let b = force_of(&curl, Vec3::new(0.3, 0.6, -0.2));
        assert_eq!(a, b);
        assert!(a.is_finite());
    }

    #[test]
    fn composite_is_the_sum_of_its_fields() {
        let a = Attractor {
            target: Vec3::new(1.0, 0.0, 0.0),
            strength: 1.0,
        };
        let b = Attractor {
            target: Vec3::new(0.0, 2.0, 0.0),
            strength: 1.0,
        };
        let p = Vec3::new(0.5, 0.5, 0.0);
        let expected = force_of(&a, p) + force_of(&b, p);
        let composite = CompositeField::new()
            .push(Box::new(Attractor {
                target: Vec3::new(1.0, 0.0, 0.0),
                strength: 1.0,
            }))
            .push(Box::new(Attractor {
                target: Vec3::new(0.0, 2.0, 0.0),
                strength: 1.0,
            }));
        let got = force_of(&composite, p);
        assert!((got - expected).length() < 1e-6);
    }

    #[test]
    fn damping_bleeds_speed_toward_zero_without_forces() {
        // Empty field: nothing accelerates, damping is the only effect.
        let mut behavior = FieldBehavior::new(CompositeField::new(), 2.0, 100.0);
        let (params, signals) = sample_at(0.0);
        let mut particles = vec![Particle {
            position: Vec3::ZERO,
            velocity: Vec3::new(1.0, 0.0, 0.0),
            ..Default::default()
        }];
        let start = particles[0].velocity.length();
        for _ in 0..120 {
            behavior.update(&mut particles, 1.0 / 60.0, &params, &signals);
        }
        let end = particles[0].velocity.length();
        assert!(
            end < start * 0.2,
            "damping did not bleed speed: {start} -> {end}"
        );
    }

    #[test]
    fn speed_is_capped_by_max_speed() {
        // A very stiff spring far from target would otherwise blow the speed up.
        let field = CompositeField::new().push(Box::new(Attractor {
            target: Vec3::ZERO,
            strength: 50.0,
        }));
        let mut behavior = FieldBehavior::new(field, 0.0, 1.5);
        let (params, signals) = sample_at(0.0);
        let mut particles = vec![Particle {
            position: Vec3::new(10.0, 0.0, 0.0),
            velocity: Vec3::ZERO,
            ..Default::default()
        }];
        for _ in 0..240 {
            behavior.update(&mut particles, 1.0 / 60.0, &params, &signals);
            assert!(
                particles[0].velocity.length() <= 1.5 + 1e-4,
                "speed exceeded the cap: {}",
                particles[0].velocity.length()
            );
        }
    }

    #[test]
    fn field_behavior_is_deterministic() {
        let (params, signals) = sample_at(0.0);
        let run = || {
            let mut behavior = FieldBehavior::nebula(0xC0FFEE);
            let gen = FieldCloudGenerator::new(0xC0FFEE, 1.0);
            let mut particles = gen.generate(200, &params);
            for _ in 0..180 {
                behavior.update(&mut particles, 1.0 / 60.0, &params, &signals);
            }
            particles
        };
        let a = run();
        let b = run();
        for (pa, pb) in a.iter().zip(b.iter()) {
            assert_eq!(pa.position, pb.position);
            assert_eq!(pa.velocity, pb.velocity);
        }
    }

    #[test]
    fn nebula_stays_bounded_under_a_long_run() {
        // The whole point of spring + damping + clamp: the cloud does not fly
        // apart or collapse. After a long run every point is still within a
        // sane radius of the centre.
        let (params, signals) = sample_at(0.0);
        let mut behavior = FieldBehavior::nebula(1);
        let gen = FieldCloudGenerator::new(1, 1.0);
        let mut particles = gen.generate(300, &params);
        for _ in 0..600 {
            behavior.update(&mut particles, 1.0 / 60.0, &params, &signals);
        }
        for p in &particles {
            assert!(p.position.is_finite());
            assert!(
                p.position.length() < 6.0,
                "a nebula particle escaped: {}",
                p.position.length()
            );
        }
    }

    #[test]
    fn generator_fills_a_bounded_ball_deterministically() {
        let mut params = EntityParams::new(Vec3::new(2.0, 0.0, 0.0), 1.5);
        params.intensity = 0.4;
        let gen = FieldCloudGenerator::new(42, 1.0);
        let a = gen.generate(500, &params);
        let b = gen.generate(500, &params);
        assert_eq!(a.len(), 500);
        for (pa, pb) in a.iter().zip(b.iter()) {
            assert_eq!(pa.position, pb.position);
        }
        // Every point sits inside radius*scale of the centre.
        for p in &a {
            let r = (p.position - params.center).length();
            assert!(r <= 1.0 * 1.5 + 1e-4, "point outside the ball: {r}");
        }
        // And the fill is a volume, not a shell: some points are well inside.
        let inner = a
            .iter()
            .filter(|p| (p.position - params.center).length() < 0.5 * 1.5)
            .count();
        assert!(inner > 0, "generator produced a hollow shell, not a ball");
    }
}
