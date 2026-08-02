//! Headless voice-pipeline smoke: mock mic → VAD → STT → TTS.
//!
//! Shared by unit tests and the Tauri edge (`voice_smoke` IPC) so desktop
//! Phase 1 can exercise the audio path without hardware.

use crate::source::{AudioSource, MockAudioSource};
use crate::stt::{MockStt, SpeechToText};
use crate::tts::{MockTts, TextToSpeech};
use crate::vad::{VadConfig, VadState, VoiceActivityDetector};
use serde::{Deserialize, Serialize};

/// Result of one mock mic → VAD → STT → TTS pass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MockVoicePipelineResult {
    pub transcript: String,
    pub tts_samples: usize,
    pub sample_rate_hz: u32,
}

/// Collect PCM from frames while VAD reports speech (after confirm).
fn collect_utterance(source: &mut dyn AudioSource, vad: &mut VoiceActivityDetector) -> Vec<f32> {
    let mut pcm = Vec::new();
    let mut in_speech = false;
    while let Some(frame) = source.next_frame() {
        let state = vad.process_frame(&frame);
        match state {
            VadState::Speech | VadState::MaybeSilence => {
                in_speech = true;
                pcm.extend_from_slice(&frame.samples);
            }
            VadState::Silence if in_speech => break,
            VadState::MaybeSpeech => {}
            VadState::Silence => {}
        }
    }
    pcm
}

/// Deterministic smoke used by tests and the desktop edge (no mic / models).
pub fn run_mock_voice_pipeline() -> Result<MockVoicePipelineResult, String> {
    let mut source = MockAudioSource::new();
    source.push_silence(16_000, 320, 0);
    source.push_speech(16_000, 320, 0.5, 1);
    source.push_speech(16_000, 320, 0.5, 2);
    source.push_speech(16_000, 320, 0.5, 3);
    source.push_speech(16_000, 320, 0.5, 4);
    source.push_silence(16_000, 320, 5);
    source.push_silence(16_000, 320, 6);
    source.push_silence(16_000, 320, 7);
    source.push_silence(16_000, 320, 8);

    let mut vad = VoiceActivityDetector::new(VadConfig::default());
    let pcm = collect_utterance(&mut source, &mut vad);
    if pcm.is_empty() {
        return Err("VAD captured no speech frames from mock mic".into());
    }

    let stt = MockStt::new("hello from headless pipeline");
    let transcript = stt
        .transcribe(&pcm, 16_000)
        .map_err(|e| e.to_string())?;
    if transcript.no_speech {
        return Err("MockStt marked utterance as no_speech".into());
    }

    let tts = MockTts::new();
    let audio = tts
        .synthesize(&transcript.text)
        .map_err(|e| e.to_string())?;

    Ok(MockVoicePipelineResult {
        transcript: transcript.text,
        tts_samples: audio.samples.len(),
        sample_rate_hz: audio.sample_rate_hz,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_mock_pipeline_vad_stt_tts() {
        let result = run_mock_voice_pipeline().expect("pipeline");
        assert_eq!(result.transcript, "hello from headless pipeline");
        assert!(result.tts_samples > 0);
        assert_eq!(result.sample_rate_hz, 16_000);
    }
}
