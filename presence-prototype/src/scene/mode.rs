//! What the assistant is *doing*, and the shell weights that expresses.
//!
//! `docs/PRESENCE_VISUAL_ENTITY.md` §5.1 lists the states; this is the layer
//! that turns them into numbers the shell can add up.
//!
//! ## Modes compose rather than switch
//!
//! There is no "current mode" here, only a set of engaged ones. A real
//! assistant narrates a tool call while it is running it and keeps thinking
//! while it speaks, so a single-slot mode would spend its life mid-transition
//! between states that are all genuinely true at once. Each mode raises its own
//! shell term, so two engaged modes are simply two raised weights and the
//! overlap needs no code of its own.
//!
//! ## Transitions are weight ramps, not cross-fades
//!
//! Because the terms are additive, changing state never means swapping one
//! population for another — nothing has to fade out while something else fades
//! in, and the particle set is untouched. `PRESENCE_INTEGRATION_PLAN.md` asks
//! for eased 300-900ms transitions, so each mode carries its own attack and
//! release inside that window and the weight is a smoothstep of a linear ramp.
//! Easing the value directly instead would ease only the *first* step out of
//! rest and run linearly thereafter, which is the shape it is trying not to be.

use crate::sim::{EntityParams, PresenceSignals, ShellDrive};

/// A thing the assistant is doing that the presence has geometry for.
///
/// Idle is deliberately absent: it is not a mode but the absence of all of
/// them, which is also exactly what the resting shell is. Adding an `Idle`
/// variant would make "idle plus thinking" representable, and it isn't.
///
/// `listening`, `error`, and `attention` from §5.1 are not here either. They
/// need no geometry — they are colour, brightness, and framing changes — so
/// they will sit on this weight machinery without adding shell terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenceMode {
    Thinking,
    Speaking,
    ToolUse,
}

impl PresenceMode {
    pub const ALL: [PresenceMode; 3] = [
        PresenceMode::Thinking,
        PresenceMode::Speaking,
        PresenceMode::ToolUse,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PresenceMode::Thinking => "thinking",
            PresenceMode::Speaking => "speaking",
            PresenceMode::ToolUse => "tool_use",
        }
    }

    /// Dev keyboard shortcut, mirrored in the debug panel's labels.
    pub fn key(self) -> char {
        match self {
            PresenceMode::Thinking => 'T',
            PresenceMode::Speaking => 'S',
            PresenceMode::ToolUse => 'U',
        }
    }

    fn index(self) -> usize {
        match self {
            PresenceMode::Thinking => 0,
            PresenceMode::Speaking => 1,
            PresenceMode::ToolUse => 2,
        }
    }

    fn profile(self) -> ModeProfile {
        match self {
            // The most internally complex state (§5.1), and the slowest to
            // arrive: thinking that snaps on reads as a UI toggle rather than
            // as something building.
            PresenceMode::Thinking => ModeProfile {
                intensity: 0.75,
                cool: 0.85,
                expand: 0.10,
                attack: 0.9,
                release: 0.8,
            },
            // Fast in and fast out. Speech starts and stops abruptly, and a
            // shell still swelling half a second into an utterance is visibly
            // lagging the thing it is supposed to be showing.
            PresenceMode::Speaking => ModeProfile {
                intensity: 0.55,
                cool: 0.45,
                expand: 0.05,
                attack: 0.35,
                release: 0.45,
            },
            // The pendant's extension *is* this weight (see the neck term), so
            // attack and release are the reach and the retraction. Release is
            // longer than attack because a call completing should read as the
            // shell drawing something back in, not as the pendant vanishing.
            PresenceMode::ToolUse => ModeProfile {
                intensity: 0.60,
                cool: 0.70,
                expand: 0.04,
                attack: 0.55,
                release: 0.75,
            },
        }
    }
}

/// What one mode asks of the presence when fully engaged.
struct ModeProfile {
    intensity: f32,
    cool: f32,
    expand: f32,
    /// Seconds for the mode's weight to travel 0 to 1, and back.
    attack: f32,
    release: f32,
}

/// How far the resting fold recedes when the additive terms are at full
/// weight.
///
/// Some yield is necessary: four terms all displacing outward at once
/// compounds into a shell that leaves its scale budget. But `fold` keeps most
/// of its depth, because it is the entity's identity rather than a background
/// the other terms are drawn over — a shell that flattens to a sphere whenever
/// anything happens has its calm state as the only state with any character.
///
/// This was 0.42 first, which is not "most" and did not look like it either:
/// with the folds that far down, thinking read as the shell smoothing out and
/// inflating rather than as bulges rising through a folded skin. The radial
/// budget that bought back is worth less than it costs, because the shell only
/// spends all of it where a lobe summit, a pendant tip, and a fold peak
/// coincide — which the radius clamp already handles, and which is rare.
const FOLD_YIELD: f32 = 0.25;

/// Seconds for the speech phrase envelope to follow the raw level.
///
/// Around a 0.35 Hz corner, comfortably inside what the surface spring passes.
/// This is a phrase envelope, not a syllable one, and it is deliberately not
/// tunable per mode: the constraint it encodes is the spring's, not speech's.
const AUDIO_ENVELOPE_SECONDS: f32 = 0.45;

/// The engaged modes and their eased weights.
#[derive(Clone, Debug)]
pub struct ModeLayer {
    engaged: [bool; PresenceMode::ALL.len()],
    /// Linear ramp per mode; the published weight is a smoothstep of this.
    ramp: [f32; PresenceMode::ALL.len()],
    audio_envelope: f32,
}

impl ModeLayer {
    pub fn new() -> Self {
        Self {
            engaged: [false; PresenceMode::ALL.len()],
            ramp: [0.0; PresenceMode::ALL.len()],
            audio_envelope: 0.0,
        }
    }

    pub fn set(&mut self, mode: PresenceMode, engaged: bool) {
        self.engaged[mode.index()] = engaged;
    }

    pub fn toggle(&mut self, mode: PresenceMode) {
        let i = mode.index();
        self.engaged[i] = !self.engaged[i];
    }

    pub fn is_engaged(&self, mode: PresenceMode) -> bool {
        self.engaged[mode.index()]
    }

    /// The mode's contribution, eased. Zero when it has fully released, which
    /// is what lets the shell skip its term.
    pub fn weight(&self, mode: PresenceMode) -> f32 {
        smoothstep01(self.ramp[mode.index()])
    }

    /// True once every ramp has settled at its target, so nothing is still
    /// moving. Read by the debug panel; also the condition under which idle is
    /// genuinely back at its resting cost.
    pub fn is_settled(&self) -> bool {
        PresenceMode::ALL.iter().all(|m| {
            let ramp = self.ramp[m.index()];
            if self.engaged[m.index()] {
                ramp >= 1.0
            } else {
                ramp <= 0.0
            }
        })
    }

    pub fn tick(&mut self, dt: f32, signals: &PresenceSignals) {
        for mode in PresenceMode::ALL {
            let i = mode.index();
            let profile = mode.profile();
            let (target, duration) = if self.engaged[i] {
                (1.0, profile.attack)
            } else {
                (0.0, profile.release)
            };
            self.ramp[i] = step_toward(self.ramp[i], target, dt, duration);
        }

        // One-pole toward the live level. Falls as slowly as it rises, so a
        // pause between phrases relaxes the shell rather than dropping it.
        let alpha = (dt / AUDIO_ENVELOPE_SECONDS.max(1e-4)).clamp(0.0, 1.0);
        self.audio_envelope += (signals.audio_level.clamp(0.0, 1.0) - self.audio_envelope) * alpha;
    }

    /// This frame's shell term weights.
    pub fn drive(&self) -> ShellDrive {
        let lobes = self.weight(PresenceMode::Thinking);
        let pulse = self.weight(PresenceMode::Speaking);
        let neck = self.weight(PresenceMode::ToolUse);
        ShellDrive {
            // Pulse is excluded from the yield on purpose: it is a shallow
            // ripple *of* the radius rather than an addition to it, so it
            // spends none of the radial budget the yield exists to protect.
            // Charging it would make the shell visibly deflate the moment
            // speech began, for a term that changes the radius by a few
            // percent.
            fold: 1.0 - FOLD_YIELD * (lobes + neck).min(1.0),
            lobes,
            pulse,
            neck,
        }
    }

    /// Folds the engaged modes into an entity's per-frame params, treating the
    /// values already there as the resting baseline.
    ///
    /// The per-mode values combine by weighted max rather than by sum. These
    /// are levels, not quantities: two modes at once is not twice as intense
    /// as one, it is as intense as the more demanding of them.
    pub fn apply(&self, params: &mut EntityParams) {
        let base = *params;
        params.intensity = self.blend(base.intensity, |p| p.intensity);
        params.cool = self.blend(base.cool, |p| p.cool);
        params.expand = self.blend(base.expand, |p| p.expand);
        params.audio_envelope = self.audio_envelope;
        params.drive = self.drive();
    }

    fn blend(&self, base: f32, pick: impl Fn(&ModeProfile) -> f32) -> f32 {
        PresenceMode::ALL.iter().fold(base, |acc, mode| {
            let target = pick(&mode.profile());
            acc.max(base + (target - base) * self.weight(*mode))
        })
    }

    /// Human-readable engaged set, for the debug overlay.
    pub fn summary(&self) -> String {
        let engaged: Vec<&str> = PresenceMode::ALL
            .iter()
            .filter(|m| self.is_engaged(**m))
            .map(|m| m.label())
            .collect();
        if engaged.is_empty() {
            "idle".to_string()
        } else {
            engaged.join(" + ")
        }
    }
}

impl Default for ModeLayer {
    fn default() -> Self {
        Self::new()
    }
}

/// Linear step of `current` toward `target` at `1/duration_secs` per second,
/// clamped to `[0, 1]`.
///
/// This is `step_presence` generalized off the entity-fade case it was written
/// for: the same ramp, with the duration a caller's choice rather than a
/// module constant. The easing is applied by the reader, not here, so a ramp
/// that reverses mid-flight resumes from where it actually is instead of
/// jumping to wherever the eased curve happens to cross that value.
pub(crate) fn step_toward(current: f32, target: f32, dt: f32, duration_secs: f32) -> f32 {
    let step = if duration_secs > 0.0 {
        dt / duration_secs
    } else {
        1.0
    };
    (current + (target - current).clamp(-step, step)).clamp(0.0, 1.0)
}

fn smoothstep01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn run(layer: &mut ModeLayer, seconds: f32) {
        let signals = PresenceSignals::default();
        for _ in 0..(seconds * 60.0) as usize {
            layer.tick(1.0 / 60.0, &signals);
        }
    }

    #[test]
    fn nothing_engaged_is_exactly_the_resting_shell() {
        let layer = ModeLayer::new();
        assert_eq!(layer.drive(), ShellDrive::IDLE);
        assert_eq!(layer.summary(), "idle");
        assert!(layer.is_settled());
    }

    /// The gate that keeps idle cheap only works if released modes reach
    /// *exactly* zero. An asymptotic decay would leave every term permanently
    /// just above the gate, so the shell would evaluate all four forever after
    /// the first time anything was engaged.
    #[test]
    fn releasing_a_mode_returns_the_weight_to_a_hard_zero() {
        let mut layer = ModeLayer::new();
        layer.set(PresenceMode::Thinking, true);
        run(&mut layer, 2.0);
        assert!(layer.weight(PresenceMode::Thinking) > 0.99);

        layer.set(PresenceMode::Thinking, false);
        run(&mut layer, 2.0);
        assert_eq!(layer.weight(PresenceMode::Thinking), 0.0);
        assert_eq!(layer.drive(), ShellDrive::IDLE);
        assert!(layer.is_settled());
    }

    #[test]
    fn every_mode_settles_within_the_transition_window() {
        for mode in PresenceMode::ALL {
            let profile = mode.profile();
            assert!(
                (0.3..=0.9).contains(&profile.attack),
                "{}'s attack is outside the 300-900ms window: {}",
                mode.label(),
                profile.attack
            );
            assert!(
                (0.3..=0.9).contains(&profile.release),
                "{}'s release is outside the 300-900ms window: {}",
                mode.label(),
                profile.release
            );

            let mut layer = ModeLayer::new();
            layer.set(mode, true);
            run(&mut layer, profile.attack + 0.05);
            assert!(
                layer.weight(mode) > 0.99,
                "{} did not reach full weight within its attack",
                mode.label()
            );
        }
    }

    /// The weight must ease at *both* ends. A ramp that leaves rest at full
    /// rate is the linear transition this replaced, and it shows as a visible
    /// kick at the start of every mode change.
    #[test]
    fn the_weight_eases_in_rather_than_ramping_linearly() {
        let mut layer = ModeLayer::new();
        layer.set(PresenceMode::Thinking, true);
        let attack = PresenceMode::Thinking.profile().attack;

        run(&mut layer, attack * 0.1);
        let early = layer.weight(PresenceMode::Thinking);
        run(&mut layer, attack * 0.4);
        let middle = layer.weight(PresenceMode::Thinking);

        assert!(
            early < 0.1 * 0.5,
            "the ramp left rest at full rate: {early}"
        );
        assert!(middle > 0.4, "the ramp never picked up speed: {middle}");
    }

    /// Interrupting a transition has to resume from where the shell actually
    /// is. Restarting the ramp would make a quickly-cancelled mode snap back,
    /// which is the one moment the motion is being watched closely.
    #[test]
    fn reversing_mid_transition_continues_from_the_current_weight() {
        let mut layer = ModeLayer::new();
        layer.set(PresenceMode::ToolUse, true);
        run(&mut layer, 0.25);
        let interrupted = layer.weight(PresenceMode::ToolUse);
        assert!(
            (0.05..0.95).contains(&interrupted),
            "not mid-flight: {interrupted}"
        );

        layer.set(PresenceMode::ToolUse, false);
        layer.tick(1.0 / 60.0, &PresenceSignals::default());
        let after = layer.weight(PresenceMode::ToolUse);
        assert!(after < interrupted, "the weight did not turn around");
        assert!(
            interrupted - after < 0.15,
            "the weight jumped on reversal: {interrupted} -> {after}"
        );
    }

    /// The premise of the whole model: concurrency needs no special case.
    #[test]
    fn two_modes_at_once_raise_two_terms() {
        let mut layer = ModeLayer::new();
        layer.set(PresenceMode::Thinking, true);
        layer.set(PresenceMode::ToolUse, true);
        run(&mut layer, 2.0);

        let drive = layer.drive();
        assert!(drive.lobes > 0.99 && drive.neck > 0.99);
        assert_eq!(drive.pulse, 0.0);
        assert_eq!(layer.summary(), "thinking + tool_use");
    }

    /// The fold recedes under load but stays the dominant term. A shell whose
    /// identity switches off the moment it has something to say is a different
    /// entity per state, which is what §3.1 rules out.
    #[test]
    fn the_fold_yields_to_other_terms_without_ever_disappearing() {
        let mut layer = ModeLayer::new();
        assert_eq!(layer.drive().fold, 1.0);

        for mode in PresenceMode::ALL {
            layer.set(mode, true);
        }
        run(&mut layer, 2.0);

        let fold = layer.drive().fold;
        assert!(fold < 1.0, "the fold did not yield at all");
        assert!(fold > 0.5, "the fold gave up the shell's identity: {fold}");
    }

    /// Speech is a ripple, not a bulge, so engaging it alone must not deflate
    /// the fold.
    #[test]
    fn speaking_alone_costs_the_fold_nothing() {
        let mut layer = ModeLayer::new();
        layer.set(PresenceMode::Speaking, true);
        run(&mut layer, 2.0);

        let drive = layer.drive();
        assert_eq!(drive.fold, 1.0);
        assert!(drive.pulse > 0.99);
    }

    #[test]
    fn modes_raise_intensity_and_cool_above_the_resting_baseline() {
        let mut params = EntityParams::new(Vec3::ZERO, 1.0);
        params.intensity = 0.15;
        params.cool = 0.0;
        let resting = params;

        let mut layer = ModeLayer::new();
        layer.apply(&mut params);
        assert_eq!(
            params.intensity, resting.intensity,
            "idle moved the baseline"
        );
        assert_eq!(params.cool, resting.cool);

        layer.set(PresenceMode::Thinking, true);
        run(&mut layer, 2.0);
        let mut active = resting;
        layer.apply(&mut active);
        assert!(active.intensity > resting.intensity);
        assert!(active.cool > resting.cool);
    }

    /// Levels, not quantities. Summing would make "thinking while speaking"
    /// brighter than either mode is ever supposed to get on its own.
    #[test]
    fn concurrent_modes_take_the_stronger_level_rather_than_adding_up() {
        let mut layer = ModeLayer::new();
        layer.set(PresenceMode::Thinking, true);
        layer.set(PresenceMode::Speaking, true);
        run(&mut layer, 2.0);

        let mut params = EntityParams::new(Vec3::ZERO, 1.0);
        params.intensity = 0.15;
        layer.apply(&mut params);

        let solo = PresenceMode::Thinking.profile().intensity;
        assert!(
            (params.intensity - solo).abs() < 1e-3,
            "levels summed: {}",
            params.intensity
        );
    }

    /// The envelope must be slow enough for the surface spring to pass it.
    /// Following the raw level would put a 4-7 Hz signal into geometry that
    /// attenuates it to nothing, so the shell would sit still while the panel
    /// insisted it was speaking.
    #[test]
    fn the_audio_envelope_lags_the_raw_level() {
        let mut layer = ModeLayer::new();
        let loud = PresenceSignals {
            audio_level: 1.0,
            ..Default::default()
        };

        layer.tick(1.0 / 60.0, &loud);
        assert!(
            layer.audio_envelope < 0.1,
            "the envelope tracked a single frame of audio: {}",
            layer.audio_envelope
        );

        for _ in 0..120 {
            layer.tick(1.0 / 60.0, &loud);
        }
        assert!(layer.audio_envelope > 0.9, "the envelope never caught up");

        let quiet = PresenceSignals::default();
        for _ in 0..120 {
            layer.tick(1.0 / 60.0, &quiet);
        }
        assert!(layer.audio_envelope < 0.1, "the envelope never relaxed");
    }

    #[test]
    fn step_toward_handles_zero_duration_as_an_instant_cut() {
        assert_eq!(step_toward(0.0, 1.0, 1.0 / 60.0, 0.0), 1.0);
    }

    #[test]
    fn ticking_with_zero_dt_moves_nothing() {
        let mut layer = ModeLayer::new();
        layer.set(PresenceMode::Speaking, true);
        run(&mut layer, 0.2);
        let before = layer.drive();
        layer.tick(0.0, &PresenceSignals::default());
        assert_eq!(layer.drive(), before);
    }
}
