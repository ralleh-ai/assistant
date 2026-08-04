//! Behavior Graph — stackable, blended contributions to `EntityParams`.
//!
//! This is the engine-side seed the roadmap (ADR-014, M3) grows the presence
//! from. Cognition arrives from the shell as an abstract, bounded snapshot
//! (`PresenceState` over IPC); the graph turns it into simulation parameters.
//!
//! ## Why a stack rather than one resolver
//!
//! `scene/mode.rs`'s `ModeLayer` already resolves *what the assistant is
//! doing* (thinking/speaking/…) into blended shell weights, and it is
//! carefully tuned — we do not touch its math. Instead it becomes the first
//! [`Behavior`] on the stack ([`ModeBehavior`]), and *how the assistant feels*
//! (confidence/curiosity/uncertainty) becomes a second, independent one
//! ([`CognitionBehavior`]). New behaviors (fields, morph, audio) land as
//! further entries in later milestones without any behavior having to know
//! about the others.
//!
//! ## Composition order matters, and is explicit
//!
//! Behaviors [`Behavior::apply`] in insertion order onto the *same*
//! `EntityParams`, each reading what the ones before it produced. The mode
//! layer runs first (it establishes the activity baseline); cognition runs
//! after (it modulates that baseline). This is the additive discipline from
//! ADR-012 made a first-class structure rather than a call sequence buried in
//! the director.

use crate::scene::mode::{ModeLayer, PresenceMode};
use crate::sim::{EntityParams, PresenceSignals};

pub mod response;
pub use response::{audio_response, cursor_aim, AudioResponse, CursorAim};

/// The abstract "how it feels" cognitive snapshot the engine holds between
/// frames. Mirrors the cognitive scalars on the IPC `PresenceState` (the
/// adapter in `ipc.rs` copies them across); kept free of any wire type so the
/// core crate still builds without the `ipc` feature.
///
/// The default is deliberately *neutral*, not zero: `confidence` rests at the
/// midpoint (`0.5`), so a director that has never received a `PresenceState`
/// applies no cognitive modulation at all and reproduces today's output
/// exactly. That neutrality is the contract [`CognitiveState::apply`] leans on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CognitiveState {
    /// `0.0` unsure .. `0.5` neutral .. `1.0` certain.
    pub confidence: f32,
    /// `0.0` incurious .. `1.0` reaching outward.
    pub curiosity: f32,
    /// `0.0` settled .. `1.0` wavering.
    pub uncertainty: f32,
    /// `0.0` diffuse .. `1.0` concentrated. Consumed by the morph milestone
    /// (M5); carried now so the contract is stable.
    pub focus: f32,
    /// `0.0` trivial .. `1.0` demanding.
    pub task_complexity: f32,
    /// `0.0` quiet .. `1.0` actively recalling.
    pub memory_activity: f32,
    /// Overall affective energy, `0.0` calm .. `1.0` excited.
    pub emotional_tone: f32,
}

impl Default for CognitiveState {
    fn default() -> Self {
        Self {
            confidence: 0.5,
            curiosity: 0.0,
            uncertainty: 0.0,
            focus: 0.0,
            task_complexity: 0.0,
            memory_activity: 0.0,
            emotional_tone: 0.0,
        }
    }
}

/// How much confidence brightens (or, below neutral, dims) the shell across
/// the full `0..1` range. Small: cognition colours the activity, it does not
/// compete with it.
const CONFIDENCE_INTENSITY_GAIN: f32 = 0.10;
/// Confidence also *warms* the shell (pulls `cool` down); doubt cools it. Same
/// gentle magnitude as the intensity term so the two read as one gesture.
const CONFIDENCE_WARMTH_GAIN: f32 = 0.10;
/// Curiosity leans the shell outward a touch — a reach, not a bulge, so it
/// stays a material `expand` nudge rather than a geometry (`ShellDrive`) term.
const CURIOSITY_EXPAND_GAIN: f32 = 0.05;
/// …and brightens it slightly, the "interested" lift.
const CURIOSITY_INTENSITY_GAIN: f32 = 0.06;
/// Uncertainty desaturates (raises `cool`) — the wavering, unsure look.
const UNCERTAINTY_COOL_GAIN: f32 = 0.10;
/// …and pulls a little brightness out, so doubt reads as hesitation.
const UNCERTAINTY_INTENSITY_DROP: f32 = 0.05;

impl CognitiveState {
    /// True when this state asks for no modulation at all. The graph short-
    /// circuits on it so the neutral resting engine is byte-for-byte what it
    /// was before the Behavior Graph existed.
    fn is_neutral(&self) -> bool {
        (self.confidence - 0.5).abs() <= f32::EPSILON
            && self.curiosity == 0.0
            && self.uncertainty == 0.0
    }

    /// Folds the cognitive scalars into an entity's params as bounded material
    /// modulations, treating what is already there (the mode layer's output)
    /// as the baseline to nudge.
    ///
    /// Every term here touches only *material* fields — `intensity`, `cool`,
    /// `expand` — and never `drive`/`ShellDrive`. That is the spring-bandwidth
    /// rule from ADR-012 kept by construction: cognition cannot move the
    /// surface geometry, only its brightness/warmth/breath, so nothing it does
    /// can outrun the ~0.7 Hz surface spring.
    pub fn apply(&self, params: &mut EntityParams) {
        if self.is_neutral() {
            return;
        }

        // Confidence is signed about the neutral midpoint: `+1` fully certain,
        // `-1` fully unsure, `0` neutral.
        let confidence = (self.confidence - 0.5) * 2.0;
        params.intensity += CONFIDENCE_INTENSITY_GAIN * confidence;
        params.cool -= CONFIDENCE_WARMTH_GAIN * confidence;

        params.expand += CURIOSITY_EXPAND_GAIN * self.curiosity;
        params.intensity += CURIOSITY_INTENSITY_GAIN * self.curiosity;

        params.cool += UNCERTAINTY_COOL_GAIN * self.uncertainty;
        params.intensity -= UNCERTAINTY_INTENSITY_DROP * self.uncertainty;

        // Keep the modulated result inside the ranges the shapes expect. These
        // clamps only ever bite off the cognitive contribution's own overshoot
        // (the mode layer already lands in-range), so a neutral state — which
        // returns above — is never reshaped by them.
        params.intensity = params.intensity.max(0.0);
        params.cool = params.cool.clamp(0.0, 1.0);
    }
}

/// Per-frame inputs a [`Behavior`] reads while ticking. Borrows the director's
/// live signal + cognition snapshots so a behavior never owns a stale copy.
pub struct BehaviorCtx<'a> {
    pub signals: &'a PresenceSignals,
    pub cognition: &'a CognitiveState,
}

/// One contributor to the presence's per-frame parameters.
///
/// A behavior advances its own internal state in [`tick`](Behavior::tick) and
/// writes its contribution in [`apply`](Behavior::apply). Splitting the two
/// lets the stack tick everything against a consistent snapshot before any of
/// them mutate the shared `EntityParams`.
pub trait Behavior {
    /// Advance internal state (ramps, envelopes) by `dt` seconds.
    fn tick(&mut self, dt: f32, ctx: &BehaviorCtx);
    /// Fold this behavior's contribution into `params`, reading whatever the
    /// behaviors before it in the stack already wrote.
    fn apply(&self, params: &mut EntityParams);
}

/// An ordered set of behaviors that tick together and apply in sequence.
///
/// Insertion order *is* composition order: earlier behaviors establish the
/// baseline that later ones read and modulate.
#[derive(Default)]
pub struct BehaviorStack {
    behaviors: Vec<Box<dyn Behavior>>,
}

impl BehaviorStack {
    pub fn new() -> Self {
        Self {
            behaviors: Vec::new(),
        }
    }

    pub fn push(&mut self, behavior: Box<dyn Behavior>) {
        self.behaviors.push(behavior);
    }

    pub fn tick(&mut self, dt: f32, ctx: &BehaviorCtx) {
        for behavior in &mut self.behaviors {
            behavior.tick(dt, ctx);
        }
    }

    pub fn apply(&self, params: &mut EntityParams) {
        for behavior in &self.behaviors {
            behavior.apply(params);
        }
    }
}

/// The activity layer (`ModeLayer`) as a [`Behavior`]. A thin wrapper: it owns
/// a `ModeLayer` and forwards to it, so the tuned mode math is reused verbatim
/// rather than reimplemented. The wrapped layer stays reachable for engagement
/// (`set`/`is_engaged`) via [`ModeBehavior::layer`]/[`layer_mut`].
pub struct ModeBehavior {
    layer: ModeLayer,
}

impl ModeBehavior {
    pub fn new() -> Self {
        Self {
            layer: ModeLayer::new(),
        }
    }

    pub fn layer(&self) -> &ModeLayer {
        &self.layer
    }

    pub fn layer_mut(&mut self) -> &mut ModeLayer {
        &mut self.layer
    }

    pub fn set(&mut self, mode: PresenceMode, engaged: bool) {
        self.layer.set(mode, engaged);
    }

    pub fn is_engaged(&self, mode: PresenceMode) -> bool {
        self.layer.is_engaged(mode)
    }
}

impl Default for ModeBehavior {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for ModeBehavior {
    fn tick(&mut self, dt: f32, ctx: &BehaviorCtx) {
        self.layer.tick(dt, ctx.signals);
    }

    fn apply(&self, params: &mut EntityParams) {
        self.layer.apply(params);
    }
}

/// The cognition layer as a [`Behavior`]. Holds the latest [`CognitiveState`]
/// and applies it as bounded material modulation. Stateless between frames
/// beyond the snapshot it carries, so `tick` simply refreshes that snapshot
/// from the context.
#[derive(Default)]
pub struct CognitionBehavior {
    state: CognitiveState,
}

impl CognitionBehavior {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> CognitiveState {
        self.state
    }
}

impl Behavior for CognitionBehavior {
    fn tick(&mut self, _dt: f32, ctx: &BehaviorCtx) {
        self.state = *ctx.cognition;
    }

    fn apply(&self, params: &mut EntityParams) {
        self.state.apply(params);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn resting() -> EntityParams {
        let mut p = EntityParams::new(Vec3::ZERO, 1.0);
        p.intensity = 0.15;
        p.cool = 0.20;
        p.expand = 0.0;
        p
    }

    #[test]
    fn neutral_cognition_is_a_no_op() {
        let before = resting();
        let mut after = before;
        CognitiveState::default().apply(&mut after);
        assert_eq!(after.intensity, before.intensity);
        assert_eq!(after.cool, before.cool);
        assert_eq!(after.expand, before.expand);
    }

    #[test]
    fn confidence_brightens_and_warms_doubt_dims_and_cools() {
        let base = resting();

        let mut certain = base;
        CognitiveState {
            confidence: 1.0,
            ..Default::default()
        }
        .apply(&mut certain);
        assert!(
            certain.intensity > base.intensity,
            "confidence did not brighten"
        );
        assert!(certain.cool < base.cool, "confidence did not warm");

        let mut unsure = base;
        CognitiveState {
            confidence: 0.0,
            ..Default::default()
        }
        .apply(&mut unsure);
        assert!(unsure.intensity < base.intensity, "doubt did not dim");
        assert!(unsure.cool > base.cool, "doubt did not cool");
    }

    #[test]
    fn curiosity_reaches_out_and_lifts() {
        let base = resting();
        let mut curious = base;
        CognitiveState {
            curiosity: 1.0,
            ..Default::default()
        }
        .apply(&mut curious);
        assert!(curious.expand > base.expand, "curiosity did not reach out");
        assert!(curious.intensity > base.intensity, "curiosity did not lift");
    }

    #[test]
    fn uncertainty_desaturates_and_hesitates() {
        let base = resting();
        let mut wavering = base;
        CognitiveState {
            // Hold confidence neutral so only uncertainty moves the result.
            uncertainty: 1.0,
            ..Default::default()
        }
        .apply(&mut wavering);
        assert!(wavering.cool > base.cool, "uncertainty did not desaturate");
        assert!(
            wavering.intensity < base.intensity,
            "uncertainty did not pull brightness in"
        );
    }

    /// Even at the corners of every scalar the modulation stays inside the
    /// ranges the shapes expect — a hostile snapshot can only nudge, never
    /// drive the shell out of bounds.
    #[test]
    fn extreme_cognition_stays_bounded() {
        let mut params = resting();
        CognitiveState {
            confidence: 1.0,
            curiosity: 1.0,
            uncertainty: 1.0,
            focus: 1.0,
            task_complexity: 1.0,
            memory_activity: 1.0,
            emotional_tone: 1.0,
        }
        .apply(&mut params);
        assert!(params.intensity >= 0.0);
        assert!((0.0..=1.0).contains(&params.cool));
    }

    /// The port must be exact: a stack whose only behavior is the mode layer
    /// produces the same params as driving that `ModeLayer` directly. If this
    /// drifts, the Behavior Graph changed the tuned mode output — which M3 is
    /// explicitly not allowed to do.
    #[test]
    fn mode_behavior_reproduces_mode_layer_output_exactly() {
        let signals = PresenceSignals::default();
        let cognition = CognitiveState::default();
        let ctx = BehaviorCtx {
            signals: &signals,
            cognition: &cognition,
        };

        let mut layer = ModeLayer::new();
        let mut stack = BehaviorStack::new();
        let mut mode = ModeBehavior::new();
        // Engage the same modes on both.
        layer.set(PresenceMode::Thinking, true);
        layer.set(PresenceMode::ToolUse, true);
        mode.set(PresenceMode::Thinking, true);
        mode.set(PresenceMode::ToolUse, true);
        stack.push(Box::new(mode));

        for _ in 0..120 {
            layer.tick(1.0 / 60.0, &signals);
            stack.tick(1.0 / 60.0, &ctx);
        }

        let mut direct = resting();
        let mut via_stack = resting();
        layer.apply(&mut direct);
        stack.apply(&mut via_stack);

        assert_eq!(via_stack.intensity, direct.intensity);
        assert_eq!(via_stack.cool, direct.cool);
        assert_eq!(via_stack.expand, direct.expand);
        assert_eq!(via_stack.audio_envelope, direct.audio_envelope);
        assert_eq!(via_stack.drive, direct.drive);
    }

    /// Cognition applies *after* the mode layer and modulates its output. The
    /// stack ordering is the contract: mode establishes the baseline, cognition
    /// nudges it.
    #[test]
    fn stack_applies_cognition_after_modes() {
        let signals = PresenceSignals::default();
        let cognition = CognitiveState {
            confidence: 1.0,
            ..Default::default()
        };
        let ctx = BehaviorCtx {
            signals: &signals,
            cognition: &cognition,
        };

        let mut stack = BehaviorStack::new();
        let mut mode = ModeBehavior::new();
        mode.set(PresenceMode::Thinking, true);
        stack.push(Box::new(mode));
        stack.push(Box::new(CognitionBehavior::new()));

        for _ in 0..120 {
            stack.tick(1.0 / 60.0, &ctx);
        }

        // Mode-only baseline for the same engagement.
        let mut mode_only = ModeLayer::new();
        mode_only.set(PresenceMode::Thinking, true);
        for _ in 0..120 {
            mode_only.tick(1.0 / 60.0, &signals);
        }
        let mut baseline = resting();
        mode_only.apply(&mut baseline);

        let mut modulated = resting();
        stack.apply(&mut modulated);

        assert!(
            modulated.intensity > baseline.intensity,
            "confident cognition did not lift the mode baseline: {} vs {}",
            modulated.intensity,
            baseline.intensity,
        );
    }
}
