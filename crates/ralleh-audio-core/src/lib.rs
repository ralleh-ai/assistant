//! ralleh-audio-core
//!
//! Audio pipeline core for Ralleh's edge agent: capture, VAD (voice activity
//! detection), wake/push-to-talk triggering, and STT/TTS adapters.
//!
//! ## Status
//!
//! - VAD / wake-word: fully tested against `MockAudioSource`.
//! - Live capture: `CpalMicSource` (`cpal`); `try_open_default()` → `None`
//!   on headless hosts.
//! - STT: `SpeechToText` + `MockStt`; `WhisperCliStt` (ggml via whisper.cpp
//!   CLI); optional in-process `WhisperStt` behind `--features whisper`.
//! - TTS: `TextToSpeech` + `MockTts`; `PiperCliTts` (ONNX voice via Piper CLI).

mod cpal_source;
mod source;
mod stt;
mod tts;
mod vad;
mod wakeword;
mod wav;

pub use cpal_source::{CpalMicError, CpalMicSource, FrameAssembler};
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
