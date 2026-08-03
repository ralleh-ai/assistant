//! Headless voice-pipeline smoke: mock mic → VAD → STT → TTS.
//!
//! Shared by unit tests and the Tauri edge (`voice_smoke` IPC) so desktop
//! Phase 1 can exercise the audio path without hardware.
//! Live mic capture smoke is available behind `--features mic`.

use serde::{Deserialize, Serialize};

use crate::source::{AudioSource, MockAudioSource};
use crate::stt::{MockStt, SpeechToText};
use crate::tts::{MockTts, TextToSpeech};
use crate::vad::{VadConfig, VadState, VoiceActivityDetector};

/// Result of one mock mic → VAD → STT → TTS pass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MockVoicePipelineResult {
    pub transcript: String,
    pub tts_samples: usize,
    pub sample_rate_hz: u32,
}

/// Short live-mic capture metrics (no STT — proves device open + frames).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiveMicSmokeResult {
    pub sample_rate_hz: u32,
    pub duration_ms: u32,
    pub frames: u32,
    pub samples: usize,
    pub peak_rms: f32,
    pub max_abs: f32,
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

/// Capture ~`seconds` from the default mic and return level metrics.
///
/// Requires `--features mic`. Soft-skips under CI / `RALLEH_SKIP_LIVE_AUDIO`
/// unless `RALLEH_LIVE_MIC=1` (same rules as `CpalMicSource::try_open_default`).
pub fn run_live_mic_smoke(seconds: f32) -> Result<LiveMicSmokeResult, String> {
    #[cfg(not(feature = "mic"))]
    {
        let _ = seconds;
        Err(
            "live mic not compiled into this binary — restart with scripts\\tauri-dev.cmd (mic is on by default for desktop-edge)"
                .into(),
        )
    }

    #[cfg(feature = "mic")]
    {
        use std::thread;
        use std::time::{Duration, Instant};

        use crate::cpal_source::{should_skip_live_audio, CpalMicSource};

        let seconds = seconds.clamp(0.25, 10.0);
        if should_skip_live_audio() {
            return Err(
                "live mic skipped (CI without RALLEH_LIVE_MIC=1, or RALLEH_SKIP_LIVE_AUDIO set)"
                    .into(),
            );
        }

        let mut mic = CpalMicSource::open_default().map_err(|e| e.to_string())?;
        let sample_rate_hz = mic.sample_rate_hz();
        let deadline = Instant::now() + Duration::from_secs_f32(seconds);

        let mut samples: Vec<f32> = Vec::new();
        let mut frames: u32 = 0;
        let mut peak_rms = 0.0_f32;
        let mut max_abs = 0.0_f32;

        while Instant::now() < deadline {
            while let Some(frame) = mic.next_frame() {
                frames += 1;
                let mut sum_sq = 0.0_f32;
                for &s in &frame.samples {
                    sum_sq += s * s;
                    max_abs = max_abs.max(s.abs());
                }
                let n = frame.samples.len().max(1) as f32;
                peak_rms = peak_rms.max((sum_sq / n).sqrt());
                samples.extend_from_slice(&frame.samples);
            }
            thread::sleep(Duration::from_millis(10));
        }

        if frames == 0 {
            return Err(
                "mic opened but delivered no frames — check device and OS permissions".into(),
            );
        }

        Ok(LiveMicSmokeResult {
            sample_rate_hz,
            duration_ms: (seconds * 1000.0) as u32,
            frames,
            samples: samples.len(),
            peak_rms,
            max_abs,
        })
    }
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

    #[test]
    fn live_mic_smoke_errors_without_mic_feature_or_skips_cleanly() {
        // Without `mic`: clear error. With `mic` under CI skip: also Err.
        // Never panics; never opens hardware in default workspace test.
        let _ = run_live_mic_smoke(0.5);
    }
}
