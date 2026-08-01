//! Text-to-speech adapter surface (symmetric to `stt`).
//!
//! Trait + mock always available; native Piper/Kokoro bindings remain a
//! follow-up per ADR-003 once a mature Rust crate is wired the same way
//! `WhisperStt` is behind the `whisper` feature.

use serde::{Deserialize, Serialize};

/// PCM mono speech produced by a TTS engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeechAudio {
    pub samples: Vec<f32>,
    pub sample_rate_hz: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("TTS engine error: {0}")]
    Engine(String),
    #[error("empty text")]
    EmptyText,
}

/// Something that turns text into PCM samples.
pub trait TextToSpeech: Send + Sync {
    fn synthesize(&self, text: &str) -> Result<SpeechAudio, TtsError>;
}

/// Deterministic TTS double: emits a short tone burst whose length scales
/// with text length (enough for pipeline tests without a real voice model).
#[derive(Debug, Clone)]
pub struct MockTts {
    sample_rate_hz: u32,
    samples_per_char: usize,
}

impl MockTts {
    pub fn new() -> Self {
        Self {
            sample_rate_hz: 16_000,
            samples_per_char: 40,
        }
    }
}

impl Default for MockTts {
    fn default() -> Self {
        Self::new()
    }
}

impl TextToSpeech for MockTts {
    fn synthesize(&self, text: &str) -> Result<SpeechAudio, TtsError> {
        if text.trim().is_empty() {
            return Err(TtsError::EmptyText);
        }
        let n = (text.chars().count() * self.samples_per_char).max(self.samples_per_char);
        // Simple audible-ish square-ish tone for non-zero energy.
        let samples: Vec<f32> = (0..n)
            .map(|i| if i % 2 == 0 { 0.2 } else { -0.2 })
            .collect();
        Ok(SpeechAudio {
            samples,
            sample_rate_hz: self.sample_rate_hz,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_rejects_empty_text() {
        let tts = MockTts::new();
        assert!(matches!(tts.synthesize("   "), Err(TtsError::EmptyText)));
    }

    #[test]
    fn mock_scales_output_with_text_length() {
        let tts = MockTts::new();
        let short = tts.synthesize("hi").unwrap();
        let long = tts.synthesize("hello world").unwrap();
        assert!(long.samples.len() > short.samples.len());
        assert_eq!(short.sample_rate_hz, 16_000);
        assert!(short.samples.iter().any(|s| s.abs() > 0.0));
    }
}
