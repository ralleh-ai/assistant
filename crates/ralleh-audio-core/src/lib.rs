//! ralleh-audio-core
//!
//! Audio pipeline core for Ralleh's edge agent: capture, VAD (voice activity
//! detection), wake/push-to-talk triggering, and STT/TTS adapters.
//!
//! ## Headless vs desktop
//!
//! Default builds are **headless-safe**: mocks + `FrameAssembler`, no `cpal`.
//! Live mic is `--features mic`. Real Whisper/Piper engines are ignored e2e
//! or feature-gated. See `docs/HEADLESS.md`.

mod frame;
mod pipeline;
mod source;
mod stt;
mod tts;
mod vad;
mod wakeword;
mod wav;

#[cfg(feature = "mic")]
mod cpal_source;

pub use frame::FrameAssembler;
pub use source::{AudioFrame, AudioSource, MockAudioSource};
pub use stt::{MockStt, SpeechToText, SttError, Transcript, WhisperCliStt};
#[cfg(feature = "whisper")]
pub use stt::WhisperStt;
pub use tts::{MockTts, PiperCliTts, SpeechAudio, TextToSpeech, TtsError};
pub use vad::{VadConfig, VadState, VoiceActivityDetector};
pub use wakeword::{
    MockWakeWordMatcher, WakeWordConfig, WakeWordDetector, WakeWordMatcher, WakeWordTrigger,
};
pub use wav::{read_pcm16, write_pcm16_mono, PcmMono, WavError};

#[cfg(feature = "mic")]
pub use cpal_source::{
    live_mic_requested, should_skip_live_audio, CpalMicError, CpalMicSource,
};
