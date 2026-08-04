//! One body, two substrates — the behavior that makes ADR-015's single
//! continuous presence possible.
//!
//! # The problem this solves
//!
//! The engine has two ways to move a particle, and they are both right. The
//! surface spring (ADR-011) pulls points onto a scanned skin, which is what
//! makes the droplet read as a *thing* with a silhouette. The force field
//! (ADR-014) carries points wherever the flow takes them, which is what makes
//! a nebula or an SDF morph read as *substance*. Before ADR-015 these were
//! different entities with different behaviors, so going from one to the other
//! meant cross-fading two particle populations — and a cross-fade is exactly
//! the moment the illusion breaks, because for half a second the presence is
//! two half-drawn things instead of one thing changing.
//!
//! [`MorphBehavior`] removes the choice. It runs both substrates over the
//! *same* particles and blends their **accelerations**, weighted by
//! [`FormWeights`]. Because the blend happens before integration there is only
//! ever one position per particle, so there is no cross-fade to see: at
//! `droplet = 1` the body is the shell, at `ring = 1` it is a torus of
//! circulating points, and at `0.5/0.5` it is one continuous population being
//! pulled by both — a state that is not a transition artifact but a legitimate
//! shape of its own.
//!
//! # Why the droplet path is a short-circuit and not a special case
//!
//! With no field weight at all, `update` hands the step to
//! [`SurfaceBehavior`] verbatim. That is not an optimization bolted on
//! afterwards; it is the guarantee that adopting this behavior cannot change
//! how the resting presence looks or costs. `the_resting_body_is_bit_identical_
//! to_the_surface_behavior` holds it to that, byte for byte.
//!
//! # Blending forces, not results
//!
//! Damping and the speed clamp are blended too, rather than picked from
//! whichever substrate is winning. A hard switch at some threshold would put a
//! visible discontinuity in the middle of every morph — the one place a viewer
//! is already looking.

use rayon::prelude::*;

use crate::sim::field::{
    CompositeField, Drift, FieldSample, ForceField, MorphTarget, SdfAttractor, VolumetricCloud,
};
use crate::sim::form::{FormTarget, FormWeights};
use crate::sim::shapes::{SurfaceBehavior, SurfaceShape};
use crate::sim::types::{hash01, EntityParams, Particle, PresenceSignals};
use crate::sim::{flush_to_rest, PointBehavior};

/// Below this a weight contributes nothing and is skipped outright.
///
/// The skip is what keeps an open-ended vocabulary affordable: an unengaged
/// form costs one comparison per particle, not an SDF evaluation, so forms can
/// be added without every one of them taxing the resting state.
const FORM_EPSILON: f32 = 1e-4;

/// Steps between refreshes of a given particle's cached ambient field force.
///
/// Wider than the surface's [`DEFAULT_DEFORM_STRIDE`](crate::sim::shapes::DEFAULT_DEFORM_STRIDE)
/// and deliberately so: the ambient drift runs at spatial frequency 0.35
/// evolving at 0.25× real time, which is slower than the fold deformation the
/// deform stride already hides. Measured at the shell's real 80k budget,
/// refreshing every particle every step cost 15 ms more per step than a stride
/// of 16, for motion no one can see the difference in.
///
/// The stagger is strided rather than blocked for the same reason the deform
/// refresh is: generation walks the surface in seed order, so a contiguous
/// block would update one spatial region at a time and the boundary would be
/// visible as a seam sweeping across the body.
///
/// The quality tier overrides this — see
/// [`QualityTier::field_stride`](crate::scene::QualityTier::field_stride).
pub const DEFAULT_FIELD_STRIDE: usize = 16;

/// Speed clamp at the pure-surface end of the blend, in world units per second.
///
/// The surface spring is critically damped and self-limiting, so it does not
/// need a clamp; this exists only so the cap has something finite to interpolate
/// *from* as field weight rises. It is set far above any speed the spring
/// actually reaches, which is what makes it a runaway rail rather than a
/// governor on the shell's motion.
const SURFACE_SPEED_CAP: f32 = 24.0;

/// Blends the surface spring and the force field over one particle set,
/// weighted by [`EntityParams::form`].
pub struct MorphBehavior<S: SurfaceShape> {
    /// The droplet substrate. Public so a caller can tune the skin spring, and
    /// so the quality tier's deform stride reaches it.
    pub surface: SurfaceBehavior<S>,
    /// Forces that act on the field share regardless of which shape is held —
    /// the circulation and wander that keep a morph *alive* on its target
    /// instead of frozen onto it.
    ambient: CompositeField,
    /// The field that draws the body into each non-surface target.
    ///
    /// Boxed as a general [`ForceField`] rather than typed as an
    /// [`SdfAttractor`] because not every shape is a surface to land on — a
    /// nebula is a volume to fill, and needs a different force entirely. A
    /// `Vec` rather than a fixed array so the vocabulary stays open-ended
    /// (ADR-015 decision 4); it is walked, never allocated, on the hot path.
    forms: Vec<(FormTarget, Box<dyn ForceField>)>,
    /// Velocity decay rate at the pure-field end (1/s).
    pub damping: f32,
    /// Speed clamp at the pure-field end (world units / s).
    pub max_speed: f32,
    /// Steps between refreshes of a particle's cached ambient force. See
    /// [`DEFAULT_FIELD_STRIDE`].
    pub field_stride: usize,
    /// Which stride class refreshes its cached ambient force this step.
    field_phase: usize,
    /// The behavior's own time-scaled clock, so the noise fields evolve
    /// coherently across the whole set.
    time: f32,
}

impl<S: SurfaceShape> MorphBehavior<S> {
    /// The standard vocabulary: the shell as the droplet, three signed-distance
    /// targets, and a nebula. Deterministic from `seed`.
    pub fn new(shape: S, seed: u32) -> Self {
        Self {
            surface: SurfaceBehavior::new(shape),
            // Drift only, deliberately — no curl.
            //
            // `FieldBehavior::nebula` uses divergence-free curl because a
            // free-standing cloud has nothing else holding it: a field that
            // pumped points outward would disperse it. A morphing body is not
            // in that position. It is pinned by its SDF attractor and, at any
            // partial weight, by the skin spring as well, so what the ambient
            // owes it is *wander* — proof it is alive on the shape rather than
            // frozen onto it — and not source-free circulation.
            //
            // The cost difference is the whole reason this distinction is worth
            // drawing. A curl is eighteen taps of 4D simplex to the drift's
            // three, and at the shell's 80k points that one field was 15 ms of
            // every step. Buying confinement we do not need, at seven times the
            // price, on the largest population in the engine.
            ambient: CompositeField::new().push(Box::new(Drift::new(
                seed.wrapping_add(202),
                0.35,
                0.22,
                0.25,
            ))),
            forms: default_vocabulary(),
            damping: 1.4,
            max_speed: 2.4,
            field_stride: DEFAULT_FIELD_STRIDE,
            field_phase: 0,
            time: 0.0,
        }
    }

    /// Replaces the field for one target, or adds it if the vocabulary does not
    /// have it yet. This is how a scene gets a ring of a different radius, or a
    /// wholly new shape, without a new behavior type.
    pub fn set_form_field(&mut self, form: FormTarget, field: Box<dyn ForceField>) {
        debug_assert!(
            !form.is_surface(),
            "{} is carried by the surface spring and has no field",
            form.label()
        );
        match self.forms.iter_mut().find(|(t, _)| *t == form) {
            Some(slot) => slot.1 = field,
            None => self.forms.push((form, field)),
        }
    }

    /// Sum of the field accelerations acting on one sample: the (cached)
    /// ambient flow scaled by the field share, plus each engaged target's
    /// attractor at its own weight.
    ///
    /// The weights are already normalized across *all* targets, so they sum to
    /// the field share and need no second scaling — only the ambient forces,
    /// which belong to no particular shape, are scaled by `field_weight`.
    ///
    /// The form fields are evaluated every step while the ambient flow is not.
    /// That asymmetry is the point: a form field is a spring, and a spring fed
    /// a stale position oscillates, whereas the ambient flow is a slow drift
    /// that no viewer can tell is sixteen steps behind. The form fields are
    /// also pure geometry — no noise — so they are the cheap half.
    fn field_accel(
        &self,
        ambient: glam::Vec3,
        sample: &FieldSample,
        weights: &FormWeights,
        field_weight: f32,
    ) -> glam::Vec3 {
        let mut accel = ambient * field_weight;
        for (target, field) in &self.forms {
            let w = weights.get(*target);
            if w > FORM_EPSILON {
                accel += field.force(sample) * w;
            }
        }
        accel
    }
}

/// Sphere, ring, and helix pull at equal strength so that a blend between two
/// of them lands halfway rather than being dominated by whichever was tuned
/// hardest. The nebula is not a surface at all and is the exception.
fn default_vocabulary() -> Vec<(FormTarget, Box<dyn ForceField>)> {
    const PULL: f32 = 6.0;
    let shaped = |form: FormTarget, strength: f32| {
        let target = form_geometry(form).expect("form has a target shape");
        (
            form,
            Box::new(SdfAttractor::new(target, strength)) as Box<dyn ForceField>,
        )
    };
    vec![
        shaped(FormTarget::Sphere, PULL),
        shaped(FormTarget::Ring, PULL),
        shaped(FormTarget::Helix, PULL),
        // Parametric, so its stations carry the whole pull and it arrives
        // crisper than the implicit shapes do — the same way the droplet's skin
        // does. Pulled a little harder than they are because a heart is only
        // legible while its cleft and point are held sharply, and a soft one
        // reads as a lopsided ball.
        shaped(FormTarget::Heart, PULL * 1.35),
        (
            FormTarget::Nebula,
            // Wider than the sphere and, crucially, *filled*: every particle
            // gets its own radius, so this is a body of substance rather than
            // another skin. It also pulls far more gently than the surfaces do
            // — a nebula that snapped to its radii would read as a solid ball,
            // and what makes it a nebula is that the ambient drift is allowed
            // to win locally.
            Box::new(VolumetricCloud {
                inner: 0.15,
                outer: 1.55,
                strength: 1.8,
            }) as _,
        ),
    ]
}

/// The shape each form holds, or `None` for the two that are not targets at
/// all — the droplet is the surface spring's own skin and the nebula is free
/// space.
///
/// One source of truth, so that anything measuring a form measures the geometry
/// the engine actually pulls toward rather than a copy of it that can drift.
pub(crate) fn form_geometry(form: FormTarget) -> Option<MorphTarget> {
    match form {
        FormTarget::Sphere => Some(MorphTarget::Sphere { radius: 1.0 }),
        FormTarget::Ring => Some(MorphTarget::Ring {
            major: 1.05,
            minor: 0.28,
        }),
        FormTarget::Helix => Some(MorphTarget::Helix {
            radius: 0.75,
            pitch: 0.34,
            thickness: 0.16,
        }),
        FormTarget::Heart => Some(MorphTarget::Heart {
            size: 1.15,
            depth: 0.42,
        }),
        FormTarget::Droplet | FormTarget::Nebula => None,
    }
}

impl<S: SurfaceShape> PointBehavior for MorphBehavior<S> {
    fn set_deform_stride(&mut self, stride: usize) {
        self.surface.set_deform_stride(stride);
    }

    fn update(
        &mut self,
        particles: &mut [Particle],
        dt: f32,
        params: &EntityParams,
        signals: &PresenceSignals,
    ) {
        let weights = params.form.normalized();
        let field_weight = weights.field_weight();

        // Pure droplet: the field substrate is not merely weighted to zero, it
        // is not run at all. See the module docs — this is the preservation
        // guarantee, not a fast path.
        if field_weight <= FORM_EPSILON {
            self.surface.update(particles, dt, params, signals);
            return;
        }
        if dt <= 0.0 {
            return;
        }
        let surface_weight = 1.0 - field_weight;

        let (params, energy, voice) = SurfaceBehavior::<S>::resolve(params, signals);
        let (frame, stride, phase) = self.surface.begin_step(&params);
        self.time += dt * params.time_scale;

        // Both substrates' integrators, interpolated: the skin spring's
        // per-frame retention and the field's frame-rate-independent decay.
        let damp = self.surface.damp_factor(dt) * surface_weight
            + (-self.damping * dt).exp() * field_weight;
        let speed_cap = SURFACE_SPEED_CAP * surface_weight + self.max_speed * field_weight;

        let field_stride = self.field_stride.max(1);
        let field_phase = self.field_phase % field_stride;
        self.field_phase = (self.field_phase + 1) % field_stride;

        // Once the body has fully left the droplet the skin contributes nothing
        // to its motion, and evaluating the shape for every particle to
        // multiply the result by zero is the most expensive no-op available:
        // it is the entire resting cost of the engine.
        let skin_is_live = surface_weight > FORM_EPSILON;

        // The normal is a property of having a skin, so it carries the skin's
        // authority in its length rather than freezing at whatever the particle
        // held on its way out — a held normal lights a surface that is no
        // longer there, and hands it all back the moment the skin re-engages.
        //
        // Scaled by the weight rather than eased on its own clock so that it is
        // in step with the morph by construction, and so that it passes through
        // the shader's zero-normal cutoff exactly when the skin stops
        // contributing. An independent time constant would cross that cutoff on
        // its own schedule, which is a brightness step.

        // Parallel for the same reason the surface behavior is: each particle
        // reads only itself. See `SurfaceBehavior::update`.
        particles.par_iter_mut().enumerate().for_each(|(i, p)| {
            let skin = if skin_is_live {
                Some(
                    self.surface
                        .skin_target(p, i, stride, phase, &frame, &params),
                )
            } else {
                None
            };

            let sample = FieldSample {
                position: p.position,
                velocity: p.velocity,
                seed01: hash01(p.base_offset),
                time: self.time,
                params: &params,
                signals,
            };
            if i % field_stride == field_phase {
                p.field_force = self.ambient.force(&sample);
            }

            let mut accel = self.field_accel(p.field_force, &sample, &weights, field_weight);
            if let Some(skin) = skin {
                accel += self.surface.spring_accel(p, skin.position) * surface_weight;
                p.normal = skin.normal * surface_weight;
            } else {
                p.normal = glam::Vec3::ZERO;
            }

            let mut velocity = (p.velocity + accel * dt) * damp;
            let speed = velocity.length();
            if speed > speed_cap {
                velocity *= speed_cap / speed;
            }
            p.velocity = flush_to_rest(velocity);
            p.position += velocity * dt;

            // Material is a property of the layer, not of which substrate is
            // moving the point, so it is assigned exactly as the surface
            // behavior would. A morphing body keeps its skin's palette.
            p.brightness =
                SurfaceBehavior::<S>::layer_brightness(p.layer, energy, voice) * params.presence;
            p.color_bias = params.cool.clamp(0.0, 1.0);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::field::sdf::station_seeds;
    use crate::sim::form::{FormTarget, FormTransition};
    use crate::sim::shapes::{PresenceShell, SurfaceGenerator};
    use crate::sim::PointGenerator;
    use glam::Vec3;

    const SEED: u32 = 0x1DEE;
    const COUNT: usize = 400;

    fn params_with(form: FormWeights) -> EntityParams {
        let mut params = EntityParams::new(Vec3::ZERO, 1.0);
        params.form = form;
        params.focus = 0.8;
        params.confidence = 0.8;
        params
    }

    fn particles(params: &EntityParams) -> Vec<Particle> {
        let shell = PresenceShell::new(SEED);
        SurfaceGenerator::new(shell.domain()).generate(COUNT, params)
    }

    fn run<B: PointBehavior>(
        behavior: &mut B,
        params: &EntityParams,
        steps: usize,
    ) -> Vec<Particle> {
        let signals = PresenceSignals::default();
        let mut set = particles(params);
        let mut params = *params;
        for _ in 0..steps {
            params.time += 1.0 / 60.0;
            behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
        }
        set
    }

    /// The whole point of the short-circuit: adopting `MorphBehavior` may not
    /// change the resting presence by a single bit.
    #[test]
    fn the_resting_body_is_bit_identical_to_the_surface_behavior() {
        let params = params_with(FormWeights::default());

        let mut surface = SurfaceBehavior::new(PresenceShell::new(SEED));
        let mut morph = MorphBehavior::new(PresenceShell::new(SEED), SEED);

        let expected = run(&mut surface, &params, 180);
        let actual = run(&mut morph, &params, 180);

        for (a, b) in expected.iter().zip(actual.iter()) {
            assert_eq!(a.position, b.position, "the resting body drifted");
            assert_eq!(a.velocity, b.velocity);
            assert_eq!(a.brightness, b.brightness);
            assert_eq!(a.crease, b.crease);
        }
    }

    /// A weight that rounds to nothing must not quietly engage the field —
    /// otherwise "droplet" in practice would mean "droplet plus a little
    /// noise", and the resting cost would creep.
    #[test]
    fn a_negligible_field_weight_still_takes_the_surface_path() {
        let mut form = FormWeights::default();
        form.set(FormTarget::Ring, FORM_EPSILON * 0.1);
        let params = params_with(form);

        let mut surface = SurfaceBehavior::new(PresenceShell::new(SEED));
        let mut morph = MorphBehavior::new(PresenceShell::new(SEED), SEED);

        let expected = run(&mut surface, &params, 60);
        let actual = run(&mut morph, &params, 60);
        for (a, b) in expected.iter().zip(actual.iter()) {
            assert_eq!(a.position, b.position);
        }
    }

    fn mean_abs_sdf(particles: &[Particle], target: MorphTarget, params: &EntityParams) -> f32 {
        let scale = params.scale.max(1e-4);
        let sum: f32 = particles
            .iter()
            .map(|p| target.sdf((p.position - params.center) / scale).abs())
            .sum();
        sum / particles.len() as f32
    }

    /// Measured against the resting droplet rather than against the starting
    /// positions: the shell is already a rough unit sphere, so "did it get
    /// closer than where it began" is trivially true for some targets and
    /// meaningless. "Did holding this form put the body on the shape, where
    /// resting would not have" is the claim worth guarding.
    #[test]
    fn a_fully_weighted_form_pulls_the_body_onto_that_shape() {
        for (form, target) in [
            (
                FormTarget::Ring,
                MorphTarget::Ring {
                    major: 1.05,
                    minor: 0.28,
                },
            ),
            (
                FormTarget::Helix,
                MorphTarget::Helix {
                    radius: 0.75,
                    pitch: 0.34,
                    thickness: 0.16,
                },
            ),
        ] {
            let params = params_with(FormWeights::single(form));
            let held = run(
                &mut MorphBehavior::new(PresenceShell::new(SEED), SEED),
                &params,
                600,
            );

            let resting_params = params_with(FormWeights::default());
            let resting = run(
                &mut MorphBehavior::new(PresenceShell::new(SEED), SEED),
                &resting_params,
                600,
            );

            let held_err = mean_abs_sdf(&held, target, &params);
            let resting_err = mean_abs_sdf(&resting, target, &resting_params);
            assert!(
                held_err < resting_err * 0.5,
                "{} never coalesced: resting {resting_err}, held {held_err}",
                form.label()
            );
        }
    }

    /// The state that justifies the whole design: a half-and-half body is one
    /// population sitting between the two shapes, not a copy of either.
    #[test]
    fn a_blended_form_settles_between_the_two_shapes() {
        let ring = MorphTarget::Ring {
            major: 1.05,
            minor: 0.28,
        };

        let mut half = FormWeights::zeroed();
        half.set(FormTarget::Droplet, 0.5);
        half.set(FormTarget::Ring, 0.5);

        let blended_params = params_with(half);
        let ring_params = params_with(FormWeights::single(FormTarget::Ring));
        let droplet_params = params_with(FormWeights::default());

        let blended = run(
            &mut MorphBehavior::new(PresenceShell::new(SEED), SEED),
            &blended_params,
            600,
        );
        let pure_ring = run(
            &mut MorphBehavior::new(PresenceShell::new(SEED), SEED),
            &ring_params,
            600,
        );
        let droplet = run(
            &mut MorphBehavior::new(PresenceShell::new(SEED), SEED),
            &droplet_params,
            600,
        );

        let blended_err = mean_abs_sdf(&blended, ring, &blended_params);
        let ring_err = mean_abs_sdf(&pure_ring, ring, &ring_params);
        let droplet_err = mean_abs_sdf(&droplet, ring, &droplet_params);

        assert!(
            blended_err > ring_err && blended_err < droplet_err,
            "the blend did not land between the shapes: ring {ring_err}, blend {blended_err}, droplet {droplet_err}"
        );
    }

    /// The nebula used to be a weak sphere attractor, which meant it settled
    /// onto a surface and was indistinguishable from the sphere on screen.
    /// A nebula has to have an inside.
    #[test]
    fn the_nebula_is_a_volume_where_the_sphere_is_a_shell() {
        let radial_spread = |form: FormTarget| {
            let params = params_with(FormWeights::single(form));
            let settled = run(
                &mut MorphBehavior::new(PresenceShell::new(SEED), SEED),
                &params,
                900,
            );
            let mut radii: Vec<f32> = settled
                .iter()
                .map(|p| (p.position - params.center).length() / params.scale)
                .collect();
            radii.sort_by(|a, b| a.partial_cmp(b).unwrap());
            // Interquartile range: robust to the few stragglers still in
            // transit, unlike min/max.
            radii[radii.len() * 3 / 4] - radii[radii.len() / 4]
        };

        let sphere = radial_spread(FormTarget::Sphere);
        let nebula = radial_spread(FormTarget::Nebula);
        assert!(
            nebula > sphere * 3.0,
            "the nebula is as thin as the sphere shell: nebula {nebula}, sphere {sphere}"
        );
    }

    /// Every shape has to be reachable from every other one. Going out to a
    /// form and only being able to come back to the droplet would make the
    /// vocabulary a set of dead ends.
    #[test]
    fn a_held_form_can_morph_straight_into_another_one() {
        let ring_target = MorphTarget::Ring {
            major: 1.05,
            minor: 0.28,
        };
        let signals = PresenceSignals::default();
        let mut params = params_with(FormWeights::single(FormTarget::Helix));
        let mut behavior = MorphBehavior::new(PresenceShell::new(SEED), SEED);
        let mut set = particles(&params);

        // Hold the helix long enough to actually settle into it.
        for _ in 0..1_200 {
            params.time += 1.0 / 60.0;
            behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
        }

        // Straight to the ring, without passing through the droplet.
        params.form = FormWeights::single(FormTarget::Ring);
        for _ in 0..1_200 {
            params.time += 1.0 / 60.0;
            behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
        }

        let err = mean_abs_sdf(&set, ring_target, &params);
        assert!(
            err < 0.25,
            "the body never reached the ring from the helix: mean |sdf| {err}"
        );
    }

    /// The helix winds around the y axis, and nothing in its pull acts along
    /// that axis — it attracts a particle to the curve *at the particle's own
    /// height*. Left that way the ambient drift random-walks the population up
    /// and down forever, so the body slowly stretches into an ever-longer
    /// spiral and takes longer and longer to reach anywhere else.
    /// Reaching a shape must not depend on which shape you started from. If
    /// leaving the helix takes many times longer than leaving the droplet, the
    /// morph reads as ignored no matter what the weights say.
    #[test]
    fn leaving_one_form_for_another_takes_about_as_long_whatever_it_started_as() {
        let ring = MorphTarget::Ring {
            major: 1.05,
            minor: 0.28,
        };
        let signals = PresenceSignals::default();

        let settle_time = |from: FormTarget| {
            let mut params = params_with(FormWeights::single(from));
            let mut behavior = MorphBehavior::new(PresenceShell::new(SEED), SEED);
            let mut set = particles(&params);
            for _ in 0..900 {
                params.time += 1.0 / 60.0;
                behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
            }

            let mut transition = FormTransition::new(FormWeights::single(from));
            transition.set_target(FormWeights::single(FormTarget::Ring), 1.5);
            let mut reached = None;
            for step in 0..1_800 {
                params.form = transition.tick(1.0 / 60.0);
                params.time += 1.0 / 60.0;
                behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
                if reached.is_none() && mean_abs_sdf(&set, ring, &params) < 0.12 {
                    reached = Some(step as f32 / 60.0);
                }
            }
            (reached, mean_abs_sdf(&set, ring, &params))
        };

        let (from_droplet, droplet_err) = settle_time(FormTarget::Droplet);
        let (from_helix, helix_err) = settle_time(FormTarget::Helix);

        let baseline = from_droplet.unwrap_or_else(|| {
            panic!("the droplet never reached the ring at all: mean |sdf| {droplet_err}")
        });
        let helix = from_helix.unwrap_or_else(|| {
            panic!("the helix never reached the ring in 30s: mean |sdf| {helix_err}")
        });
        assert!(
            helix < baseline * 3.0 + 1.0,
            "leaving the helix took {helix}s against the droplet's {baseline}s"
        );
    }

    /// A shape has to look like itself whatever it was a moment ago.
    ///
    /// Distance to the surface does not capture this: a body can sit exactly on
    /// a ring and still be two clumps rather than a ring, because a signed
    /// distance says nothing about coverage. The helix winds around one axis,
    /// so it is the starting shape that exposes the difference.
    #[test]
    fn a_form_covers_itself_evenly_whatever_it_morphed_from() {
        const BINS: usize = 16;
        let signals = PresenceSignals::default();

        let bin_of = |p: Vec3, centre: Vec3| {
            let d = p - centre;
            let angle = d.y.atan2(d.x).rem_euclid(std::f32::consts::TAU);
            ((angle / std::f32::consts::TAU * BINS as f32) as usize).min(BINS - 1)
        };

        // What the shape itself says its coverage should be. Measuring against
        // a flat distribution instead would be wrong for anything that is not
        // radially symmetric: the heart's stations are deliberately denser at
        // the cleft and the point, which are the features that make it
        // readable. The question is whether the body covers the shape the way
        // the shape asks, not whether every direction gets equal share.
        let reference = |to: FormTarget| {
            let mut bins = [0usize; BINS];
            if let Some(target) = form_geometry(to) {
                for i in 0..20_000 {
                    let seed = i as f32 / 20_000.0;
                    let (a, b) = station_seeds(seed);
                    bins[bin_of(target.surface_point(a, b), Vec3::ZERO)] += 1;
                }
            }
            bins
        };

        // The settled body's coverage, as a share per slice.
        let settled = |from: FormTarget, to: FormTarget| {
            let mut params = params_with(FormWeights::single(from));
            let mut behavior = MorphBehavior::new(PresenceShell::new(SEED), SEED);
            let mut set = particles(&params);
            for _ in 0..900 {
                params.time += 1.0 / 60.0;
                behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
            }

            params.form = FormWeights::single(to);
            for _ in 0..1_500 {
                params.time += 1.0 / 60.0;
                behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
            }

            let mut bins = [0usize; BINS];
            for p in &set {
                bins[bin_of(p.position, params.center)] += 1;
            }
            bins
        };

        let share = |bins: [usize; BINS]| {
            let total: usize = bins.iter().sum();
            bins.map(|c| c as f32 / total.max(1) as f32)
        };

        for to in [FormTarget::Ring, FormTarget::Heart] {
            let want = share(reference(to));
            for from in [FormTarget::Droplet, FormTarget::Helix] {
                let got = share(settled(from, to));
                for slice in 0..BINS {
                    // Every part of the shape that should hold points holds
                    // roughly its share of them, whatever the body was before.
                    // The failure this catches is stark — before targets had
                    // stations, morphing out of the helix left whole slices of
                    // the ring completely empty.
                    let (want, got) = (want[slice], got[slice]);
                    if want < 0.01 {
                        continue;
                    }
                    assert!(
                        got > want * 0.4,
                        "{} -> {} left slice {slice} at {got:.3} of the body \
                         where the shape asks for {want:.3}",
                        from.label(),
                        to.label()
                    );
                }
            }
        }
    }

    /// A settled body must stop, not creep toward zero forever.
    ///
    /// Timing this would be flaky, so the invariant is checked directly: no
    /// particle may sit in the band between "stopped" and "moving", because
    /// that band is where velocities decay into denormals and take the whole
    /// simulation down with them. Both substrates are checked, since each
    /// integrates velocity itself.
    #[test]
    fn a_settled_body_stops_dead_instead_of_creeping_into_denormals() {
        let signals = PresenceSignals::default();

        for form in [FormTarget::Droplet, FormTarget::Ring, FormTarget::Heart] {
            let mut params = params_with(FormWeights::single(form));
            let mut behavior = MorphBehavior::new(PresenceShell::new(SEED), SEED);
            // No ambient drift, so the body is genuinely allowed to come to
            // rest — with drift there is always something to chase.
            behavior.ambient = CompositeField::new();
            let mut set = particles(&params);

            for _ in 0..3_600 {
                params.time += 1.0 / 60.0;
                behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
            }

            let creeping = set
                .iter()
                .filter(|p| {
                    let v = p.velocity.length_squared();
                    v > 0.0 && v < 1e-12
                })
                .count();
            assert_eq!(
                creeping,
                0,
                "{} left {creeping} particles creeping below the rest threshold",
                form.label()
            );
        }
    }

    #[test]
    fn a_long_held_helix_does_not_stretch_without_bound() {
        let signals = PresenceSignals::default();
        let mut params = params_with(FormWeights::single(FormTarget::Helix));
        let mut behavior = MorphBehavior::new(PresenceShell::new(SEED), SEED);
        let mut set = particles(&params);

        let extent = |set: &[Particle]| {
            set.iter()
                .map(|p| (p.position.y - params.center.y).abs())
                .fold(0.0f32, f32::max)
        };

        // A minute of holding the shape.
        for _ in 0..3_600 {
            params.time += 1.0 / 60.0;
            behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
        }
        let after_a_minute = extent(&set);
        assert!(
            after_a_minute < 3.0,
            "the helix stretched to {after_a_minute} along its axis in one minute"
        );
    }

    /// A body in free space has no skin, so it should carry no surface normal.
    /// Holding the last one taken from the droplet lights a shape that is not
    /// there any more, and hands it back all at once on the way home.
    #[test]
    fn the_surface_normal_fades_out_with_the_skin_and_back_in_with_it() {
        let signals = PresenceSignals::default();
        let mut params = params_with(FormWeights::single(FormTarget::Droplet));
        let mut behavior = MorphBehavior::new(PresenceShell::new(SEED), SEED);
        let mut set = particles(&params);

        let mean_normal = |set: &[Particle]| {
            set.iter().map(|p| p.normal.length()).sum::<f32>() / set.len() as f32
        };

        for _ in 0..120 {
            params.time += 1.0 / 60.0;
            behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
        }
        let with_a_skin = mean_normal(&set);
        assert!(
            with_a_skin > 0.9,
            "the droplet has no normals: {with_a_skin}"
        );

        params.form = FormWeights::single(FormTarget::Nebula);
        for _ in 0..240 {
            params.time += 1.0 / 60.0;
            behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
        }
        let in_free_space = mean_normal(&set);
        assert!(
            in_free_space < 0.05,
            "the cloud kept the droplet's normals: {in_free_space}"
        );

        // Half way onto the skin the normal has to be half length, not full
        // length about to switch off: the shader reads the length as how much
        // silhouette to apply, so anything else is a brightness step.
        let mut half = params;
        half.form = {
            let mut w = FormWeights::zeroed();
            w.set(FormTarget::Droplet, 0.5);
            w.set(FormTarget::Nebula, 0.5);
            w
        };
        behavior.update(&mut set, 1.0 / 60.0, &half, &signals);
        let midway = mean_normal(&set);
        assert!(
            (midway - 0.5).abs() < 0.05,
            "the silhouette did not track the skin's share: {midway}"
        );

        params.form = FormWeights::single(FormTarget::Droplet);
        for _ in 0..240 {
            params.time += 1.0 / 60.0;
            behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
        }
        let home_again = mean_normal(&set);
        assert!(
            home_again > 0.9,
            "the returned droplet never got its normals back: {home_again}"
        );
    }

    #[test]
    fn a_morphing_body_stays_bounded() {
        let params = params_with(FormWeights::single(FormTarget::Helix));
        let mut morph = MorphBehavior::new(PresenceShell::new(SEED), SEED);
        let settled = run(&mut morph, &params, 900);

        for p in &settled {
            assert!(p.position.is_finite(), "a particle went non-finite");
            assert!(
                p.position.length() < 20.0,
                "a particle escaped: {:?}",
                p.position
            );
        }
    }

    #[test]
    fn morphing_is_deterministic() {
        let params = params_with(FormWeights::single(FormTarget::Sphere));
        let a = run(
            &mut MorphBehavior::new(PresenceShell::new(SEED), SEED),
            &params,
            120,
        );
        let b = run(
            &mut MorphBehavior::new(PresenceShell::new(SEED), SEED),
            &params,
            120,
        );
        for (pa, pb) in a.iter().zip(b.iter()) {
            assert_eq!(pa.position, pb.position);
        }
    }

    /// Not a correctness test — a measurement, run by hand:
    /// `cargo test --release -- --ignored --nocapture morph_cost`.
    ///
    /// This exists because the morph path once cost 135 ms/step at the shell's
    /// real 80k budget — 18.5× the resting path, which in the running app was
    /// 4 FPS and a catch-up spiral. Two changes fixed it: caching the ambient
    /// force on a stagger, and dropping the ambient curl for a drift. The
    /// numbers to beat, on the reference machine:
    ///
    /// ```text
    /// droplet   7.0 ms/step
    /// ring/s16 11.7 ms/step   (1.7x resting; the floor with no ambient is 11.3)
    /// ```
    ///
    /// The remaining 11.3 ms is the blend itself over 80,000 particles and is
    /// not a caching problem — reducing it means fewer points or a GPU pass.
    #[test]
    #[ignore]
    fn morph_cost_at_the_real_shell_budget() {
        use std::time::Instant;

        const BUDGET: usize = 80_000;
        let signals = PresenceSignals::default();

        let measure = |label: &str, form: FormWeights, field_stride: usize, ambient: bool| {
            let mut params = params_with(form);
            let shell = PresenceShell::new(SEED);
            let mut set = SurfaceGenerator::new(shell.domain()).generate(BUDGET, &params);
            let mut behavior = MorphBehavior::new(PresenceShell::new(SEED), SEED);
            behavior.set_deform_stride(4);
            behavior.field_stride = field_stride;
            if !ambient {
                behavior.ambient = CompositeField::new();
            }

            // Warm up, then time.
            for _ in 0..10 {
                behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
            }
            let steps = 120;
            let start = Instant::now();
            for _ in 0..steps {
                params.time += 1.0 / 60.0;
                behavior.update(&mut set, 1.0 / 60.0, &params, &signals);
            }
            let per_step = start.elapsed().as_secs_f64() * 1000.0 / steps as f64;
            println!(
                "{label:>10}: {per_step:.2} ms/step  ({:.0} steps/s)",
                1000.0 / per_step
            );
            per_step
        };

        let droplet = measure(
            "droplet",
            FormWeights::default(),
            DEFAULT_FIELD_STRIDE,
            true,
        );
        // The worst case, and the one that showed up in the running app:
        // mid-transition, where the skin and the field are both live and
        // neither can be skipped. A *settled* form is cheaper than resting
        // because the skin is skipped outright; a morph in flight is not.
        let mut half = FormWeights::zeroed();
        half.set(FormTarget::Droplet, 0.5);
        half.set(FormTarget::Ring, 0.5);
        let morphing = measure("morphing", half, DEFAULT_FIELD_STRIDE, true);
        println!("           -> {:.1}x resting", morphing / droplet);

        for stride in [8, 16, 32] {
            let ring = measure(
                &format!("ring/s{stride}"),
                FormWeights::single(FormTarget::Ring),
                stride,
                true,
            );
            println!("           -> {:.1}x resting", ring / droplet);
        }
        // Attractors only: isolates the geometry half from the noise half.
        let bare = measure(
            "ring/noamb",
            FormWeights::single(FormTarget::Ring),
            DEFAULT_FIELD_STRIDE,
            false,
        );
        println!("           -> {:.1}x resting", bare / droplet);
    }

    /// The stagger must reach *every* particle within one cycle. A particle
    /// missed by the phase walk would hold one stale force forever, which at
    /// this stride is a point quietly drifting on 16-step-old information.
    #[test]
    fn the_field_refresh_reaches_every_particle_within_one_cycle() {
        let params = params_with(FormWeights::single(FormTarget::Nebula));
        let signals = PresenceSignals::default();
        let mut morph = MorphBehavior::new(PresenceShell::new(SEED), SEED);
        morph.field_stride = 8;

        let mut set = particles(&params);
        let count = set.len();
        for p in &mut set {
            p.field_force = Vec3::ZERO;
        }

        for _ in 0..morph.field_stride {
            morph.update(&mut set, 1.0 / 60.0, &params, &signals);
        }

        let refreshed = set.iter().filter(|p| p.field_force != Vec3::ZERO).count();
        assert_eq!(
            refreshed,
            count,
            "{} of {count} particles never had their field force refreshed",
            count - refreshed
        );
    }

    #[test]
    fn the_deform_stride_reaches_the_surface_substrate() {
        let mut morph = MorphBehavior::new(PresenceShell::new(SEED), SEED);
        morph.set_deform_stride(9);
        assert_eq!(morph.surface.deform_stride, 9);
    }
}
