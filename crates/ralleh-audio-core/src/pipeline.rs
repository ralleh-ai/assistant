//! Headless voice-pipeline smoke: mock mic → VAD → STT → TTS.
//!
//! Proves the desktop audio path can be exercised without hardware so
//! development can return to a headless host without losing coverage.

use crate::source::{AudioSource, MockAudioSource};
use crate::stt::{MockStt, SpeechToText};
use crate::tts::{MockTts, TextToSpeech};
use crate::vad::{VadConfig, VadState, VoiceActivityDetector};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_mock_pipeline_vad_stt_tts() {
        let mut source = MockAudioSource::new();
        source.push_silence(16_000, 320, 0);
        // speech_confirm_frames default = 2
        source.push_speech(16_000, 320, 0.5, 1);
        source.push_speech(16_000, 320, 0.5, 2);
        source.push_speech(16_000, 320, 0.5, 3);
        source.push_speech(16_000, 320, 0.5, 4);
        // silence_confirm_frames default = 3
        source.push_silence(16_000, 320, 5);
        source.push_silence(16_000, 320, 6);
        source.push_silence(16_000, 320, 7);
        source.push_silence(16_000, 320, 8);

        let mut vad = VoiceActivityDetector::new(VadConfig::default());
        let pcm = collect_utterance(&mut source, &mut vad);
        assert!(
            !pcm.is_empty(),
            "VAD should capture speech frames from mock mic"
        );

        let stt = MockStt::new("hello from headless pipeline");
        let transcript = stt.transcribe(&pcm, 16_000).expect("stt");
        assert!(!transcript.no_speech);
        assert_eq!(transcript.text, "hello from headless pipeline");

        let tts = MockTts::new();
        let audio = tts.synthesize(&transcript.text).expect("tts");
        assert!(!audio.samples.is_empty());
        assert_eq!(audio.sample_rate_hz, 16_000);
    }
}
