//! The Presence Brain — the shell-side cognition layer of ADR-014.
//!
//! # What this is
//!
//! The Brain is the *only* component that knows the assistant exists. It
//! receives abstract lifecycle events — a request started, a tool call is
//! running, the mic heard speech, the cursor moved — and folds them into a
//! single bounded [`PresenceState`](presence_ipc::PresenceState) that the
//! presence engine consumes over IPC. It knows nothing about particles,
//! springs, or force fields; the engine knows nothing about the AI. They meet
//! only at `PresenceState`.
//!
//! # Why it is pure
//!
//! This crate has no threads, no I/O, no `Sender`, and no clock. It is a
//! deterministic state machine: feed it events, read [`PresenceBrain::state`].
//! That is what lets it live in the headless root workspace with real unit
//! tests, per the repo's testing rule and `docs/PRESENCE_ENGINE_ROADMAP.md`
//! §Validation. The `desktop-edge` glue (`presence_brain.rs`) owns the wiring
//! that this crate deliberately does not: the `Sender<Envelope>`, the timed
//! pulse releases, and the RAII guards for sustained engagements.
//!
//! # Concurrency model
//!
//! Sustained engagements (`thinking`, `tool_use`) are **reference counted**,
//! not boolean. Two overlapping requests both raise `thinking`; the mode only
//! releases when the last one finishes. This is the invariant that ADR-012's
//! "modes are a set" composition needs and that the scattered `hold_mode`
//! call sites in the shell were approximating by hand.
//!
//! # Cognition mapping
//!
//! Some fields are stored and set directly by events (`confidence`,
//! `uncertainty`, `task_complexity`, the raw signals); others are *derived* at
//! read time from the stored state (`curiosity`, `focus`, `memory_activity`,
//! `emotional_tone`). Deriving keeps those four internally consistent — they
//! cannot drift out of step with the events that should imply them — and keeps
//! the event API small. Every value that leaves the Brain is passed through
//! [`PresenceState::clamped`](presence_ipc::PresenceState::clamped), so a
//! caller can never push the engine out of range.

use presence_ipc::{PresenceMode, PresenceState};

/// Neutral starting confidence — the assistant is neither sure nor unsure
/// before it has done anything. Requests move it up on success, down on
/// failure.
const CONFIDENCE_BASELINE: f32 = 0.5;

/// Resting `intensity`, mirroring the shell's idle brightness default (the mic
/// and speaking pumps already publish this figure). Kept here so a Brain that
/// has never been told an intensity still reports the same idle level the rest
/// of the shell expects rather than a dead `0.0`.
const IDLE_INTENSITY: f32 = 0.15;

/// How far a single request outcome nudges `confidence` / `uncertainty`. Small
/// enough that one result does not slam the value to an extreme, so a run of
/// successes reads as growing assurance rather than a binary flip.
const OUTCOME_STEP: f32 = 0.2;
const UNCERTAINTY_STEP: f32 = 0.3;

/// Maps abstract cognitive lifecycle events into a bounded [`PresenceState`].
///
/// Construct with [`PresenceBrain::new`], mutate via the event methods, and
/// read the current snapshot with [`PresenceBrain::state`]. All methods are
/// infallible and clamp their inputs; the type cannot be driven into an
/// invalid state.
#[derive(Clone, Debug, PartialEq)]
pub struct PresenceBrain {
    /// In-flight request count. `thinking` is engaged while this is > 0.
    thinking: u32,
    /// In-flight tool-call count. `tool_use` is engaged while this is > 0.
    tool_use: u32,
    /// Latest debounced VAD verdict from the mic pump.
    listening: bool,
    /// True while TTS / a streamed response is being voiced.
    speaking: bool,
    /// Transient "look here" glance (notifications, inbound streams).
    attention: bool,
    /// Brief self-clearing error flash.
    error: bool,

    audio_level: f32,
    progress: f32,
    intensity: f32,
    cursor_dir: [f32; 2],
    cursor_proximity: f32,

    confidence: f32,
    uncertainty: f32,
    task_complexity: f32,
}

impl Default for PresenceBrain {
    fn default() -> Self {
        Self::new()
    }
}

impl PresenceBrain {
    /// A resting Brain: nothing engaged, neutral confidence, idle intensity.
    /// Equivalent to the shell's idle presence state.
    pub fn new() -> Self {
        Self {
            thinking: 0,
            tool_use: 0,
            listening: false,
            speaking: false,
            attention: false,
            error: false,
            audio_level: 0.0,
            progress: 0.0,
            intensity: IDLE_INTENSITY,
            cursor_dir: [0.0, 0.0],
            cursor_proximity: 0.0,
            confidence: CONFIDENCE_BASELINE,
            uncertainty: 0.0,
            task_complexity: 0.0,
        }
    }

    // --- Request lifecycle --------------------------------------------------

    /// A request to the assistant began. Engages `thinking` (reference
    /// counted), records the caller's difficulty estimate as `task_complexity`,
    /// and raises `uncertainty` because the outcome is not yet known.
    ///
    /// `complexity` is the caller's `[0,1]` estimate of how hard the request
    /// is (token budget, tool fan-out, model tier). Pass `0.5` when unknown.
    pub fn request_started(&mut self, complexity: f32) {
        self.thinking = self.thinking.saturating_add(1);
        self.task_complexity = sanitize_unit(complexity).max(self.task_complexity);
        self.uncertainty = (self.uncertainty + UNCERTAINTY_STEP).min(1.0);
    }

    /// A request finished, releasing one `thinking` reference. Deliberately
    /// does **not** touch `confidence`/`uncertainty` — the outcome is reported
    /// separately via [`note_success`](Self::note_success) /
    /// [`note_failure`](Self::note_failure), because a scope-owned engagement
    /// guard drops without knowing whether the work succeeded, while the call
    /// site that *does* know reports the outcome explicitly.
    pub fn request_ended(&mut self) {
        self.thinking = self.thinking.saturating_sub(1);
        // With nothing left in flight there is no progress to report.
        if self.thinking == 0 {
            self.progress = 0.0;
        }
    }

    /// Records a successful outcome: confidence up, uncertainty down.
    pub fn note_success(&mut self) {
        self.confidence = (self.confidence + OUTCOME_STEP).min(1.0);
        self.uncertainty = (self.uncertainty - UNCERTAINTY_STEP).max(0.0);
    }

    /// Records a failed outcome: confidence down, uncertainty up, and a
    /// self-clearing `error` flash the shell glue schedules the clear for.
    pub fn note_failure(&mut self) {
        self.confidence = (self.confidence - UNCERTAINTY_STEP).max(0.0);
        self.uncertainty = (self.uncertainty + OUTCOME_STEP).min(1.0);
        self.error = true;
    }

    // --- Tool lifecycle -----------------------------------------------------

    /// A tool call started. Engages `tool_use` (reference counted). Overlapping
    /// tool calls stack; the mode holds until the last one finishes.
    pub fn tool_started(&mut self) {
        self.tool_use = self.tool_use.saturating_add(1);
    }

    /// A tool call finished. Releases one `tool_use` reference.
    pub fn tool_finished(&mut self) {
        self.tool_use = self.tool_use.saturating_sub(1);
    }

    // --- Streaming / speech -------------------------------------------------

    /// Sets whether the assistant is currently voicing a response.
    pub fn set_speaking(&mut self, speaking: bool) {
        self.speaking = speaking;
    }

    /// Sets the debounced VAD listening verdict from the mic pump.
    pub fn set_listening(&mut self, listening: bool) {
        self.listening = listening;
    }

    /// Raises or clears the transient attention glance.
    pub fn set_attention(&mut self, on: bool) {
        self.attention = on;
    }

    /// Raises or clears the error flash.
    pub fn set_error(&mut self, on: bool) {
        self.error = on;
    }

    /// Absolutely sets a mode's engagement, overriding any reference count.
    /// This is the authoritative-snapshot path used by the shell's dev panel
    /// (`SetSignals` / `SetMode`), as opposed to the reference-counted
    /// [`request_started`](Self::request_started) / [`tool_started`](Self::tool_started)
    /// lifecycle used by real work. For the counted modes it forces the count
    /// to `0` or `1`; mixing the two paths on the same mode is a debug-only
    /// situation and the absolute set intentionally wins.
    pub fn set_engaged(&mut self, mode: PresenceMode, engaged: bool) {
        let count = u32::from(engaged);
        match mode {
            PresenceMode::Thinking => self.thinking = count,
            PresenceMode::ToolUse => self.tool_use = count,
            PresenceMode::Listening => self.listening = engaged,
            PresenceMode::Speaking => self.speaking = engaged,
            PresenceMode::Attention => self.attention = engaged,
            PresenceMode::Error => self.error = engaged,
        }
    }

    // --- Continuous signals -------------------------------------------------

    /// Sets the audio envelope (mic RMS while listening, or TTS RMS while
    /// speaking). Clamped to `[0,1]`; NaN folds to `0`.
    pub fn set_audio_level(&mut self, level: f32) {
        self.audio_level = sanitize_unit(level);
    }

    /// Sets task progress `[0,1]` (e.g. streamed-token fraction).
    pub fn set_progress(&mut self, progress: f32) {
        self.progress = sanitize_unit(progress);
    }

    /// Overrides the overall intensity `[0,1.5]`. Rarely needed — most callers
    /// leave it at the idle baseline and let audio/modes carry energy.
    pub fn set_intensity(&mut self, intensity: f32) {
        self.intensity = if intensity.is_nan() {
            0.0
        } else {
            intensity.clamp(0.0, 1.5)
        };
    }

    /// Updates cursor awareness. `dir` is a screen-space bias toward the cursor
    /// (each component `[-1,1]`); `proximity` is `1.0` on top of the droplet,
    /// falling to `0.0` far away.
    pub fn set_cursor(&mut self, dir: [f32; 2], proximity: f32) {
        self.cursor_dir = [sanitize_signed(dir[0]), sanitize_signed(dir[1])];
        self.cursor_proximity = sanitize_unit(proximity);
    }

    // --- Read side ----------------------------------------------------------

    /// True while any sustained mode is engaged or a signal is active — i.e.
    /// the presence is not at rest. Handy for the shell to decide whether to
    /// keep a high-rate pump running.
    pub fn is_active(&self) -> bool {
        self.thinking > 0
            || self.tool_use > 0
            || self.listening
            || self.speaking
            || self.attention
            || self.error
    }

    /// The current bounded snapshot to send over IPC. Derived fields
    /// (`curiosity`, `focus`, `memory_activity`, `emotional_tone`) are computed
    /// here so they cannot drift from the events that imply them. The result is
    /// always [`PresenceState::clamped`].
    pub fn state(&self) -> PresenceState {
        let thinking = bool_level(self.thinking > 0);
        let tool_use = bool_level(self.tool_use > 0);
        let listening = bool_level(self.listening);
        let speaking = bool_level(self.speaking);
        let attention = bool_level(self.attention);
        let error = bool_level(self.error);

        // Curiosity: outward, exploratory attention — highest while taking in
        // input (listening) or turning a fresh request over (thinking), and
        // damped once the assistant commits to acting through a tool.
        let curiosity = (0.6 * listening + 0.5 * thinking) * (1.0 - 0.5 * tool_use);

        // Focus: narrowing onto a target — the cursor when it is near, or the
        // act of running a tool. Attention nudges it up.
        let focus = cursor_focus(self.cursor_proximity)
            .max(0.8 * tool_use)
            .max(0.4 * attention);

        // Memory activity: retrieval/consolidation, strongest during tool use.
        let memory_activity = (0.7 * tool_use).max(0.2 * thinking);

        // Emotional tone as affective *energy*: how animated the presence is.
        // Voice and audio dominate; thinking and attention add a little.
        let emotional_tone = 0.5 * speaking
            + 0.4 * listening
            + 0.3 * thinking
            + 0.5 * self.audio_level
            + 0.2 * attention;

        PresenceState {
            thinking,
            speaking,
            tool_use,
            listening,
            attention,
            error,
            confidence: self.confidence,
            curiosity,
            uncertainty: self.uncertainty,
            focus,
            task_complexity: self.task_complexity,
            memory_activity,
            emotional_tone,
            intensity: self.intensity,
            audio_level: self.audio_level,
            progress: self.progress,
            cursor_dir: self.cursor_dir,
            cursor_proximity: self.cursor_proximity,
        }
        .clamped()
    }
}

/// Boolean engagement as a `0.0`/`1.0` activation level. The engine's Behavior
/// Graph (M3) will eventually receive true continuous levels; until then the
/// Brain speaks the same on/off grammar the modes always used.
fn bool_level(on: bool) -> f32 {
    if on {
        1.0
    } else {
        0.0
    }
}

/// Cursor proximity shaped into a focus contribution. Linear for now; a curve
/// can go here later without touching callers.
fn cursor_focus(proximity: f32) -> f32 {
    proximity.clamp(0.0, 1.0)
}

fn sanitize_unit(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

fn sanitize_signed(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use presence_ipc::PresenceMode;

    #[test]
    fn new_brain_is_idle_with_neutral_confidence() {
        let s = PresenceBrain::new().state();
        assert_eq!(s.thinking, 0.0);
        assert_eq!(s.tool_use, 0.0);
        assert_eq!(s.speaking, 0.0);
        assert_eq!(s.confidence, CONFIDENCE_BASELINE);
        assert_eq!(s.uncertainty, 0.0);
        assert_eq!(s.intensity, IDLE_INTENSITY);
        assert!(!PresenceBrain::new().is_active());
    }

    #[test]
    fn thinking_is_reference_counted_across_overlapping_requests() {
        let mut brain = PresenceBrain::new();
        brain.request_started(0.5);
        brain.request_started(0.5);
        assert_eq!(brain.state().thinking, 1.0, "engaged while any in flight");
        brain.request_ended();
        assert_eq!(
            brain.state().thinking,
            1.0,
            "still engaged: one request remains"
        );
        brain.request_ended();
        assert_eq!(brain.state().thinking, 0.0, "released when last finishes");
        assert!(!brain.is_active());
    }

    #[test]
    fn outcome_notes_move_confidence_and_uncertainty() {
        let mut brain = PresenceBrain::new();
        brain.request_started(0.5);
        let after_start = brain.state();
        assert!(
            after_start.uncertainty > 0.0,
            "unknown outcome raises uncertainty"
        );

        let mut success = brain.clone();
        success.note_success();
        success.request_ended();
        assert!(success.state().confidence > CONFIDENCE_BASELINE);

        let mut failure = brain.clone();
        failure.note_failure();
        failure.request_ended();
        let f = failure.state();
        assert!(f.confidence < CONFIDENCE_BASELINE);
        assert!(f.error > 0.0, "a failed request raises the error flash");
    }

    #[test]
    fn set_engaged_is_absolute_over_the_reference_count() {
        let mut brain = PresenceBrain::new();
        brain.request_started(0.5);
        brain.request_started(0.5);
        brain.set_engaged(PresenceMode::Thinking, false);
        assert_eq!(brain.state().thinking, 0.0, "absolute set overrides count");
        brain.set_engaged(PresenceMode::Speaking, true);
        assert_eq!(brain.state().speaking, 1.0);
    }

    #[test]
    fn tool_use_is_reference_counted() {
        let mut brain = PresenceBrain::new();
        brain.tool_started();
        brain.tool_started();
        assert_eq!(brain.state().tool_use, 1.0);
        brain.tool_finished();
        assert_eq!(brain.state().tool_use, 1.0);
        brain.tool_finished();
        assert_eq!(brain.state().tool_use, 0.0);
        // Underflow is saturating, never a panic.
        brain.tool_finished();
        assert_eq!(brain.state().tool_use, 0.0);
    }

    #[test]
    fn tool_use_drives_focus_and_memory_activity() {
        let mut brain = PresenceBrain::new();
        brain.tool_started();
        let s = brain.state();
        assert!(
            s.focus > 0.5,
            "running a tool narrows focus, got {}",
            s.focus
        );
        assert!(s.memory_activity > 0.5, "tool use lights up memory");
    }

    #[test]
    fn cursor_proximity_drives_focus_and_is_carried_through() {
        let mut brain = PresenceBrain::new();
        brain.set_cursor([0.5, -0.5], 0.9);
        let s = brain.state();
        assert_eq!(s.cursor_dir, [0.5, -0.5]);
        assert_eq!(s.cursor_proximity, 0.9);
        assert!(s.focus >= 0.9, "near cursor focuses the presence");
    }

    #[test]
    fn speaking_and_audio_raise_emotional_energy() {
        let mut brain = PresenceBrain::new();
        let calm = brain.state().emotional_tone;
        brain.set_speaking(true);
        brain.set_audio_level(0.8);
        assert!(
            brain.state().emotional_tone > calm,
            "voice + audio should animate the presence"
        );
    }

    #[test]
    fn listening_maps_to_the_listening_mode_and_curiosity() {
        let mut brain = PresenceBrain::new();
        brain.set_listening(true);
        let s = brain.state();
        assert_eq!(s.listening, 1.0);
        assert!(s.curiosity > 0.0, "taking in input is curious");
    }

    #[test]
    fn signals_are_clamped_on_the_way_out() {
        let mut brain = PresenceBrain::new();
        brain.set_audio_level(9.0);
        brain.set_progress(-1.0);
        brain.set_cursor([5.0, -5.0], 2.0);
        brain.set_intensity(f32::NAN);
        let s = brain.state();
        assert_eq!(s.audio_level, 1.0);
        assert_eq!(s.progress, 0.0);
        assert_eq!(s.cursor_dir, [1.0, -1.0]);
        assert_eq!(s.cursor_proximity, 1.0);
        assert_eq!(s.intensity, 0.0);
    }

    #[test]
    fn mode_levels_thresholds_match_the_engaged_modes() {
        // The snapshot the Brain emits must engage exactly the modes it has
        // active, under the >= 0.5 threshold the core adapter uses.
        let mut brain = PresenceBrain::new();
        brain.request_started(0.5);
        brain.tool_started();
        let s = brain.state();
        let engaged: Vec<PresenceMode> = s
            .mode_levels()
            .into_iter()
            .filter(|(_, level)| *level >= 0.5)
            .map(|(mode, _)| mode)
            .collect();
        assert!(engaged.contains(&PresenceMode::Thinking));
        assert!(engaged.contains(&PresenceMode::ToolUse));
        assert!(!engaged.contains(&PresenceMode::Speaking));
    }
}
