//! ralleh-audio-core
//!
//! Audio pipeline core for Ralleh's edge agent: capture, VAD (voice activity
//! detection), and wake/push-to-talk triggering.
//!
//! ## Status
//!
//! Pipeline logic (VAD, wake-word) is fully tested against `MockAudioSource`.
//! Live capture is available via `CpalMicSource` (`cpal` default-input
//! device). On hosts with no microphone, `CpalMicSource::try_open_default`
//! returns `Ok(None)` so headless CI stays green. Real STT/TTS bindings
//! (whisper-rs, Piper/Kokoro) remain follow-ups per DEVELOPMENT.md Phase 1.

mod cpal_source;
mod source;
mod vad;
mod wakeword;

pub use cpal_source::{CpalMicError, CpalMicSource, FrameAssembler};
pub use source::{AudioFrame, AudioSource, MockAudioSource};
pub use vad::{VadConfig, VadState, VoiceActivityDetector};
pub use wakeword::{
    MockWakeWordMatcher, WakeWordConfig, WakeWordDetector, WakeWordMatcher, WakeWordTrigger,
};
