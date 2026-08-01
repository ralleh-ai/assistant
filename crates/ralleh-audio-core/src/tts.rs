//! Text-to-speech adapter surface (symmetric to `stt`).
//!
//! Trait + mock always available. Native Piper/Kokoro Rust crates remain a
//! follow-up (ADR-003); `PiperCliTts` covers real-model e2e via the official
//! Piper CLI the same way `WhisperCliStt` covers ggml on Windows.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::io::Write;

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

/// Process-backed [Piper](https://github.com/rhasspy/piper) CLI.
///
/// Env for ignored e2e: `PIPER_CLI_PATH`, `PIPER_MODEL_PATH` (`.onnx`; config
/// defaults to `model.onnx.json` beside it).
#[derive(Debug, Clone)]
pub struct PiperCliTts {
    cli_path: PathBuf,
    model_path: PathBuf,
}

impl PiperCliTts {
    pub fn new(cli_path: impl AsRef<Path>, model_path: impl AsRef<Path>) -> Self {
        Self {
            cli_path: cli_path.as_ref().to_path_buf(),
            model_path: model_path.as_ref().to_path_buf(),
        }
    }

    pub fn from_env() -> Result<Self, TtsError> {
        let cli = std::env::var("PIPER_CLI_PATH")
            .map_err(|_| TtsError::Engine("PIPER_CLI_PATH not set".into()))?;
        let model = std::env::var("PIPER_MODEL_PATH")
            .map_err(|_| TtsError::Engine("PIPER_MODEL_PATH not set".into()))?;
        Ok(Self::new(cli, model))
    }
}

impl TextToSpeech for PiperCliTts {
    fn synthesize(&self, text: &str) -> Result<SpeechAudio, TtsError> {
        if text.trim().is_empty() {
            return Err(TtsError::EmptyText);
        }
        let out_wav = std::env::temp_dir().join(format!(
            "ralleh-piper-{}.wav",
            std::process::id()
        ));
        let work_dir = self
            .cli_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let mut child = Command::new(&self.cli_path)
            .current_dir(work_dir)
            .arg("--model")
            .arg(&self.model_path)
            .arg("--output_file")
            .arg(&out_wav)
            .arg("--quiet")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| TtsError::Engine(format!("spawn piper: {e}")))?;
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| TtsError::Engine("piper stdin missing".into()))?;
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| TtsError::Engine(format!("piper stdin: {e}")))?;
            // Piper reads until EOF.
        }
        let output = child
            .wait_with_output()
            .map_err(|e| TtsError::Engine(format!("wait piper: {e}")))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            let _ = std::fs::remove_file(&out_wav);
            return Err(TtsError::Engine(format!(
                "piper exited {}: {err}",
                output.status
            )));
        }
        let pcm = crate::wav::read_pcm16(&out_wav).map_err(|e| TtsError::Engine(e.to_string()))?;
        let _ = std::fs::remove_file(&out_wav);
        if pcm.samples.is_empty() {
            return Err(TtsError::Engine("piper produced empty audio".into()));
        }
        Ok(SpeechAudio {
            samples: pcm.samples,
            sample_rate_hz: pcm.sample_rate_hz,
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

    /// Opt-in e2e via Piper CLI + ONNX voice:
    ///   ./scripts/download-piper.ps1
    ///   $env:PIPER_CLI_PATH / $env:PIPER_MODEL_PATH
    ///   cargo test -p ralleh-audio-core -- --ignored piper_cli_e2e
    #[test]
    #[ignore = "requires PIPER_CLI_PATH + PIPER_MODEL_PATH"]
    fn piper_cli_e2e_synthesizes_non_empty_pcm() {
        let tts = PiperCliTts::from_env().expect("from_env");
        let audio = tts
            .synthesize("Hello from Ralleh.")
            .expect("synthesize");
        assert!(!audio.samples.is_empty());
        assert!(audio.sample_rate_hz >= 16_000);
        let energy: f32 = audio.samples.iter().map(|s| s * s).sum::<f32>() / audio.samples.len() as f32;
        assert!(energy.sqrt() > 1e-4, "expected non-silent speech");
    }
}
