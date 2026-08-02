//! `cpal`-backed live microphone `AudioSource` (feature `mic`).
//!
//! Off by default so headless CI/dev hosts never need ALSA/WASAPI link
//! deps. Enable with `--features mic` on machines with a real input device.
//! Device open is best-effort via `try_open_default` — broken Pulse/ALSA
//! setups return `Ok(None)` instead of failing the unit suite.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, Stream, StreamConfig};

use crate::frame::FrameAssembler;
use crate::source::{AudioFrame, AudioSource};

const DEFAULT_FRAME_MS: u32 = 20;

#[derive(Debug, thiserror::Error)]
pub enum CpalMicError {
    #[error("no input device available on this host")]
    NoInputDevice,
    #[error("live audio skipped (RALLEH_SKIP_LIVE_AUDIO or CI without RALLEH_LIVE_MIC)")]
    SkippedByEnv,
    #[error("failed to query default input config: {0}")]
    DefaultConfig(String),
    #[error("unsupported sample format: {0:?}")]
    UnsupportedFormat(SampleFormat),
    #[error("failed to build input stream: {0}")]
    BuildStream(String),
    #[error("failed to start input stream: {0}")]
    PlayStream(String),
}

/// True when the operator explicitly wants live mic smoke (`RALLEH_LIVE_MIC=1`).
pub fn live_mic_requested() -> bool {
    matches!(
        std::env::var("RALLEH_LIVE_MIC").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
}

/// Soft-skip live open: explicit skip, or CI hosts unless live mic is requested.
pub fn should_skip_live_audio() -> bool {
    if std::env::var_os("RALLEH_SKIP_LIVE_AUDIO").is_some() {
        return true;
    }
    std::env::var_os("CI").is_some() && !live_mic_requested()
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
    /// Hard errors for desktop apps that need a real mic.
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

    /// Best-effort open for tests and optional capture: CI / skip-env and
    /// any hardware failure become `None`. Use `open_default` in apps
    /// (e.g. `mic-capture`) when a mic is required.
    pub fn try_open_default() -> Option<Self> {
        if should_skip_live_audio() {
            return None;
        }
        Self::open_default().ok()
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.assembler.sample_rate_hz()
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
    fn try_open_default_never_hard_fails() {
        // With or without a mic / under CI skip: always None or Some.
        if let Some(mut src) = CpalMicSource::try_open_default() {
            let _ = src.next_frame();
            assert!(src.sample_rate_hz() > 0);
        }
    }

    /// Opt-in live mic proof on a desktop with hardware:
    ///   RALLEH_LIVE_MIC=1 cargo test -p ralleh-audio-core --features mic -- --ignored live_mic
    #[test]
    #[ignore = "requires RALLEH_LIVE_MIC=1, --features mic, and a working input device"]
    fn live_mic_smoke_when_explicitly_enabled() {
        assert!(
            live_mic_requested(),
            "set RALLEH_LIVE_MIC=1 to run this smoke"
        );
        let src = CpalMicSource::open_default().expect("mic should open when live requested");
        assert!(src.sample_rate_hz() > 0);
    }
}
