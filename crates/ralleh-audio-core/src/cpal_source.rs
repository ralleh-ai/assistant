//! `cpal`-backed live microphone `AudioSource`.
//!
//! Complements `MockAudioSource`: VAD/wake-word consumers keep talking to
//! the `AudioSource` trait; swapping mock → mic is a construction-site
//! change only. Device open is best-effort — headless CI hosts without an
//! input device get a clean `NoInputDevice` error rather than a panic.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, Stream, StreamConfig};

use crate::source::{AudioFrame, AudioSource};

const DEFAULT_FRAME_MS: u32 = 20;

#[derive(Debug, thiserror::Error)]
pub enum CpalMicError {
    #[error("no input device available on this host")]
    NoInputDevice,
    #[error("failed to query default input config: {0}")]
    DefaultConfig(String),
    #[error("unsupported sample format: {0:?}")]
    UnsupportedFormat(SampleFormat),
    #[error("failed to build input stream: {0}")]
    BuildStream(String),
    #[error("failed to start input stream: {0}")]
    PlayStream(String),
}

/// Assembles a stream of PCM samples into fixed-size `AudioFrame`s.
/// Pure logic — unit-tested without a microphone.
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
}

/// Live microphone source. Holds the `cpal` stream so capture keeps running
/// for the lifetime of this value; `next_frame` drains assembled frames.
pub struct CpalMicSource {
    _stream: Stream,
    rx: Receiver<Vec<f32>>,
    assembler: FrameAssembler,
    ready: std::collections::VecDeque<AudioFrame>,
}

impl CpalMicSource {
    /// Open the host default input device, delivering ~`frame_ms` frames.
    pub fn open_default() -> Result<Self, CpalMicError> {
        Self::open_default_with_frame_ms(DEFAULT_FRAME_MS)
    }

    pub fn open_default_with_frame_ms(frame_ms: u32) -> Result<Self, CpalMicError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(CpalMicError::NoInputDevice)?;
        let supported = device
            .default_input_config()
            .map_err(|e| CpalMicError::DefaultConfig(e.to_string()))?;

        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.clone().into();
        let sample_rate_hz = config.sample_rate.0;
        let channels = config.channels as usize;
        let frame_len = ((sample_rate_hz as u64 * frame_ms as u64) / 1000) as usize;
        let frame_len = frame_len.max(1);

        let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(64);
        let err_flag = Arc::new(Mutex::new(None::<String>));
        let err_flag_cb = err_flag.clone();

        let stream = match sample_format {
            SampleFormat::F32 => build_input_stream::<f32, _>(
                &device,
                &config,
                channels,
                tx,
                err_flag_cb,
                |s| *s,
            )?,
            SampleFormat::I16 => build_input_stream::<i16, _>(
                &device,
                &config,
                channels,
                tx,
                err_flag_cb,
                |s| (*s).to_sample::<f32>(),
            )?,
            SampleFormat::U16 => build_input_stream::<u16, _>(
                &device,
                &config,
                channels,
                tx,
                err_flag_cb,
                |s| (*s).to_sample::<f32>(),
            )?,
            other => return Err(CpalMicError::UnsupportedFormat(other)),
        };

        stream
            .play()
            .map_err(|e| CpalMicError::PlayStream(e.to_string()))?;

        Ok(Self {
            _stream: stream,
            rx,
            assembler: FrameAssembler::new(sample_rate_hz, frame_len),
            ready: std::collections::VecDeque::new(),
        })
    }

    /// Like `open_default`, but returns `Ok(None)` when no input device
    /// exists — convenient for tests on headless hosts.
    pub fn try_open_default() -> Result<Option<Self>, CpalMicError> {
        match Self::open_default() {
            Ok(src) => Ok(Some(src)),
            Err(CpalMicError::NoInputDevice) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.assembler.sample_rate_hz
    }
}

fn build_input_stream<T, F>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    tx: mpsc::SyncSender<Vec<f32>>,
    err_flag: Arc<Mutex<Option<String>>>,
    convert: F,
) -> Result<Stream, CpalMicError>
where
    T: Sample + cpal::SizedSample + Send + 'static,
    F: Fn(&T) -> f32 + Send + 'static,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                // Downmix to mono by averaging channels when needed.
                let mono: Vec<f32> = if channels <= 1 {
                    data.iter().map(|s| convert(s)).collect()
                } else {
                    data.chunks(channels)
                        .map(|frame| {
                            let sum: f32 = frame.iter().map(|s| convert(s)).sum();
                            sum / channels as f32
                        })
                        .collect()
                };
                // Drop on full channel rather than block the audio callback.
                let _ = tx.try_send(mono);
            },
            move |err| {
                if let Ok(mut slot) = err_flag.lock() {
                    *slot = Some(err.to_string());
                }
            },
            None,
        )
        .map_err(|e| CpalMicError::BuildStream(e.to_string()))
}

impl AudioSource for CpalMicSource {
    fn next_frame(&mut self) -> Option<AudioFrame> {
        // Drain whatever the callback has queued, then return one ready frame.
        loop {
            match self.rx.try_recv() {
                Ok(chunk) => {
                    for frame in self.assembler.push(&chunk) {
                        self.ready.push_back(frame);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        self.ready.pop_front()
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

    #[test]
    fn try_open_default_is_none_or_usable_without_panic() {
        // Headless CI: Ok(None). Desktop with a mic: Ok(Some(...)) and we
        // can pull zero-or-more frames without panicking.
        let opened = CpalMicSource::try_open_default().expect("open should not hard-error");
        if let Some(mut src) = opened {
            let _ = src.next_frame();
            assert!(src.sample_rate_hz() > 0);
        }
    }
}
