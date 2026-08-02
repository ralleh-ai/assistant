use serde::{Deserialize, Serialize};

/// A single frame of audio, decoupled from any specific hardware backend.
/// Real backends (e.g. a future `cpal`-based one) and the mock backend both
/// produce this same shape, so pipeline code never needs to know which one
/// is in use.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioFrame {
    /// Mono PCM samples, normalized to [-1.0, 1.0].
    pub samples: Vec<f32>,
    /// Sample rate in Hz for this frame's samples.
    pub sample_rate_hz: u32,
    /// Monotonically increasing sequence number, useful for ordering and
    /// detecting dropped frames in tests/telemetry.
    pub sequence: u64,
}

impl AudioFrame {
    /// Root-mean-square energy of the frame — the simplest useful signal
    /// for a first-pass voice-activity heuristic.
    pub fn rms_energy(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = self.samples.iter().map(|s| s * s).sum();
        (sum_sq / self.samples.len() as f32).sqrt()
    }
}

/// Abstraction over "something that produces audio frames." Implemented by
/// `MockAudioSource` (always) and `CpalMicSource` (behind `--features mic`).
pub trait AudioSource {
    /// Pull the next available frame, or `None` if the source is exhausted
    /// (mock) / no more frames are currently available (live, non-blocking
    /// backends may return `None` transiently).
    fn next_frame(&mut self) -> Option<AudioFrame>;
}

/// A simulated audio source for headless development and testing. Frames
/// are supplied up front (e.g. representing silence, then speech, then
/// silence again) and returned one at a time via `next_frame`.
///
/// This lets us validate VAD/state-machine logic deterministically without
/// any real microphone — every test using this is a real, meaningful test
/// of the pipeline logic, not a placeholder.
#[derive(Debug, Clone, Default)]
pub struct MockAudioSource {
    frames: std::collections::VecDeque<AudioFrame>,
}

impl MockAudioSource {
    pub fn new() -> Self {
        Self {
            frames: std::collections::VecDeque::new(),
        }
    }

    /// Queue a frame to be returned by a future call to `next_frame`.
    pub fn push_frame(&mut self, frame: AudioFrame) {
        self.frames.push_back(frame);
    }

    /// Convenience: queue a frame of constant-amplitude "silence" (near-zero
    /// energy) of the given length in samples.
    pub fn push_silence(&mut self, sample_rate_hz: u32, num_samples: usize, sequence: u64) {
        self.push_frame(AudioFrame {
            samples: vec![0.0_f32; num_samples],
            sample_rate_hz,
            sequence,
        });
    }

    /// Convenience: queue a frame simulating "speech" as a simple sine-like
    /// alternating pattern at the given amplitude (loud enough to clear a
    /// reasonable VAD energy threshold).
    pub fn push_speech(
        &mut self,
        sample_rate_hz: u32,
        num_samples: usize,
        amplitude: f32,
        sequence: u64,
    ) {
        let samples: Vec<f32> = (0..num_samples)
            .map(|i| if i % 2 == 0 { amplitude } else { -amplitude })
            .collect();
        self.push_frame(AudioFrame {
            samples,
            sample_rate_hz,
            sequence,
        });
    }
}

impl AudioSource for MockAudioSource {
    fn next_frame(&mut self) -> Option<AudioFrame> {
        self.frames.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_frame_has_near_zero_energy() {
        let mut source = MockAudioSource::new();
        source.push_silence(16_000, 320, 0);
        let frame = source.next_frame().unwrap();
        assert!(frame.rms_energy() < 0.001);
    }

    #[test]
    fn speech_frame_has_significant_energy() {
        let mut source = MockAudioSource::new();
        source.push_speech(16_000, 320, 0.5, 0);
        let frame = source.next_frame().unwrap();
        assert!(frame.rms_energy() > 0.1);
    }

    #[test]
    fn mock_source_returns_frames_in_order_then_none() {
        let mut source = MockAudioSource::new();
        source.push_silence(16_000, 10, 0);
        source.push_speech(16_000, 10, 0.5, 1);

        let f0 = source.next_frame().unwrap();
        assert_eq!(f0.sequence, 0);
        let f1 = source.next_frame().unwrap();
        assert_eq!(f1.sequence, 1);
        assert!(source.next_frame().is_none());
    }

    #[test]
    fn empty_frame_has_zero_energy_without_panicking() {
        let frame = AudioFrame {
            samples: vec![],
            sample_rate_hz: 16_000,
            sequence: 0,
        };
        assert_eq!(frame.rms_energy(), 0.0);
    }
}
