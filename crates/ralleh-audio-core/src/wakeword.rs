use serde::{Deserialize, Serialize};

use crate::source::AudioFrame;
use crate::vad::{VadConfig, VadState, VoiceActivityDetector};

/// Configuration for wake-word trigger detection.
///
/// This is deliberately a lightweight, testable heuristic layered on top of
/// the VAD, not a trained keyword-spotting model. It exists to validate the
/// *pipeline contract* (VAD confirms speech -> candidate utterance window
/// opens -> "matcher" decides hit/miss -> debounced trigger event fires)
/// before a real wake-word engine (Porcupine / openWakeWord, per
/// DEVELOPMENT.md open items) is swapped in behind the same trait boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WakeWordConfig {
    pub vad: VadConfig,
    /// Minimum number of confirmed-speech frames an utterance must contain
    /// before it's even considered as a wake-word candidate. Filters out
    /// utterances too short to plausibly contain "Ralleh" (2 syllables).
    pub min_utterance_frames: u32,
    /// Maximum number of confirmed-speech frames an utterance may contain
    /// and still be considered a wake-word candidate (as opposed to a
    /// longer command/sentence that happens to start with a similar sound).
    pub max_utterance_frames: u32,
    /// Cooldown: number of frames that must elapse after a trigger fires
    /// before another trigger can fire. Prevents a single sustained
    /// utterance from firing multiple triggers.
    pub cooldown_frames: u32,
}

impl Default for WakeWordConfig {
    fn default() -> Self {
        Self {
            vad: VadConfig::default(),
            min_utterance_frames: 2,
            max_utterance_frames: 12,
            cooldown_frames: 5,
        }
    }
}

/// Pluggable matcher: given a completed utterance (sequence of frames that
/// the VAD confirmed as one contiguous speech segment), decide whether it
/// matches the wake word.
///
/// `MockWakeWordMatcher` (test-only, energy/length heuristic) stands in for
/// this today. A real implementation backed by Porcupine or openWakeWord
/// implements the same trait later — nothing above this boundary changes.
pub trait WakeWordMatcher {
    fn is_match(&self, utterance_frames: &[AudioFrame]) -> bool;
}

/// Test/dev matcher: treats any utterance within the configured frame-count
/// bounds as a match. This is intentionally simplistic — it exists to
/// validate the *state machine* around triggering (debounce, cooldown,
/// utterance windowing), not to actually perform keyword spotting. Real
/// acoustic matching is out of scope until a production engine is wired in.
#[derive(Debug, Clone, Default)]
pub struct MockWakeWordMatcher {
    /// If `Some`, only utterances with exactly this many frames match.
    /// Lets tests simulate both "sounds like Ralleh" and "doesn't."
    pub only_match_frame_count: Option<usize>,
}

impl WakeWordMatcher for MockWakeWordMatcher {
    fn is_match(&self, utterance_frames: &[AudioFrame]) -> bool {
        match self.only_match_frame_count {
            Some(n) => utterance_frames.len() == n,
            None => true,
        }
    }
}

/// A detected wake-word trigger event.
#[derive(Debug, Clone, PartialEq)]
pub struct WakeWordTrigger {
    /// Sequence number of the frame that completed the matched utterance.
    pub triggered_at_sequence: u64,
    /// Number of frames in the matched utterance.
    pub utterance_frame_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetectorPhase {
    Idle,
    Collecting,
    Cooldown,
}

/// Wake-word detection pipeline: VAD -> utterance windowing -> matcher ->
/// debounced trigger. Fully deterministic and testable against
/// `MockAudioSource`-produced frames; no live audio or trained model
/// required to validate the state machine's correctness.
pub struct WakeWordDetector<M: WakeWordMatcher> {
    config: WakeWordConfig,
    vad: VoiceActivityDetector,
    matcher: M,
    phase: DetectorPhase,
    current_utterance: Vec<AudioFrame>,
    cooldown_remaining: u32,
}

impl<M: WakeWordMatcher> WakeWordDetector<M> {
    pub fn new(config: WakeWordConfig, matcher: M) -> Self {
        let vad = VoiceActivityDetector::new(config.vad.clone());
        Self {
            config,
            vad,
            matcher,
            phase: DetectorPhase::Idle,
            current_utterance: Vec::new(),
            cooldown_remaining: 0,
        }
    }

    /// Feed one frame into the detector. Returns `Some(trigger)` exactly on
    /// the frame where a wake-word match is confirmed; `None` otherwise.
    pub fn process_frame(&mut self, frame: AudioFrame) -> Option<WakeWordTrigger> {
        if self.phase == DetectorPhase::Cooldown {
            // Still feed the VAD so its internal state stays consistent,
            // but ignore its output for triggering purposes during cooldown.
            let vad_state = self.vad.process_frame(&frame);

            // Only let the cooldown clock run down during quiet frames.
            // If speech resumes immediately after a trigger, we hold in
            // cooldown until that utterance finishes and silence returns,
            // so a single drawn-out speaker can't cause a second trigger
            // by simply continuing to talk through the cooldown window.
            if vad_state == VadState::Silence {
                self.cooldown_remaining = self.cooldown_remaining.saturating_sub(1);
                if self.cooldown_remaining == 0 {
                    self.phase = DetectorPhase::Idle;
                }
            }
            return None;
        }

        let vad_state = self.vad.process_frame(&frame);

        match vad_state {
            VadState::Speech => {
                if self.phase == DetectorPhase::Idle {
                    self.phase = DetectorPhase::Collecting;
                    self.current_utterance.clear();
                }
                self.current_utterance.push(frame);
                None
            }
            VadState::MaybeSilence if self.phase == DetectorPhase::Collecting => {
                // Still mid-utterance (VAD is debouncing the end); keep
                // collecting frames.
                self.current_utterance.push(frame);
                None
            }
            VadState::Silence if self.phase == DetectorPhase::Collecting => {
                // Utterance just ended. Evaluate it.
                let frame_count = self.current_utterance.len() as u32;
                self.phase = DetectorPhase::Idle;

                let in_bounds = frame_count >= self.config.min_utterance_frames
                    && frame_count <= self.config.max_utterance_frames;

                let result = if in_bounds && self.matcher.is_match(&self.current_utterance) {
                    let trigger = WakeWordTrigger {
                        triggered_at_sequence: frame.sequence,
                        utterance_frame_count: self.current_utterance.len(),
                    };
                    self.phase = DetectorPhase::Cooldown;
                    self.cooldown_remaining = self.config.cooldown_frames;
                    Some(trigger)
                } else {
                    None
                };

                self.current_utterance.clear();
                result
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{AudioSource, MockAudioSource};

    fn frames_silence(count: usize, start_seq: u64) -> Vec<AudioFrame> {
        let mut src = MockAudioSource::new();
        for i in 0..count {
            src.push_silence(16_000, 320, start_seq + i as u64);
        }
        let mut out = Vec::new();
        while let Some(f) = src.next_frame() {
            out.push(f);
        }
        out
    }

    fn frames_speech(count: usize, start_seq: u64) -> Vec<AudioFrame> {
        let mut src = MockAudioSource::new();
        for i in 0..count {
            src.push_speech(16_000, 320, 0.5, start_seq + i as u64);
        }
        let mut out = Vec::new();
        while let Some(f) = src.next_frame() {
            out.push(f);
        }
        out
    }

    #[test]
    fn matching_utterance_within_bounds_triggers() {
        let mut detector =
            WakeWordDetector::new(WakeWordConfig::default(), MockWakeWordMatcher::default());

        let mut trigger = None;
        for f in frames_silence(2, 0) {
            trigger = detector.process_frame(f).or(trigger);
        }
        for f in frames_speech(4, 2) {
            trigger = detector.process_frame(f).or(trigger);
        }
        // enough silence to confirm end-of-utterance (silence_confirm_frames = 3)
        for f in frames_silence(3, 6) {
            trigger = detector.process_frame(f).or(trigger);
        }

        assert!(trigger.is_some(), "expected a wake-word trigger to fire");
    }

    #[test]
    fn utterance_too_short_does_not_trigger() {
        let config = WakeWordConfig {
            min_utterance_frames: 5,
            ..WakeWordConfig::default()
        };
        let mut detector = WakeWordDetector::new(config, MockWakeWordMatcher::default());

        let mut trigger = None;
        for f in frames_silence(2, 0) {
            trigger = detector.process_frame(f).or(trigger);
        }
        // Only 2 confirmed speech frames after debounce -> below min of 5.
        for f in frames_speech(2, 2) {
            trigger = detector.process_frame(f).or(trigger);
        }
        for f in frames_silence(3, 4) {
            trigger = detector.process_frame(f).or(trigger);
        }

        assert!(
            trigger.is_none(),
            "utterance below min length must not trigger"
        );
    }

    #[test]
    fn utterance_too_long_does_not_trigger() {
        let config = WakeWordConfig {
            max_utterance_frames: 3,
            ..WakeWordConfig::default()
        };
        let mut detector = WakeWordDetector::new(config, MockWakeWordMatcher::default());

        let mut trigger = None;
        for f in frames_silence(2, 0) {
            trigger = detector.process_frame(f).or(trigger);
        }
        for f in frames_speech(6, 2) {
            trigger = detector.process_frame(f).or(trigger);
        }
        for f in frames_silence(3, 8) {
            trigger = detector.process_frame(f).or(trigger);
        }

        assert!(
            trigger.is_none(),
            "utterance above max length must not trigger"
        );
    }

    #[test]
    fn non_matching_acoustic_pattern_does_not_trigger() {
        // Matcher configured to only accept exactly 99 frames — our
        // utterance won't match, simulating "this speech isn't the wake word."
        let matcher = MockWakeWordMatcher {
            only_match_frame_count: Some(99),
        };
        let mut detector = WakeWordDetector::new(WakeWordConfig::default(), matcher);

        let mut trigger = None;
        for f in frames_silence(2, 0) {
            trigger = detector.process_frame(f).or(trigger);
        }
        for f in frames_speech(4, 2) {
            trigger = detector.process_frame(f).or(trigger);
        }
        for f in frames_silence(3, 6) {
            trigger = detector.process_frame(f).or(trigger);
        }

        assert!(trigger.is_none(), "non-matching utterance must not trigger");
    }

    #[test]
    fn cooldown_prevents_immediate_retrigger() {
        let config = WakeWordConfig {
            cooldown_frames: 3,
            ..WakeWordConfig::default()
        };
        let mut detector = WakeWordDetector::new(config, MockWakeWordMatcher::default());

        let mut triggers = Vec::new();
        let mut seq = 0u64;

        // First utterance -> should trigger.
        for f in frames_silence(2, seq) {
            if let Some(t) = detector.process_frame(f) {
                triggers.push(t);
            }
        }
        seq += 2;
        for f in frames_speech(4, seq) {
            if let Some(t) = detector.process_frame(f) {
                triggers.push(t);
            }
        }
        seq += 4;
        for f in frames_silence(3, seq) {
            if let Some(t) = detector.process_frame(f) {
                triggers.push(t);
            }
        }
        seq += 3;

        assert_eq!(
            triggers.len(),
            1,
            "first utterance should trigger exactly once"
        );

        // Immediately followed by another speech utterance, while still
        // within cooldown -- must NOT trigger again yet.
        for f in frames_speech(4, seq) {
            if let Some(t) = detector.process_frame(f) {
                triggers.push(t);
            }
        }
        seq += 4;
        for f in frames_silence(3, seq) {
            if let Some(t) = detector.process_frame(f) {
                triggers.push(t);
            }
        }

        assert_eq!(
            triggers.len(),
            1,
            "second utterance during cooldown must not produce a second trigger"
        );
    }

    #[test]
    fn trigger_after_cooldown_expires_is_allowed() {
        let config = WakeWordConfig {
            cooldown_frames: 2,
            ..WakeWordConfig::default()
        };
        let mut detector = WakeWordDetector::new(config, MockWakeWordMatcher::default());
        let mut triggers = Vec::new();
        let mut seq = 0u64;

        // First trigger.
        for f in frames_silence(2, seq) {
            if let Some(t) = detector.process_frame(f) {
                triggers.push(t);
            }
        }
        seq += 2;
        for f in frames_speech(4, seq) {
            if let Some(t) = detector.process_frame(f) {
                triggers.push(t);
            }
        }
        seq += 4;
        for f in frames_silence(3, seq) {
            if let Some(t) = detector.process_frame(f) {
                triggers.push(t);
            }
        }
        seq += 3;
        assert_eq!(triggers.len(), 1);

        // Burn through the cooldown with plain silence frames.
        for f in frames_silence(5, seq) {
            if let Some(t) = detector.process_frame(f) {
                triggers.push(t);
            }
        }
        seq += 5;

        // New utterance after cooldown has expired -> should trigger again.
        for f in frames_speech(4, seq) {
            if let Some(t) = detector.process_frame(f) {
                triggers.push(t);
            }
        }
        seq += 4;
        for f in frames_silence(3, seq) {
            if let Some(t) = detector.process_frame(f) {
                triggers.push(t);
            }
        }

        assert_eq!(
            triggers.len(),
            2,
            "a new valid utterance after cooldown expires should trigger again"
        );
    }
}
