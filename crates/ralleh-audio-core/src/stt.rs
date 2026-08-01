//! Speech-to-text adapter surface for the voice pipeline.
//!
//! Mirrors the `AudioSource` / `CompletionBackend` pattern: a small trait,
//! a deterministic mock for tests, and (behind the `whisper` feature) a
//! native `whisper-rs` binding per ADR-003. Pipeline code depends only on
//! `SpeechToText` so STT engines can be swapped without touching VAD or
//! wake-word logic.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of one transcription attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transcript {
    pub text: String,
    /// Engine-reported confidence in \[0, 1\] when available.
    pub confidence: Option<f32>,
    /// True when the engine judged the audio as non-speech (hallucination
    /// guard — DEVELOPMENT.md calls out no-speech thresholds explicitly).
    pub no_speech: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("STT engine error: {0}")]
    Engine(String),
    #[error("unsupported sample rate {0} Hz (expected {1} Hz)")]
    UnsupportedSampleRate(u32, u32),
    #[error("empty audio buffer")]
    EmptyAudio,
}

/// Something that turns PCM mono samples into text.
pub trait SpeechToText: Send + Sync {
    fn transcribe(&self, samples: &[f32], sample_rate_hz: u32) -> Result<Transcript, SttError>;
}

/// Deterministic STT double for headless tests: returns a configured phrase
/// when RMS energy clears a threshold, otherwise `no_speech`.
#[derive(Debug, Clone)]
pub struct MockStt {
    phrase: String,
    energy_threshold: f32,
    confidence: f32,
}

impl MockStt {
    pub fn new(phrase: impl Into<String>) -> Self {
        Self {
            phrase: phrase.into(),
            energy_threshold: 0.05,
            confidence: 0.95,
        }
    }

    pub fn with_energy_threshold(mut self, threshold: f32) -> Self {
        self.energy_threshold = threshold;
        self
    }
}

impl SpeechToText for MockStt {
    fn transcribe(&self, samples: &[f32], _sample_rate_hz: u32) -> Result<Transcript, SttError> {
        if samples.is_empty() {
            return Err(SttError::EmptyAudio);
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        let rms = (sum_sq / samples.len() as f32).sqrt();
        if rms < self.energy_threshold {
            return Ok(Transcript {
                text: String::new(),
                confidence: Some(1.0),
                no_speech: true,
            });
        }
        Ok(Transcript {
            text: self.phrase.clone(),
            confidence: Some(self.confidence),
            no_speech: false,
        })
    }
}

#[cfg(feature = "whisper")]
mod whisper_engine {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Native whisper.cpp binding via `whisper-rs` (ADR-003).
    ///
    /// Expects a ggml model file on disk (e.g. `ggml-base.en.bin`). Sample
    /// rate must be 16 kHz mono PCM — resample upstream if needed.
    pub struct WhisperStt {
        ctx: whisper_rs::WhisperContext,
        model_path: PathBuf,
    }

    impl WhisperStt {
        pub fn open(model_path: impl AsRef<Path>) -> Result<Self, SttError> {
            let model_path = model_path.as_ref().to_path_buf();
            let ctx = whisper_rs::WhisperContext::new_with_params(
                &model_path,
                whisper_rs::WhisperContextParameters::default(),
            )
            .map_err(|e| SttError::Engine(e.to_string()))?;
            Ok(Self { ctx, model_path })
        }

        pub fn model_path(&self) -> &Path {
            &self.model_path
        }
    }

    impl SpeechToText for WhisperStt {
        fn transcribe(
            &self,
            samples: &[f32],
            sample_rate_hz: u32,
        ) -> Result<Transcript, SttError> {
            if samples.is_empty() {
                return Err(SttError::EmptyAudio);
            }
            if sample_rate_hz != 16_000 {
                return Err(SttError::UnsupportedSampleRate(sample_rate_hz, 16_000));
            }

            let mut state = self
                .ctx
                .create_state()
                .map_err(|e| SttError::Engine(e.to_string()))?;
            let params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy {
                best_of: 1,
            });
            state
                .full(params, samples)
                .map_err(|e| SttError::Engine(e.to_string()))?;

            let n = state
                .full_n_segments()
                .map_err(|e| SttError::Engine(e.to_string()))?;
            let mut text = String::new();
            for i in 0..n {
                let seg = state
                    .full_get_segment_text(i)
                    .map_err(|e| SttError::Engine(e.to_string()))?;
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(seg.trim());
            }
            let text = text.trim().to_string();
            let no_speech = text.is_empty();
            Ok(Transcript {
                text,
                confidence: None,
                no_speech,
            })
        }
    }
}

#[cfg(feature = "whisper")]
pub use whisper_engine::WhisperStt;

/// Process-backed whisper.cpp CLI (`whisper-cli`) for hosts where in-process
/// `whisper-rs` cannot bindgen (notably Windows MSVC). Same ggml models.
///
/// Env for ignored e2e: `WHISPER_CLI_PATH`, `WHISPER_MODEL_PATH`.
#[derive(Debug, Clone)]
pub struct WhisperCliStt {
    cli_path: PathBuf,
    model_path: PathBuf,
}

impl WhisperCliStt {
    pub fn new(cli_path: impl AsRef<Path>, model_path: impl AsRef<Path>) -> Self {
        Self {
            cli_path: cli_path.as_ref().to_path_buf(),
            model_path: model_path.as_ref().to_path_buf(),
        }
    }

    /// From `WHISPER_CLI_PATH` + `WHISPER_MODEL_PATH`.
    pub fn from_env() -> Result<Self, SttError> {
        let cli = std::env::var("WHISPER_CLI_PATH")
            .map_err(|_| SttError::Engine("WHISPER_CLI_PATH not set".into()))?;
        let model = std::env::var("WHISPER_MODEL_PATH")
            .map_err(|_| SttError::Engine("WHISPER_MODEL_PATH not set".into()))?;
        Ok(Self::new(cli, model))
    }

    /// Transcribe an existing WAV file (16-bit PCM preferred).
    pub fn transcribe_file(&self, wav_path: impl AsRef<Path>) -> Result<Transcript, SttError> {
        let output = Command::new(&self.cli_path)
            .arg("-m")
            .arg(&self.model_path)
            .arg("-f")
            .arg(wav_path.as_ref())
            .arg("-nt") // no timestamps in stdout
            .arg("-np") // no prints to stderr progress (still some logs)
            .output()
            .map_err(|e| SttError::Engine(format!("spawn whisper-cli: {e}")))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(SttError::Engine(format!(
                "whisper-cli exited {}: {err}",
                output.status
            )));
        }
        let text = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let text = text.trim().to_string();
        let no_speech = text.is_empty();
        Ok(Transcript {
            text,
            confidence: None,
            no_speech,
        })
    }
}

impl SpeechToText for WhisperCliStt {
    fn transcribe(&self, samples: &[f32], sample_rate_hz: u32) -> Result<Transcript, SttError> {
        if samples.is_empty() {
            return Err(SttError::EmptyAudio);
        }
        let dir = std::env::temp_dir();
        let wav_path = dir.join(format!(
            "ralleh-whisper-{}.wav",
            std::process::id()
        ));
        crate::wav::write_pcm16_mono(&wav_path, samples, sample_rate_hz)
            .map_err(|e| SttError::Engine(e.to_string()))?;
        let result = self.transcribe_file(&wav_path);
        let _ = std::fs::remove_file(&wav_path);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_returns_phrase_for_speech_energy() {
        let stt = MockStt::new("hello ralleh");
        let samples: Vec<f32> = (0..320)
            .map(|i| if i % 2 == 0 { 0.4 } else { -0.4 })
            .collect();
        let t = stt.transcribe(&samples, 16_000).unwrap();
        assert!(!t.no_speech);
        assert_eq!(t.text, "hello ralleh");
    }

    #[test]
    fn mock_marks_silence_as_no_speech() {
        let stt = MockStt::new("hello");
        let samples = vec![0.0_f32; 320];
        let t = stt.transcribe(&samples, 16_000).unwrap();
        assert!(t.no_speech);
        assert!(t.text.is_empty());
    }

    #[test]
    fn mock_rejects_empty_buffer() {
        let stt = MockStt::new("x");
        assert!(matches!(
            stt.transcribe(&[], 16_000),
            Err(SttError::EmptyAudio)
        ));
    }

    /// Opt-in e2e against a real ggml model via in-process whisper-rs:
    ///   set WHISPER_MODEL_PATH=/path/to/ggml-tiny.en.bin
    ///   cargo test -p ralleh-audio-core --features whisper -- --ignored whisper_rs_e2e
    #[cfg(feature = "whisper")]
    #[test]
    #[ignore = "requires WHISPER_MODEL_PATH + working whisper-rs bindgen"]
    fn whisper_rs_e2e_transcribes_or_marks_no_speech() {
        let path = std::env::var("WHISPER_MODEL_PATH").expect("WHISPER_MODEL_PATH");
        let stt = WhisperStt::open(path).expect("open whisper model");
        let samples = vec![0.0_f32; 16_000];
        let t = stt.transcribe(&samples, 16_000).expect("transcribe");
        assert!(t.no_speech || t.text.len() < 64);
    }

    /// Opt-in e2e via whisper.cpp CLI + real ggml model (works on Windows):
    ///   ./scripts/download-whisper-cli.ps1
    ///   ./scripts/download-whisper-model.ps1
    ///   $env:WHISPER_CLI_PATH = ...\whisper-cli.exe
    ///   $env:WHISPER_MODEL_PATH = ...\ggml-tiny.en.bin
    ///   cargo test -p ralleh-audio-core -- --ignored whisper_cli_e2e
    #[test]
    #[ignore = "requires WHISPER_CLI_PATH + WHISPER_MODEL_PATH"]
    fn whisper_cli_e2e_silence_and_jfk() {
        let stt = WhisperCliStt::from_env().expect("from_env");
        let silence = vec![0.0_f32; 16_000];
        let quiet = stt.transcribe(&silence, 16_000).expect("silence");
        assert!(quiet.no_speech || quiet.text.len() < 80);

        // Prefer repo sample if present (scripts download it next to the model).
        let jfk = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../models/jfk.wav");
        if jfk.is_file() {
            let uttered = stt.transcribe_file(&jfk).expect("jfk");
            assert!(!uttered.no_speech, "expected speech for jfk.wav");
            let lower = uttered.text.to_lowercase();
            assert!(
                lower.contains("ask") || lower.contains("americans") || lower.contains("country"),
                "unexpected transcript: {}",
                uttered.text
            );
        }
    }
}
