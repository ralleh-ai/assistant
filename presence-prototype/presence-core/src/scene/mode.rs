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

/// A thing the assistant is doing.
///
/// Idle is deliberately absent: it is not a mode but the absence of all of
/// them, which is also exactly what the resting shell is. Adding an `Idle`
/// variant would make "idle plus thinking" representable, and it isn't.
///
/// The first three are the shell-term modes from ADR-012: each raises a
/// weighted term on `PresenceShell`. The last three are *material* modes,
/// which the guidance document (§5.4) and `PRESENCE_VISUAL_ENTITY.md` §5.1
/// both call for as brightness/expansion/desaturation changes on the
/// existing shell — they add no new geometry and only touch the same
/// `EntityParams` fields the assistant's own signals already flow through.
/// That is a rule not a convenience: modelling "listening" as a fourth shell
/// term would deform the surface for a state that is exactly *not* internal
/// activity, and would collide with speaking's brightness path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenceMode {
    Thinking,
    Speaking,
    ToolUse,
    Listening,
    Attention,
    Error,
}

impl PresenceMode {
    pub const ALL: [PresenceMode; 6] = [
        PresenceMode::Thinking,
        PresenceMode::Speaking,
        PresenceMode::ToolUse,
        PresenceMode::Listening,
        PresenceMode::Attention,
        PresenceMode::Error,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PresenceMode::Thinking => "thinking",
            PresenceMode::Speaking => "speaking",
            PresenceMode::ToolUse => "tool_use",
            PresenceMode::Listening => "listening",
            PresenceMode::Attention => "attention",
            PresenceMode::Error => "error",
        }
    }

    /// Dev keyboard shortcut, mirrored in the debug panel's labels.
    pub fn key(self) -> char {
        match self {
            PresenceMode::Thinking => 'T',
            PresenceMode::Speaking => 'S',
            PresenceMode::ToolUse => 'U',
            // N for liste**N**ing; H would have been the obvious choice and
            // is not one, because a bare "H" reads as a help hotkey to a
            // first-time visitor of the debug overlay.
            PresenceMode::Listening => 'N',
            PresenceMode::Attention => 'A',
            PresenceMode::Error => 'E',
        }
    }

    fn index(self) -> usize {
        match self {
            PresenceMode::Thinking => 0,
            PresenceMode::Speaking => 1,
            PresenceMode::ToolUse => 2,
            PresenceMode::Listening => 3,
            PresenceMode::Attention => 4,
            PresenceMode::Error => 5,
        }
    }

    /// True for the modes that carry no geometry — they influence brightness,
    /// expansion, or colour on the existing shell only. Their weights never
    /// reach `ShellDrive`; `ModeLayer::drive` reads the geometry modes by
    /// name for exactly this reason.
    pub fn is_material_only(self) -> bool {
        matches!(
            self,
            PresenceMode::Listening | PresenceMode::Attention | PresenceMode::Error
        )
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
            // Listening: a small brightening and a small radial breath.
            // Intensity is deliberately below thinking's, since listening is
            // not *doing* anything — a shell that lights up as brightly to
            // hear as it does to think reads as pretending to work.
            PresenceMode::Listening => ModeProfile {
                intensity: 0.35,
                cool: 0.15,
                expand: 0.03,
                attack: 0.30,
                release: 0.55,
            },
            // Attention: a short bright rise that will typically be
            // immediately released by the caller into whatever comes next
            // (listening, thinking). It is a *notice-me*, not a state to
            // dwell in, so attack is fast and release is fast too. The value
            // is above thinking's so the pulse actually rises above whatever
            // was already showing.
            PresenceMode::Attention => ModeProfile {
                intensity: 0.95,
                cool: 0.20,
                expand: 0.06,
                // Kept at the fastest end of the 300-900ms window (which the
                // integration plan specifies and `every_mode_settles_within
                // _the_transition_window` locks in). A snap that beats 300ms
                // would out-run the eye's own saccade time and read as a
                // rendering artefact rather than as something demanding
                // attention.
                attack: 0.30,
                release: 0.45,
            },
            // Error: negative-going. `intensity: -1.0` here means "at full
            // engagement pull intensity to zero"; `apply` treats it as a
            // damping factor rather than a target, since the additive max
            // blend the other modes use cannot express a *reduction* and an
            // error that could not visibly dim the shell would only be an
            // extra colour, not an error. Attack is short so a denial reads
            // immediately; release is slower so it fades rather than snapping
            // back to whatever was underneath it — a shell that returns to
            // full brightness the instant the error clears reads as the
            // failure not having actually mattered.
            PresenceMode::Error => ModeProfile {
                intensity: -1.0,
                cool: -1.0,
                expand: -0.05,
                attack: 0.30,
                release: 0.60,
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

/// The window every visible transition in the presence has to sit inside,
/// in seconds. `PRESENCE_INTEGRATION_PLAN.md` §4.3's "300-900ms eased" is
/// the range this expresses, and `desktop-edge`'s splash/settings/core
/// crossfade at ~420ms is the reference cadence — the guide asks the
/// presence to feel like part of the same product, not a shell running at
/// its own tempo. `every_mode_settles_within_the_transition_window` in this
/// module and `presence_fade_is_within_the_transition_window` in
/// `director` are the two guardrails.
pub const TRANSITION_WINDOW_SECONDS: std::ops::RangeInclusive<f32> = 0.3..=0.9;

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
        // Runtime assertion mirrors the compile-time-adjacent test
        // `every_mode_settles_within_the_transition_window`: makes the
        // invariant visible in a debug build even for consumers that don't
        // run the crate's tests (e.g. someone dropping in a new mode from
        // outside the crate one day). Cheap enough to keep permanently.
        debug_assert!(
            PresenceMode::ALL.iter().all(|m| {
                let p = m.profile();
                TRANSITION_WINDOW_SECONDS.contains(&p.attack)
                    && TRANSITION_WINDOW_SECONDS.contains(&p.release)
            }),
            "a PresenceMode profile fell outside TRANSITION_WINDOW_SECONDS",
        );
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
    ///
    /// Reads the geometry modes by name and asserts (in debug) that any
    /// material-only mode is *not* one of them, since introducing a fourth
    /// term for e.g. listening would silently change the invariant §5.1
    /// depends on and no other test would necessarily catch the drift.
    pub fn drive(&self) -> ShellDrive {
        debug_assert!(
            !PresenceMode::Thinking.is_material_only()
                && !PresenceMode::Speaking.is_material_only()
                && !PresenceMode::ToolUse.is_material_only(),
            "a shell-term mode has been reclassified as material-only",
        );
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
    /// The positive-going per-mode values combine by weighted max rather than
    /// by sum. These are levels, not quantities: two modes at once is not
    /// twice as intense as one, it is as intense as the more demanding of
    /// them.
    ///
    /// Error is handled separately from the max blend and *after* it, as a
    /// multiplicative damping. This is not symmetric with the other modes for
    /// a reason: error is not another kind of activity, it is a
    /// non-completion of whatever was being shown, so it has to be able to
    /// reduce what the additive modes just added rather than take a max
    /// against them. Modelling it inside the additive blend would either let
    /// a low error weight be masked by any other engaged mode or force the
    /// blend to know about signs, and both are more complexity than a
    /// post-multiply.
    pub fn apply(&self, params: &mut EntityParams) {
        let base = *params;
        params.intensity = self.blend_positive(base.intensity, |p| p.intensity);
        params.cool = self.blend_positive(base.cool, |p| p.cool);
        params.expand = self.blend_positive(base.expand, |p| p.expand);

        // Error damps everything that just went above baseline. `error_dip`
        // in [0, 1]: 0 leaves the shell as-is, 1 drives it back to baseline.
        // Chosen conservative on purpose — a fully quenched shell reads as
        // *off*, and the error signature has to remain distinguishable from
        // a fade-out.
        let error_dip = 0.7 * self.weight(PresenceMode::Error);
        params.intensity = base.intensity + (params.intensity - base.intensity) * (1.0 - error_dip);
        params.cool = base.cool + (params.cool - base.cool) * (1.0 - error_dip);
        // Error's own expand target is negative and cannot ride the positive
        // blend, so it is added directly after the damping. The shell shrinks
        // slightly for the duration of the error and only then relaxes back.
        params.expand += PresenceMode::Error.profile().expand * self.weight(PresenceMode::Error);

        params.audio_envelope = self.audio_envelope;
        params.drive = self.drive();
    }

    fn blend_positive(&self, base: f32, pick: impl Fn(&ModeProfile) -> f32) -> f32 {
        PresenceMode::ALL.iter().fold(base, |acc, mode| {
            let target = pick(&mode.profile());
            // Only positive-going profiles contribute to the max blend. Error
            // reduces the outcome and is handled after the blend in `apply`.
            if target <= base {
                acc
            } else {
                acc.max(base + (target - base) * self.weight(*mode))
            }
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
                TRANSITION_WINDOW_SECONDS.contains(&profile.attack),
                "{}'s attack is outside the transition window: {}",
                mode.label(),
                profile.attack
            );
            assert!(
                TRANSITION_WINDOW_SECONDS.contains(&profile.release),
                "{}'s release is outside the transition window: {}",
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

    /// Listening is the "I am hearing you" state — a small brightening and a
    /// small breath, and specifically not a deformation. If listening ever
    /// contributed to `ShellDrive` the shell would be reshaping itself to
    /// hear, which is the opposite of what the state means.
    #[test]
    fn listening_lifts_the_shell_a_little_without_touching_its_shape() {
        assert!(PresenceMode::Listening.is_material_only());
        let mut layer = ModeLayer::new();
        layer.set(PresenceMode::Listening, true);
        run(&mut layer, 2.0);
        assert_eq!(layer.drive(), ShellDrive::IDLE);

        let mut params = EntityParams::new(Vec3::ZERO, 1.0);
        params.intensity = 0.15;
        params.expand = 0.0;
        let resting = params;
        layer.apply(&mut params);

        assert!(params.intensity > resting.intensity, "listening did not brighten the shell");
        assert!(params.expand > resting.expand, "listening did not lift the shell");
        // Listening should be visibly gentler than thinking.
        let mut alt = resting;
        let mut thinking = ModeLayer::new();
        thinking.set(PresenceMode::Thinking, true);
        run(&mut thinking, 2.0);
        thinking.apply(&mut alt);
        assert!(
            alt.intensity > params.intensity,
            "listening reached thinking's level — the two states are indistinguishable",
        );
    }

    /// Attention is deliberately louder than any of the working states — its
    /// entire content is "look at me". A pulse that peaks below the states it
    /// interrupts would be invisible against them.
    #[test]
    fn attention_rises_above_the_working_states() {
        let mut params = EntityParams::new(Vec3::ZERO, 1.0);
        params.intensity = 0.15;
        let resting = params;

        let mut layer = ModeLayer::new();
        layer.set(PresenceMode::Attention, true);
        run(&mut layer, 2.0);
        let mut with_attention = resting;
        layer.apply(&mut with_attention);

        let thinking_target = PresenceMode::Thinking.profile().intensity;
        assert!(
            with_attention.intensity > thinking_target,
            "attention peaked at or below thinking: {} vs {}",
            with_attention.intensity,
            thinking_target,
        );
    }

    /// Error must be able to *reduce* whatever else is happening, not just
    /// add another colour on top. A denial that leaves the shell as bright as
    /// it was during the request looks like the request succeeded.
    #[test]
    fn error_dims_the_shell_even_alongside_activity() {
        let mut layer = ModeLayer::new();
        layer.set(PresenceMode::Thinking, true);
        run(&mut layer, 2.0);

        let mut thinking = EntityParams::new(Vec3::ZERO, 1.0);
        thinking.intensity = 0.15;
        let resting = thinking;
        layer.apply(&mut thinking);

        layer.set(PresenceMode::Error, true);
        run(&mut layer, 2.0);
        let mut with_error = resting;
        layer.apply(&mut with_error);

        assert!(
            with_error.intensity < thinking.intensity,
            "error left intensity unchanged: {} vs {}",
            with_error.intensity,
            thinking.intensity,
        );
        assert!(
            with_error.intensity > resting.intensity,
            "error snuffed the shell back below the resting shell it should sit on top of",
        );
        assert!(
            with_error.expand < thinking.expand - 1e-4,
            "error did not pull expand in against the ongoing activity: {} vs {}",
            with_error.expand,
            thinking.expand,
        );
    }

    /// None of the material modes are allowed to raise a shell term. If they
    /// did, "listening" or an error would deform the surface — which is
    /// exactly the failure §5.1 rules out.
    #[test]
    fn material_modes_never_reach_the_shell_drive() {
        for mode in PresenceMode::ALL.iter().filter(|m| m.is_material_only()) {
            let mut layer = ModeLayer::new();
            layer.set(*mode, true);
            run(&mut layer, 2.0);
            assert_eq!(
                layer.drive(),
                ShellDrive::IDLE,
                "{} raised a shell term",
                mode.label(),
            );
        }
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
