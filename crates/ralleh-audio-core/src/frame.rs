//! Fixed-size PCM frame assembly — pure logic, no audio hardware.
//!
//! Shared by the live mic path (`CpalMicSource`) and headless tests so
//! chunking behavior is validated without `cpal` / ALSA.

use crate::source::AudioFrame;

/// Assembles a stream of PCM samples into fixed-size `AudioFrame`s.
#[derive(Debug)]
pub struct FrameAssembler {
    pending: Vec<f32>,
    frame_len: usize,
    sample_rate_hz: u32,
    sequence: u64,
}

impl FrameAssembler {
    pub fn new(sample_rate_hz: u32, frame_len: usize) -> Self {
        Self {
            pending: Vec::with_capacity(frame_len),
            frame_len: frame_len.max(1),
            sample_rate_hz,
            sequence: 0,
        }
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<AudioFrame> {
        self.pending.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.pending.len() >= self.frame_len {
            let frame_samples: Vec<f32> = self.pending.drain(..self.frame_len).collect();
            out.push(AudioFrame {
                samples: frame_samples,
                sample_rate_hz: self.sample_rate_hz,
                sequence: self.sequence,
            });
            self.sequence = self.sequence.wrapping_add(1);
        }
        out
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembler_emits_fixed_size_frames_in_order() {
        let mut asm = FrameAssembler::new(16_000, 4);
        let frames = asm.push(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].sequence, 0);
        assert_eq!(frames[0].samples, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(frames[1].sequence, 1);
        assert_eq!(frames[1].samples, vec![0.5, 0.6, 0.7, 0.8]);
        assert_eq!(asm.pending_len(), 1);
    }
}
