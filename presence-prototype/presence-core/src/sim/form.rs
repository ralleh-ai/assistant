//! Form — what shape the one body is currently holding (ADR-015).
//!
//! # Weights, not a selection
//!
//! A form is never chosen; it is *weighted*. This is ADR-012's additive rule
//! ("modes compose additively on one shell, rather than selecting exclusive
//! shapes") applied one level up, to shape itself. Half droplet and half ring
//! is a legal, meaningful state — and it is the state that carries most of the
//! expression, exactly as overlapping modes do. An exclusive selector would
//! make that untestable by construction.
//!
//! # Dense here, sparse on the wire
//!
//! The engine keeps a dense weight per known target, because blending a dense
//! array is trivial and allocation-free on the hot path. The IPC contract
//! (ADR-015 decision 4) carries a *length-capped list* of `(target, weight)`
//! pairs instead, which is what keeps the payload bounded while leaving the
//! vocabulary open to grow. Expanding the sparse wire form into this dense one
//! is the adapter's job; nothing below this line needs to know the wire shape.
//!
//! # Transitions are rate-limited by the spring
//!
//! [`FormTransition`] eases weights toward a target over a caller-chosen
//! duration, floored at [`MIN_FORM_TRANSITION_SECONDS`]. That floor is the
//! ADR-012 spring-bandwidth rule: geometry cannot be moved faster than the
//! surface spring can carry without reading as a teleport rather than a
//! transformation. It is a property of the medium, not a per-target tuning
//! knob, which is why it lives here and not in the scene data.

use crate::scene::mode::step_toward;

/// Number of shapes in the vocabulary. Growing this is additive: a new variant
/// defaults to zero weight everywhere, so no existing state changes meaning.
pub const FORM_TARGET_COUNT: usize = 6;

/// Shortest a form transition may take, in seconds.
///
/// Tied to the top of [`TRANSITION_WINDOW_SECONDS`]: a form change moves more
/// geometry than any single mode does, so it may never be *faster* than the
/// slowest mode transition the spring is already known to carry.
/// `the_transition_floor_respects_the_spring_bandwidth` is the guardrail.
pub const MIN_FORM_TRANSITION_SECONDS: f32 = 0.9;

/// A shape the presence can hold. Sphere/ring/helix are signed-distance
/// targets the force field pulls toward (`sim::field::sdf`); droplet is the
/// parametric scanned shell (ADR-011) carried by the surface spring; nebula is
/// free space with no target at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormTarget {
    /// The resting scanned shell — the product's identity, and the only target
    /// simulated by the surface spring rather than by fields.
    Droplet,
    Sphere,
    Ring,
    Helix,
    /// A loose free-space cloud with no shape to hold.
    Nebula,
    /// Parametric rather than implicit — the first target defined by where its
    /// surface is rather than by a distance to it, and the proof that the
    /// vocabulary is not limited to shapes with closed-form distances.
    Heart,
}

impl FormTarget {
    pub const ALL: [FormTarget; FORM_TARGET_COUNT] = [
        FormTarget::Droplet,
        FormTarget::Sphere,
        FormTarget::Ring,
        FormTarget::Helix,
        FormTarget::Nebula,
        FormTarget::Heart,
    ];

    pub fn index(self) -> usize {
        match self {
            FormTarget::Droplet => 0,
            FormTarget::Sphere => 1,
            FormTarget::Ring => 2,
            FormTarget::Helix => 3,
            FormTarget::Nebula => 4,
            FormTarget::Heart => 5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FormTarget::Droplet => "droplet",
            FormTarget::Sphere => "sphere",
            FormTarget::Ring => "ring",
            FormTarget::Helix => "helix",
            FormTarget::Nebula => "nebula",
            FormTarget::Heart => "heart",
        }
    }

    /// Whether this target is carried by the surface spring (ADR-011) rather
    /// than by the force-field substrate.
    pub fn is_surface(self) -> bool {
        matches!(self, FormTarget::Droplet)
    }
}

/// How much of each shape the body is currently holding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FormWeights {
    weights: [f32; FORM_TARGET_COUNT],
}

impl Default for FormWeights {
    /// Resting state is the droplet, which is what the presence is when
    /// nothing has asked it to be anything else.
    fn default() -> Self {
        Self::single(FormTarget::Droplet)
    }
}

impl FormWeights {
    pub fn zeroed() -> Self {
        Self {
            weights: [0.0; FORM_TARGET_COUNT],
        }
    }

    pub fn single(target: FormTarget) -> Self {
        let mut w = Self::zeroed();
        w.set(target, 1.0);
        w
    }

    pub fn get(&self, target: FormTarget) -> f32 {
        self.weights[target.index()]
    }

    /// Sets one weight, clamping to `[0, 1]` and folding NaN to zero — a
    /// malformed weight can only fail to contribute, never corrupt the blend.
    pub fn set(&mut self, target: FormTarget, weight: f32) {
        let w = if weight.is_nan() {
            0.0
        } else {
            weight.clamp(0.0, 1.0)
        };
        self.weights[target.index()] = w;
    }

    pub fn total(&self) -> f32 {
        self.weights.iter().sum()
    }

    /// Weights scaled to sum to one.
    ///
    /// An all-zero set falls back to the droplet rather than to nothing: a body
    /// holding no form at all is not a state the engine can render, and
    /// silently drawing an empty screen would be a worse failure than ignoring
    /// the request.
    pub fn normalized(&self) -> Self {
        let total = self.total();
        if total <= 1e-6 {
            return Self::single(FormTarget::Droplet);
        }
        let mut out = Self::zeroed();
        for (i, w) in self.weights.iter().enumerate() {
            out.weights[i] = w / total;
        }
        out
    }

    /// Normalized share carried by the surface spring.
    pub fn surface_weight(&self) -> f32 {
        let n = self.normalized();
        FormTarget::ALL
            .iter()
            .filter(|t| t.is_surface())
            .map(|t| n.get(*t))
            .sum()
    }

    /// Normalized share carried by the force fields — the complement of
    /// [`surface_weight`](Self::surface_weight).
    pub fn field_weight(&self) -> f32 {
        (1.0 - self.surface_weight()).clamp(0.0, 1.0)
    }

    /// Every non-surface target and its normalized weight, skipping the ones
    /// that contribute nothing. The blend iterates this, so a form that is not
    /// engaged costs nothing to have in the vocabulary.
    pub fn field_terms(&self) -> impl Iterator<Item = (FormTarget, f32)> + '_ {
        let n = self.normalized();
        FormTarget::ALL
            .into_iter()
            .filter(move |t| !t.is_surface() && n.get(*t) > 1e-6)
            .map(move |t| (t, n.get(t)))
    }
}

/// Eases [`FormWeights`] toward a target over time.
///
/// The ramp is linear per weight (see [`step_toward`]) precisely so that a
/// transition reversed mid-flight resumes from where the body actually is
/// rather than jumping to wherever an eased curve crosses that value — the
/// same property `ModeLayer` relies on.
#[derive(Clone, Debug)]
pub struct FormTransition {
    current: FormWeights,
    target: FormWeights,
    seconds: f32,
}

impl Default for FormTransition {
    fn default() -> Self {
        Self::new(FormWeights::default())
    }
}

impl FormTransition {
    pub fn new(initial: FormWeights) -> Self {
        Self {
            current: initial,
            target: initial,
            seconds: MIN_FORM_TRANSITION_SECONDS,
        }
    }

    /// Aims at a new set of weights. `seconds` is floored at
    /// [`MIN_FORM_TRANSITION_SECONDS`]; a caller asking for an instant morph
    /// gets the fastest one the spring can carry instead, because the
    /// alternative is a teleport.
    pub fn set_target(&mut self, target: FormWeights, seconds: f32) {
        self.target = target;
        self.seconds = if seconds.is_nan() {
            MIN_FORM_TRANSITION_SECONDS
        } else {
            seconds.max(MIN_FORM_TRANSITION_SECONDS)
        };
    }

    pub fn target(&self) -> FormWeights {
        self.target
    }

    /// The live weights, normalized and ready to blend with.
    pub fn current(&self) -> FormWeights {
        self.current.normalized()
    }

    pub fn is_settled(&self) -> bool {
        FormTarget::ALL
            .iter()
            .all(|t| (self.current.get(*t) - self.target.get(*t)).abs() < 1e-4)
    }

    /// Advances every weight one step toward the target.
    pub fn tick(&mut self, dt: f32) -> FormWeights {
        if dt > 0.0 {
            for target in FormTarget::ALL {
                let stepped = step_toward(
                    self.current.get(target),
                    self.target.get(target),
                    dt,
                    self.seconds,
                );
                self.current.set(target, stepped);
            }
        }
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::mode::TRANSITION_WINDOW_SECONDS;

    fn run(transition: &mut FormTransition, seconds: f32) {
        let steps = (seconds * 60.0).round() as i32;
        for _ in 0..steps {
            transition.tick(1.0 / 60.0);
        }
    }

    #[test]
    fn the_transition_floor_respects_the_spring_bandwidth() {
        // A form change moves more geometry than a mode does, so it may never
        // be faster than the slowest mode transition the spring is known to
        // carry (ADR-012).
        assert!(
            MIN_FORM_TRANSITION_SECONDS >= *TRANSITION_WINDOW_SECONDS.end(),
            "the form transition floor is inside the mode window: {MIN_FORM_TRANSITION_SECONDS}",
        );
    }

    #[test]
    fn a_faster_transition_than_the_spring_allows_is_floored() {
        let mut transition = FormTransition::new(FormWeights::default());
        transition.set_target(FormWeights::single(FormTarget::Ring), 0.0);

        // Half the floor in: nowhere near settled, because the request was
        // clamped up to the floor rather than honoured as an instant cut.
        run(&mut transition, MIN_FORM_TRANSITION_SECONDS * 0.5);
        assert!(
            !transition.is_settled(),
            "an instant morph was not floored to the spring's rate",
        );

        run(&mut transition, MIN_FORM_TRANSITION_SECONDS);
        assert!(transition.is_settled(), "the morph never completed");
    }

    #[test]
    fn the_resting_form_is_the_droplet() {
        let w = FormWeights::default();
        assert_eq!(w.get(FormTarget::Droplet), 1.0);
        assert_eq!(w.surface_weight(), 1.0);
        assert_eq!(w.field_weight(), 0.0);
        assert_eq!(w.field_terms().count(), 0);
    }

    #[test]
    fn weights_normalize_and_split_between_the_substrates() {
        let mut w = FormWeights::zeroed();
        w.set(FormTarget::Droplet, 1.0);
        w.set(FormTarget::Ring, 3.0); // over-range on purpose: clamps to 1.0

        let n = w.normalized();
        assert!((n.total() - 1.0).abs() < 1e-6, "did not normalize: {n:?}");
        assert!((w.surface_weight() - 0.5).abs() < 1e-6);
        assert!((w.field_weight() - 0.5).abs() < 1e-6);

        let terms: Vec<_> = w.field_terms().collect();
        assert_eq!(terms.len(), 1, "only the ring should contribute a field");
        assert_eq!(terms[0].0, FormTarget::Ring);
    }

    #[test]
    fn a_formless_body_falls_back_to_the_droplet() {
        let w = FormWeights::zeroed();
        assert_eq!(w.normalized().get(FormTarget::Droplet), 1.0);
        assert_eq!(w.surface_weight(), 1.0);
    }

    #[test]
    fn nan_and_out_of_range_weights_cannot_corrupt_the_blend() {
        let mut w = FormWeights::zeroed();
        w.set(FormTarget::Sphere, f32::NAN);
        w.set(FormTarget::Ring, -5.0);
        w.set(FormTarget::Helix, 42.0);

        assert_eq!(w.get(FormTarget::Sphere), 0.0);
        assert_eq!(w.get(FormTarget::Ring), 0.0);
        assert_eq!(w.get(FormTarget::Helix), 1.0);
        assert!(w.normalized().total().is_finite());
    }

    #[test]
    fn adding_a_target_to_the_vocabulary_stays_additive() {
        // Every target must default to zero, so extending the enum cannot
        // change what an existing weight set means.
        let w = FormWeights::zeroed();
        for target in FormTarget::ALL {
            assert_eq!(w.get(target), 0.0, "{} defaulted non-zero", target.label());
        }
        assert_eq!(FormTarget::ALL.len(), FORM_TARGET_COUNT);
    }

    #[test]
    fn a_transition_completes_within_its_window() {
        let mut transition = FormTransition::new(FormWeights::default());
        transition.set_target(FormWeights::single(FormTarget::Helix), 1.2);
        assert!(!transition.is_settled());

        run(&mut transition, 1.25);
        assert!(transition.is_settled(), "the morph did not settle in time");
        let now = transition.current();
        assert!((now.get(FormTarget::Helix) - 1.0).abs() < 1e-3);
        assert!(now.get(FormTarget::Droplet) < 1e-3);
    }

    #[test]
    fn reversing_mid_transition_continues_from_the_current_weights() {
        let mut transition = FormTransition::new(FormWeights::default());
        transition.set_target(FormWeights::single(FormTarget::Sphere), 1.0);
        run(&mut transition, 0.5);

        let midway = transition.current().get(FormTarget::Sphere);
        assert!(
            midway > 0.05 && midway < 0.95,
            "expected a partial morph to reverse from: {midway}"
        );

        // Reverse: the next step must move back down from where we actually
        // are, not jump anywhere.
        transition.set_target(FormWeights::default(), 1.0);
        transition.tick(1.0 / 60.0);
        let after = transition.current().get(FormTarget::Sphere);
        assert!(
            after < midway && (midway - after) < 0.1,
            "reversal jumped rather than resuming: {midway} -> {after}"
        );
    }

    #[test]
    fn ticking_with_zero_dt_moves_nothing() {
        let mut transition = FormTransition::new(FormWeights::default());
        transition.set_target(FormWeights::single(FormTarget::Ring), 1.0);
        let before = transition.current();
        transition.tick(0.0);
        assert_eq!(transition.current(), before);
    }

    #[test]
    fn weights_stay_bounded_across_a_long_run_of_random_targets() {
        let mut transition = FormTransition::new(FormWeights::default());
        for (i, target) in FormTarget::ALL.into_iter().cycle().take(40).enumerate() {
            let mut w = FormWeights::zeroed();
            w.set(target, 1.0);
            // Mix in a second form so the blends are genuinely partial.
            w.set(FormTarget::ALL[i % FORM_TARGET_COUNT], 0.5);
            transition.set_target(w, 1.0);
            run(&mut transition, 0.3);

            let now = transition.current();
            assert!((now.total() - 1.0).abs() < 1e-4, "weights left the simplex");
            for t in FormTarget::ALL {
                let v = now.get(t);
                assert!((0.0..=1.0).contains(&v), "{} escaped [0,1]: {v}", t.label());
            }
        }
    }
}
