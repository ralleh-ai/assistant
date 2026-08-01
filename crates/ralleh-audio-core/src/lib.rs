//! ralleh-audio-core
//!
//! Audio pipeline core for Ralleh's edge agent: capture, VAD (voice activity
//! detection), wake/push-to-talk triggering, and STT adapters.
//!
//! ## Status
//!
//! - VAD / wake-word: fully tested against `MockAudioSource`.
//! - Live capture: `CpalMicSource` (`cpal`); `try_open_default()` → `None`
//!   on headless hosts.
//! - STT: `SpeechToText` trait + `MockStt` always available; native
//!   `WhisperStt` behind the `whisper` feature (ADR-003). TTS still TBD.

mod cpal_source;
mod source;
mod stt;
mod tts;
mod vad;
mod wakeword;

pub use cpal_source::{CpalMicError, CpalMicSource, FrameAssembler};
pub use source::{AudioFrame, AudioSource, MockAudioSource};
pub use stt::{MockStt, SpeechToText, SttError, Transcript};
#[cfg(feature = "whisper")]
pub use stt::WhisperStt;
pub use tts::{MockTts, SpeechAudio, TextToSpeech, TtsError};
pub use vad::{VadConfig, VadState, VoiceActivityDetector};
pub use wakeword::{
    MockWakeWordMatcher, WakeWordConfig, WakeWordDetector, WakeWordMatcher, WakeWordTrigger,
};
