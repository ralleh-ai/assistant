//! Shell-side wiring around the headless [`presence_brain::PresenceBrain`].
//!
//! ADR-014 puts the cognition layer in `desktop-edge`: it is the only part of
//! the system that knows the assistant exists, and it maps real AI/audio/cursor
//! lifecycle into a single bounded [`PresenceState`] that the presence engine
//! consumes over IPC.
//!
//! The pure state machine lives in the `presence-brain` crate (headless, unit
//! tested in root CI). This module is the glue the crate deliberately omits:
//!
//! - a shared, `Clone`, `Send` [`PresenceBrainHandle`] that background pumps
//!   (mic, TTS, scan sweep) and the Tauri command threads all hold;
//! - emission — every mutation is followed by exactly one
//!   [`Command::SetPresenceState`] onto the writer channel, so cognition
//!   travels the wire as *one authoritative snapshot* rather than the older mix
//!   of `SetMode` / `SetSignals` / `SetSignalsScalars` (which would fight an
//!   authoritative snapshot if they coexisted);
//! - timed pulses — the self-clearing error / speaking / attention flashes,
//!   whose release is scheduled on a detached thread (this crate predates any
//!   async runtime, same as the old `Presence::pulse_mode`).
//!
//! [`Presence`](crate::presence::Presence) owns one handle and routes all of
//! its cognition methods through it; the transport (`presence::writer_loop`)
//! and non-cognitive commands (palette, quality, position, scenes) are
//! unchanged.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use presence_brain::PresenceBrain;
use presence_ipc::{Command, Envelope, PresenceMode};

/// Cloneable handle to the shell's single [`PresenceBrain`]. Every clone shares
/// the same brain (`Arc<Mutex<_>>`) and the same writer channel, so a mic-pump
/// thread and a Tauri command handler updating the brain concurrently produce a
/// consistent, serialized sequence of `SetPresenceState` snapshots.
#[derive(Clone)]
pub struct PresenceBrainHandle {
    brain: Arc<Mutex<PresenceBrain>>,
    tx: Sender<Envelope>,
}

impl PresenceBrainHandle {
    /// Wraps a fresh brain around the writer channel. One of these is created
    /// per spawned runtime; disabled presence has no handle at all.
    pub fn new(tx: Sender<Envelope>) -> Self {
        Self {
            brain: Arc::new(Mutex::new(PresenceBrain::new())),
            tx,
        }
    }

    /// Mutates the brain under the lock, then emits one `SetPresenceState` with
    /// the resulting snapshot. Emission happens after the lock is released so a
    /// slow channel never widens the critical section.
    fn update(&self, f: impl FnOnce(&mut PresenceBrain)) {
        let snapshot = {
            let Ok(mut brain) = self.brain.lock() else {
                // A poisoned lock means another thread panicked mid-update. The
                // presence is a visual nicety; drop the update rather than
                // propagate the panic into a Tauri command handler.
                return;
            };
            f(&mut brain);
            brain.state()
        };
        // Best-effort, like every other presence send: a dead writer thread
        // just means the child exited, and the next launch starts clean.
        let _ = self
            .tx
            .send(Envelope::wrap(Command::SetPresenceState(snapshot)));
    }

    // --- Reference-counted sustained engagements (real work) ----------------

    /// Engages a sustained mode. `Thinking` / `ToolUse` are reference counted
    /// so overlapping requests/tool calls compose; the others are boolean.
    pub fn engage(&self, mode: PresenceMode) {
        self.update(|b| match mode {
            PresenceMode::Thinking => b.request_started(0.5),
            PresenceMode::ToolUse => b.tool_started(),
            PresenceMode::Listening => b.set_listening(true),
            PresenceMode::Speaking => b.set_speaking(true),
            PresenceMode::Attention => b.set_attention(true),
            PresenceMode::Error => b.set_error(true),
        });
    }

    /// Releases a sustained mode previously engaged via [`Self::engage`].
    pub fn release(&self, mode: PresenceMode) {
        self.update(|b| match mode {
            PresenceMode::Thinking => b.request_ended(),
            PresenceMode::ToolUse => b.tool_finished(),
            PresenceMode::Listening => b.set_listening(false),
            PresenceMode::Speaking => b.set_speaking(false),
            PresenceMode::Attention => b.set_attention(false),
            PresenceMode::Error => b.set_error(false),
        });
    }

    /// Records a successful request outcome (confidence up).
    pub fn note_success(&self) {
        self.update(|b| b.note_success());
    }

    /// Records a failed request outcome (confidence down, error flash up).
    pub fn note_failure(&self) {
        self.update(|b| b.note_failure());
    }

    // --- Authoritative snapshot paths (dev panel) ---------------------------

    /// Absolutely sets a single mode's engagement (dev-panel `SetMode`).
    pub fn set_engaged(&self, mode: PresenceMode, engaged: bool) {
        self.update(|b| b.set_engaged(mode, engaged));
    }

    /// Applies a full authoritative `Signals` snapshot (dev-panel
    /// `SetSignals`): every listed mode engaged, every other released, scalars
    /// replaced.
    pub fn apply_signals(
        &self,
        intensity: f32,
        audio_level: f32,
        progress: f32,
        modes: &[PresenceMode],
    ) {
        self.update(|b| {
            b.set_intensity(intensity);
            b.set_audio_level(audio_level);
            b.set_progress(progress);
            for mode in [
                PresenceMode::Thinking,
                PresenceMode::Speaking,
                PresenceMode::ToolUse,
                PresenceMode::Listening,
                PresenceMode::Attention,
                PresenceMode::Error,
            ] {
                b.set_engaged(mode, modes.contains(&mode));
            }
        });
    }

    /// Scalars-only update (dev-panel / generic `SetSignalsScalars`).
    pub fn set_scalars(&self, intensity: f32, audio_level: f32, progress: f32) {
        self.update(|b| {
            b.set_intensity(intensity);
            b.set_audio_level(audio_level);
            b.set_progress(progress);
        });
    }

    // --- Pump-facing continuous updates -------------------------------------

    /// Sets the audio envelope (intensity + level) in one snapshot — the shape
    /// the mic and TTS pumps push at ~30 Hz.
    pub fn set_audio_envelope(&self, intensity: f32, audio_level: f32) {
        self.update(|b| {
            b.set_intensity(intensity);
            b.set_audio_level(audio_level);
        });
    }

    /// Sets the debounced VAD listening verdict (mic pump).
    pub fn set_listening(&self, listening: bool) {
        self.update(|b| b.set_listening(listening));
    }

    /// Updates cursor awareness. Plumbing for the low-rate cursor pump that
    /// lands in M7 (`docs/PRESENCE_ENGINE_ROADMAP.md`) — the Brain and wire
    /// already carry `cursor_dir`/`cursor_proximity`, so wiring the OS cursor
    /// sampler later is additive and needs no contract change.
    #[allow(dead_code)]
    pub fn set_cursor(&self, dir: [f32; 2], proximity: f32) {
        self.update(|b| b.set_cursor(dir, proximity));
    }

    // --- Timed pulses -------------------------------------------------------

    /// Fires a self-clearing error flash: records the failure immediately, then
    /// clears the error bit after `hold_ms` on a detached thread. Confidence /
    /// uncertainty moved by [`note_failure`](Self::note_failure) are *not*
    /// reverted — only the visual flash clears.
    pub fn pulse_error(&self, hold_ms: u64) {
        self.note_failure();
        self.schedule_clear(PresenceMode::Error, hold_ms);
    }

    /// Fires a mode for `hold_ms` then releases it (speaking / attention).
    pub fn pulse(&self, mode: PresenceMode, hold_ms: u64) {
        self.set_engaged(mode, true);
        self.schedule_clear(mode, hold_ms);
    }

    fn schedule_clear(&self, mode: PresenceMode, hold_ms: u64) {
        let this = self.clone();
        thread::Builder::new()
            .name(format!("presence-{mode:?}-pulse"))
            .spawn(move || {
                thread::sleep(Duration::from_millis(hold_ms));
                this.set_engaged(mode, false);
            })
            .ok();
    }

    // --- Read side ----------------------------------------------------------

    /// The modes the brain currently has engaged (level ≥ 0.5), sorted by
    /// label for the aria-live status line's deterministic order.
    pub fn current_modes(&self) -> Vec<PresenceMode> {
        let Ok(brain) = self.brain.lock() else {
            return Vec::new();
        };
        let mut out: Vec<PresenceMode> = brain
            .state()
            .mode_levels()
            .into_iter()
            .filter(|(_, level)| *level >= 0.5)
            .map(|(mode, _)| mode)
            .collect();
        out.sort_by_key(|m| m.label());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use presence_ipc::PresenceState;
    use std::sync::mpsc;

    /// Pull the latest `SetPresenceState` snapshot off the channel, ignoring
    /// any earlier ones. Returns `None` if none were sent.
    fn latest_state(rx: &mpsc::Receiver<Envelope>) -> Option<PresenceState> {
        let mut last = None;
        while let Ok(env) = rx.try_recv() {
            if let Command::SetPresenceState(s) = env.payload {
                last = Some(s);
            }
        }
        last
    }

    #[test]
    fn engage_emits_a_snapshot_with_the_mode_active() {
        let (tx, rx) = mpsc::channel();
        let h = PresenceBrainHandle::new(tx);
        h.engage(PresenceMode::Thinking);
        let s = latest_state(&rx).expect("a snapshot was emitted");
        assert_eq!(s.thinking, 1.0);
    }

    #[test]
    fn engage_release_is_reference_counted_on_the_wire() {
        let (tx, rx) = mpsc::channel();
        let h = PresenceBrainHandle::new(tx);
        h.engage(PresenceMode::ToolUse);
        h.engage(PresenceMode::ToolUse);
        h.release(PresenceMode::ToolUse);
        assert_eq!(
            latest_state(&rx).unwrap().tool_use,
            1.0,
            "still engaged: one reference remains"
        );
        h.release(PresenceMode::ToolUse);
        assert_eq!(latest_state(&rx).unwrap().tool_use, 0.0);
    }

    #[test]
    fn current_modes_reflects_engagements_sorted_by_label() {
        let (tx, _rx) = mpsc::channel();
        let h = PresenceBrainHandle::new(tx);
        h.engage(PresenceMode::Thinking);
        h.engage(PresenceMode::ToolUse);
        let modes = h.current_modes();
        assert!(modes.contains(&PresenceMode::Thinking));
        assert!(modes.contains(&PresenceMode::ToolUse));
        // Sorted by label: "thinking" < "tool_use".
        assert_eq!(modes, {
            let mut m = vec![PresenceMode::Thinking, PresenceMode::ToolUse];
            m.sort_by_key(|x| x.label());
            m
        });
    }

    #[test]
    fn apply_signals_is_authoritative_over_modes() {
        let (tx, rx) = mpsc::channel();
        let h = PresenceBrainHandle::new(tx);
        h.engage(PresenceMode::Thinking);
        // Snapshot mentions only Speaking -> Thinking must drop.
        h.apply_signals(0.6, 0.4, 0.0, &[PresenceMode::Speaking]);
        let s = latest_state(&rx).unwrap();
        assert_eq!(s.thinking, 0.0);
        assert_eq!(s.speaking, 1.0);
        assert!((s.intensity - 0.6).abs() < 1e-6);
    }

    #[test]
    fn note_failure_sets_error_and_lowers_confidence() {
        let (tx, rx) = mpsc::channel();
        let h = PresenceBrainHandle::new(tx);
        let baseline = {
            h.set_scalars(0.15, 0.0, 0.0);
            latest_state(&rx).unwrap().confidence
        };
        h.note_failure();
        let s = latest_state(&rx).unwrap();
        assert!(s.error > 0.0);
        assert!(s.confidence < baseline);
    }

    #[test]
    fn pulse_clears_after_the_hold() {
        let (tx, rx) = mpsc::channel();
        let h = PresenceBrainHandle::new(tx);
        h.pulse(PresenceMode::Attention, 20);
        assert_eq!(latest_state(&rx).unwrap().attention, 1.0);
        thread::sleep(Duration::from_millis(60));
        assert_eq!(
            latest_state(&rx).unwrap().attention,
            0.0,
            "the scheduled clear releases the mode"
        );
    }

    #[test]
    fn audio_envelope_updates_level_without_touching_modes() {
        let (tx, rx) = mpsc::channel();
        let h = PresenceBrainHandle::new(tx);
        h.engage(PresenceMode::Speaking);
        h.set_audio_envelope(0.4, 0.7);
        let s = latest_state(&rx).unwrap();
        assert_eq!(s.speaking, 1.0, "envelope must not release the mode");
        assert!((s.audio_level - 0.7).abs() < 1e-6);
    }
}
