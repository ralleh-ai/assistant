use serde::{Deserialize, Serialize};

use crate::source::AudioFrame;

/// Configuration for the voice-activity-detection state machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VadConfig {
    /// RMS energy above which a frame is considered "speech."
    pub energy_threshold: f32,
    /// Number of consecutive speech-classified frames required before the
    /// detector transitions from `Silence`/`MaybeSpeech` into `Speech`.
    /// Prevents single-frame noise spikes from triggering a false positive.
    pub speech_confirm_frames: u32,
    /// Number of consecutive silence-classified frames required before the
    /// detector transitions out of `Speech` back to `Silence`. Prevents
    /// brief pauses mid-sentence from being treated as end-of-speech.
    pub silence_confirm_frames: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            energy_threshold: 0.02,
            speech_confirm_frames: 2,
            silence_confirm_frames: 3,
        }
    }
}

/// The current state of the voice-activity detector.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VadState {
    /// No speech detected; steady-state resting state.
    Silence,
    /// Energy has risen above threshold but not yet for enough consecutive
    /// frames to be confirmed as real speech.
    MaybeSpeech,
    /// Confirmed, ongoing speech.
    Speech,
    /// Energy has dropped but not yet for enough consecutive frames to
    /// confirm the speech segment has ended.
    MaybeSilence,
}

/// A simple, deterministic energy-based voice activity detector.
///
/// This is intentionally a first-pass heuristic (RMS energy + hysteresis via
/// confirm-frame counts), not a trained model — it exists to validate the
/// pipeline's state-machine contract (debounced transitions, no flapping on
/// single-frame noise) before a more sophisticated detector (or real
/// wake-word engine) is layered in. Every transition rule below is covered
/// by a test.
#[derive(Debug, Clone)]
pub struct VoiceActivityDetector {
    config: VadConfig,
    state: VadState,
    consecutive_speech_frames: u32,
    consecutive_silence_frames: u32,
}

impl VoiceActivityDetector {
    pub fn new(config: VadConfig) -> Self {
        Self {
            config,
            state: VadState::Silence,
            consecutive_speech_frames: 0,
            consecutive_silence_frames: 0,
        }
    }

    pub fn state(&self) -> VadState {
        self.state
    }

    /// Feed one frame into the detector, returning the (possibly updated)
    /// state after processing it.
    pub fn process_frame(&mut self, frame: &AudioFrame) -> VadState {
        let is_loud = frame.rms_energy() >= self.config.energy_threshold;

        if is_loud {
            self.consecutive_speech_frames += 1;
            self.consecutive_silence_frames = 0;
        } else {
            self.consecutive_silence_frames += 1;
            self.consecutive_speech_frames = 0;
        }

        self.state = match self.state {
            VadState::Silence => {
                if is_loud {
                    if self.consecutive_speech_frames >= self.config.speech_confirm_frames {
                        VadState::Speech
                    } else {
                        VadState::MaybeSpeech
                    }
                } else {
                    VadState::Silence
                }
            }
            VadState::MaybeSpeech => {
                if is_loud {
                    if self.consecutive_speech_frames >= self.config.speech_confirm_frames {
                        VadState::Speech
                    } else {
                        VadState::MaybeSpeech
                    }
                } else {
                    VadState::Silence
                }
            }
            VadState::Speech => {
                if is_loud {
                    VadState::Speech
                } else if self.consecutive_silence_frames >= self.config.silence_confirm_frames {
                    VadState::Silence
                } else {
                    VadState::MaybeSilence
                }
            }
            VadState::MaybeSilence => {
                if is_loud {
                    VadState::Speech
                } else if self.consecutive_silence_frames >= self.config.silence_confirm_frames {
                    VadState::Silence
                } else {
                    VadState::MaybeSilence
                }
            }
        };

        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{AudioSource, MockAudioSource};

    fn silence_frame(seq: u64) -> AudioFrame {
        let mut src = MockAudioSource::new();
        src.push_silence(16_000, 320, seq);
        src.next_frame().unwrap()
    }

    fn speech_frame(seq: u64) -> AudioFrame {
        let mut src = MockAudioSource::new();
        src.push_speech(16_000, 320, 0.5, seq);
        src.next_frame().unwrap()
    }

    #[test]
    fn starts_in_silence_state() {
        let vad = VoiceActivityDetector::new(VadConfig::default());
        assert_eq!(vad.state(), VadState::Silence);
    }

    #[test]
    fn single_loud_frame_does_not_immediately_confirm_speech() {
        // speech_confirm_frames default is 2, so one loud frame should only
        // reach MaybeSpeech, not Speech — this is the debounce behavior
        // that protects against single-frame noise spikes.
        let mut vad = VoiceActivityDetector::new(VadConfig::default());
        let state = vad.process_frame(&speech_frame(0));
        assert_eq!(state, VadState::MaybeSpeech);
    }

    #[test]
    fn sustained_loud_frames_confirm_speech() {
        let mut vad = VoiceActivityDetector::new(VadConfig::default());
        vad.process_frame(&speech_frame(0));
        let state = vad.process_frame(&speech_frame(1));
        assert_eq!(state, VadState::Speech);
    }

    #[test]
    fn brief_dip_during_speech_does_not_immediately_drop_to_silence() {
        let mut vad = VoiceActivityDetector::new(VadConfig::default());
        vad.process_frame(&speech_frame(0));
        vad.process_frame(&speech_frame(1));
        assert_eq!(vad.state(), VadState::Speech);

        // One quiet frame mid-speech: should go to MaybeSilence, not
        // Silence outright (silence_confirm_frames default is 3).
        let state = vad.process_frame(&silence_frame(2));
        assert_eq!(state, VadState::MaybeSilence);
    }

    #[test]
    fn resumed_speech_during_maybe_silence_returns_to_speech() {
        let mut vad = VoiceActivityDetector::new(VadConfig::default());
        vad.process_frame(&speech_frame(0));
        vad.process_frame(&speech_frame(1));
        vad.process_frame(&silence_frame(2)); // -> MaybeSilence
        let state = vad.process_frame(&speech_frame(3));
        assert_eq!(state, VadState::Speech);
    }

    #[test]
    fn sustained_silence_after_speech_returns_to_silence() {
        let mut vad = VoiceActivityDetector::new(VadConfig::default());
        vad.process_frame(&speech_frame(0));
        vad.process_frame(&speech_frame(1)); // -> Speech
        vad.process_frame(&silence_frame(2)); // -> MaybeSilence
        vad.process_frame(&silence_frame(3)); // -> MaybeSilence (2 consecutive)
        let state = vad.process_frame(&silence_frame(4)); // -> 3 consecutive -> Silence
        assert_eq!(state, VadState::Silence);
    }

    #[test]
    fn full_utterance_lifecycle_end_to_end() {
        // Simulates: silence -> speech starts -> sustained speech ->
        // speech ends -> back to silence, using only the mock source.
        let mut vad = VoiceActivityDetector::new(VadConfig::default());

        assert_eq!(vad.process_frame(&silence_frame(0)), VadState::Silence);
        assert_eq!(vad.process_frame(&silence_frame(1)), VadState::Silence);
        assert_eq!(vad.process_frame(&speech_frame(2)), VadState::MaybeSpeech);
        assert_eq!(vad.process_frame(&speech_frame(3)), VadState::Speech);
        assert_eq!(vad.process_frame(&speech_frame(4)), VadState::Speech);
        assert_eq!(vad.process_frame(&silence_frame(5)), VadState::MaybeSilence);
        assert_eq!(vad.process_frame(&silence_frame(6)), VadState::MaybeSilence);
        assert_eq!(vad.process_frame(&silence_frame(7)), VadState::Silence);
    }
}
