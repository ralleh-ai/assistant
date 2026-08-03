//! `cpal`-backed live speaker `PlaybackSink` (feature `playback`).
//!
//! Mirror image of `cpal_source::CpalMicSource`: opens the host's
//! default output device, holds a real-time output stream, and
//! exposes a synchronous `enqueue` API that queues PCM for the
//! callback to drain. This is what turns synthesized TTS from a
//! silent visual pulse into audible speech.
//!
//! ## Why this lives here and not in `desktop-edge`
//!
//! Same reasoning as `CpalMicSource`: audio device access is a
//! `ralleh-audio-core` concern, not a Tauri concern. Keeping the sink
//! here means the CLI mock pipeline, future headless voice tests, and
//! the desktop shell all consume the same abstraction.
//!
//! ## Headless / CI behavior
//!
//! Default builds don't link `cpal` at all (this whole module is
//! `#[cfg(feature = "playback")]`). Even with the feature on,
//! `try_open_default` soft-fails to `None` under CI or when
//! `RALLEH_SKIP_LIVE_AUDIO` is set — same policy as the mic side, so
//! `voice_smoke` running in CI never has to guard around this.
//!
//! ## Ringbuffer, not "ringbuffer tap"
//!
//! The RMS pump on the presence side (`presence_speaking`) still
//! consumes the pre-computed PCM buffer, not a tap on what the
//! callback actually played. That's a deliberate scope choice for
//! this landing: the mock TTS produces its buffer in one shot, so
//! "what the pump sees" and "what the speaker plays" are the same
//! samples by construction. A true output-stream tap becomes
//! necessary when TTS becomes streaming (partial PCM as tokens
//! arrive) or when the output stream can drop / mute / underrun
//! independently of what was enqueued. At that point this module
//! grows a `take_level_rx()` that emits per-window RMS from inside
//! the callback.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, Stream, StreamConfig};

/// Bounded backlog on the enqueue path. Real playback consumes at
/// device rate (~48 kHz), so 4 seconds of buffer at 48 kHz stereo is
/// ~1.5 MB — trivial, but plenty of headroom for a mock utterance
/// or several stacked short prompts without ever blocking `enqueue`.
const ENQUEUE_BACKLOG_SAMPLES: usize = 48_000 * 4;

#[derive(Debug, thiserror::Error)]
pub enum CpalPlaybackError {
    #[error("no output device available on this host")]
    NoOutputDevice,
    #[error("live playback skipped (RALLEH_SKIP_LIVE_AUDIO or CI without RALLEH_LIVE_PLAYBACK)")]
    SkippedByEnv,
    #[error("failed to query default output config: {0}")]
    DefaultConfig(String),
    #[error("unsupported sample format: {0:?}")]
    UnsupportedFormat(SampleFormat),
    #[error("failed to build output stream: {0}")]
    BuildStream(String),
    #[error("failed to start output stream: {0}")]
    PlayStream(String),
}

/// True when the operator explicitly wants live speaker playback
/// (`RALLEH_LIVE_PLAYBACK=1`). Same shape as `live_mic_requested`.
pub fn live_playback_requested() -> bool {
    matches!(
        std::env::var("RALLEH_LIVE_PLAYBACK").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
}

/// Soft-skip live playback: explicit skip, or CI hosts unless live
/// playback is requested. Mirrors `should_skip_live_audio` on the
/// input side so `voice_smoke` running in CI never has to guard.
pub fn should_skip_live_playback() -> bool {
    if std::env::var_os("RALLEH_SKIP_LIVE_AUDIO").is_some() {
        return true;
    }
    std::env::var_os("CI").is_some() && !live_playback_requested()
}

/// Live speaker sink. The cpal output stream is `!Send` on some
/// platforms (notably Windows WASAPI), so we can't store it
/// directly if we want the sink to be Tauri-manageable. Instead
/// the stream is owned by a dedicated background thread that opens
/// it, plays it, and holds it alive until `Drop` sends a shutdown
/// signal. The `queue` on this struct is `Arc<Mutex<...>>` and
/// therefore `Send + Sync`, so the facade satisfies Tauri's state
/// bounds without exposing the stream to callers.
///
/// This mirrors `MicPump`'s "open cpal on a thread" pattern in the
/// desktop-edge shell.
pub struct CpalPlaybackSink {
    queue: Arc<Mutex<VecDeque<Vec<f32>>>>,
    shutdown: Option<Sender<()>>,
    device_sample_rate_hz: u32,
    device_channels: u16,
}

impl CpalPlaybackSink {
    /// Open the host default output device. Hard-errors on hosts
    /// where playback is expected to work but the OS says no.
    pub fn open_default() -> Result<Self, CpalPlaybackError> {
        // Ready signal from the audio thread: it either builds the
        // stream and returns (rate, channels) or reports an error
        // synchronously so callers see the same failure they'd get
        // from a direct `Stream::build_output_stream` call.
        let (ready_tx, ready_rx) =
            mpsc::sync_channel::<Result<(u32, u16), CpalPlaybackError>>(1);
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
        let queue: Arc<Mutex<VecDeque<Vec<f32>>>> = Arc::new(Mutex::new(VecDeque::new()));
        let queue_thread = queue.clone();

        std::thread::Builder::new()
            .name("ralleh-audio-playback".into())
            .spawn(move || match open_stream(queue_thread) {
                Ok((stream, rate, channels)) => {
                    let _ = ready_tx.send(Ok((rate, channels)));
                    // Park until Drop signals shutdown. The stream
                    // is dropped last, cleanly stopping playback.
                    let _ = shutdown_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| CpalPlaybackError::BuildStream(format!("spawn thread: {e}")))?;

        let (device_sample_rate_hz, device_channels) = ready_rx
            .recv()
            .map_err(|_| CpalPlaybackError::BuildStream("audio thread exited early".into()))??;

        Ok(Self {
            queue,
            shutdown: Some(shutdown_tx),
            device_sample_rate_hz,
            device_channels,
        })
    }

    /// Best-effort open for tests and startup: CI / skip-env and any
    /// hardware failure become `None`. Use `open_default` in code
    /// paths that require audible output (e.g. an interactive TTS
    /// command in a UI-owning process).
    pub fn try_open_default() -> Option<Self> {
        if should_skip_live_playback() {
            return None;
        }
        Self::open_default().ok()
    }

    pub fn device_sample_rate_hz(&self) -> u32 {
        self.device_sample_rate_hz
    }

    pub fn device_channels(&self) -> u16 {
        self.device_channels
    }

    /// Queue PCM for playback. Mono input; the callback duplicates
    /// samples across channels for stereo devices. `src_sample_rate_hz`
    /// is resampled (linear) to `device_sample_rate_hz` — real
    /// production TTS should feed already-resampled audio for
    /// quality reasons, but the mock pipeline produces 16 kHz and the
    /// default output is typically 44.1 or 48 kHz.
    ///
    /// This method never blocks. If the backlog exceeds
    /// `ENQUEUE_BACKLOG_SAMPLES` (~4 s of device-rate audio) the
    /// oldest samples are dropped rather than growing the queue
    /// unboundedly — protects against pathological producers, and
    /// the audible artifact ("skip to newest utterance") is a better
    /// failure mode than "10 seconds of stale audio backs up".
    pub fn enqueue(&self, pcm: &[f32], src_sample_rate_hz: u32) {
        if pcm.is_empty() || src_sample_rate_hz == 0 {
            return;
        }
        let resampled = if src_sample_rate_hz == self.device_sample_rate_hz {
            pcm.to_vec()
        } else {
            resample_linear(pcm, src_sample_rate_hz, self.device_sample_rate_hz)
        };
        let Ok(mut queue) = self.queue.lock() else {
            return;
        };
        // Trim from the front if the caller is outrunning the
        // callback. Sum across all queued Vecs to know the backlog.
        let mut total: usize = queue.iter().map(|v| v.len()).sum();
        while total + resampled.len() > ENQUEUE_BACKLOG_SAMPLES {
            match queue.pop_front() {
                Some(dropped) => total = total.saturating_sub(dropped.len()),
                None => break,
            }
        }
        queue.push_back(resampled);
    }
}

impl Drop for CpalPlaybackSink {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// Open the cpal output stream and start playback. Called from the
/// dedicated audio thread so the returned `Stream` (`!Send` on some
/// platforms) never has to cross a thread boundary. Returns the
/// stream + the negotiated device sample rate / channel count.
fn open_stream(
    queue: Arc<Mutex<VecDeque<Vec<f32>>>>,
) -> Result<(Stream, u32, u16), CpalPlaybackError> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(CpalPlaybackError::NoOutputDevice)?;
    let supported = device
        .default_output_config()
        .map_err(|e| CpalPlaybackError::DefaultConfig(e.to_string()))?;

    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.clone().into();
    let device_sample_rate_hz = config.sample_rate.0;
    let device_channels = config.channels;

    let mut head_cursor: usize = 0;
    let queue_cb = queue;

    let stream = match sample_format {
        SampleFormat::F32 => {
            let queue = queue_cb.clone();
            build_output_stream::<f32, _>(&device, &config, move |output_samples| {
                fill_output(
                    output_samples,
                    &queue,
                    &mut head_cursor,
                    device_channels as usize,
                    f32::from_sample,
                )
            })?
        }
        SampleFormat::I16 => {
            let queue = queue_cb.clone();
            build_output_stream::<i16, _>(&device, &config, move |output_samples| {
                fill_output(
                    output_samples,
                    &queue,
                    &mut head_cursor,
                    device_channels as usize,
                    i16::from_sample,
                )
            })?
        }
        SampleFormat::U16 => {
            let queue = queue_cb.clone();
            build_output_stream::<u16, _>(&device, &config, move |output_samples| {
                fill_output(
                    output_samples,
                    &queue,
                    &mut head_cursor,
                    device_channels as usize,
                    u16::from_sample,
                )
            })?
        }
        other => return Err(CpalPlaybackError::UnsupportedFormat(other)),
    };

    stream
        .play()
        .map_err(|e| CpalPlaybackError::PlayStream(e.to_string()))?;

    Ok((stream, device_sample_rate_hz, device_channels))
}

/// Public so callers with an already-open sink and their own
/// out-of-band PCM can pre-resample without going through
/// `enqueue`. Also unit-testable without the audio hardware.
///
/// Linear interpolation between adjacent input samples. Good enough
/// for speech-band content at typical source→device ratios (16 kHz
/// → 48 kHz), not good enough for high-fidelity music -- but this
/// crate isn't a music player.
pub fn resample_linear(src: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if src.is_empty() || src_rate == 0 || dst_rate == 0 {
        return Vec::new();
    }
    if src_rate == dst_rate {
        return src.to_vec();
    }
    let ratio = src_rate as f64 / dst_rate as f64;
    // dst_len = src.len() / ratio, kept in f64 to avoid off-by-one
    // truncation on tiny buffers.
    let dst_len = ((src.len() as f64) / ratio).round() as usize;
    let mut dst = Vec::with_capacity(dst_len);
    for i in 0..dst_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        if idx + 1 >= src.len() {
            dst.push(src[src.len() - 1]);
            continue;
        }
        let frac = (src_pos - idx as f64) as f32;
        let a = src[idx];
        let b = src[idx + 1];
        dst.push(a + (b - a) * frac);
    }
    dst
}

fn build_output_stream<T, F>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut fill: F,
) -> Result<Stream, CpalPlaybackError>
where
    T: Sample + cpal::SizedSample + Send + 'static,
    F: FnMut(&mut [T]) + Send + 'static,
{
    let err_flag = Arc::new(Mutex::new(None::<String>));
    let err_flag_cb = err_flag.clone();
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                fill(data);
            },
            move |err| {
                if let Ok(mut slot) = err_flag_cb.lock() {
                    *slot = Some(err.to_string());
                }
            },
            None,
        )
        .map_err(|e| CpalPlaybackError::BuildStream(e.to_string()))
}

/// Callback body, extracted so the four sample-format branches
/// share exactly one code path. Reads mono f32 samples from the
/// shared queue, duplicates each into `channels` interleaved slots,
/// and converts to the device sample type via `to_native`.
fn fill_output<T, C>(
    output: &mut [T],
    queue: &Arc<Mutex<VecDeque<Vec<f32>>>>,
    head_cursor: &mut usize,
    channels: usize,
    to_native: C,
) where
    T: Sample,
    C: Fn(f32) -> T,
{
    // We can't afford to hold the lock for the whole callback if it
    // ever gets contended -- keep the critical section tight by
    // taking the whole queue out, doing our work, and putting the
    // remainder back. In steady state (single enqueue producer,
    // single callback consumer) contention is nil, so this is
    // effectively free.
    let mut local: VecDeque<Vec<f32>> = match queue.lock() {
        Ok(mut g) => std::mem::take(&mut *g),
        Err(_) => return,
    };

    let mut out_idx = 0usize;
    while out_idx + channels <= output.len() {
        let sample = loop {
            match local.front() {
                Some(v) if *head_cursor < v.len() => {
                    let s = v[*head_cursor];
                    *head_cursor += 1;
                    break s;
                }
                Some(_) => {
                    // Head Vec exhausted -- drop it and try the next.
                    local.pop_front();
                    *head_cursor = 0;
                }
                None => break 0.0,
            }
        };
        for ch in 0..channels {
            output[out_idx + ch] = to_native(sample);
        }
        out_idx += channels;
    }
    // Zero any leftover slot (partial trailing channel group -- rare
    // but possible on weird buffer sizes).
    for slot in output.iter_mut().skip(out_idx) {
        *slot = to_native(0.0);
    }

    if let Ok(mut g) = queue.lock() {
        // Restore whatever remains. Order matters: our leftovers
        // must precede anything the producer pushed while we were
        // in the callback body.
        while let Some(v) = local.pop_back() {
            g.push_front(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_linear_is_identity_when_rates_match() {
        let src: Vec<f32> = (0..1000).map(|i| (i as f32) / 1000.0).collect();
        let dst = resample_linear(&src, 48_000, 48_000);
        assert_eq!(src, dst);
    }

    #[test]
    fn resample_linear_upsamples_length_correctly() {
        // 16 kHz → 48 kHz = 3x length within rounding.
        let src = vec![0.0_f32; 160]; // 10 ms at 16 kHz
        let dst = resample_linear(&src, 16_000, 48_000);
        // 10 ms at 48 kHz = 480 samples.
        assert!(
            (dst.len() as i64 - 480).abs() <= 1,
            "expected ~480 samples, got {}",
            dst.len()
        );
    }

    #[test]
    fn resample_linear_downsamples_length_correctly() {
        let src = vec![0.0_f32; 480]; // 10 ms at 48 kHz
        let dst = resample_linear(&src, 48_000, 16_000);
        assert!(
            (dst.len() as i64 - 160).abs() <= 1,
            "expected ~160 samples, got {}",
            dst.len()
        );
    }

    #[test]
    fn resample_linear_preserves_ramp_amplitude() {
        // A linear ramp 0.0 → 1.0 must remain a monotonically
        // non-decreasing sequence bounded by [0, 1] after
        // resampling -- linear interpolation cannot introduce
        // overshoot on a monotonic input.
        let src: Vec<f32> = (0..100).map(|i| (i as f32) / 99.0).collect();
        let dst = resample_linear(&src, 16_000, 48_000);
        for pair in dst.windows(2) {
            assert!(pair[1] >= pair[0] - 1e-6, "ramp went backwards: {pair:?}");
            assert!(pair[0] >= -1e-6 && pair[0] <= 1.0 + 1e-6);
        }
    }

    #[test]
    fn resample_linear_handles_empty_and_zero_rate() {
        assert!(resample_linear(&[], 16_000, 48_000).is_empty());
        assert!(resample_linear(&[0.1, 0.2], 0, 48_000).is_empty());
        assert!(resample_linear(&[0.1, 0.2], 16_000, 0).is_empty());
    }

    #[test]
    fn try_open_default_never_hard_fails() {
        // With or without an output device / under CI skip: always
        // None or Some. Same contract as CpalMicSource.
        if let Some(sink) = CpalPlaybackSink::try_open_default() {
            assert!(sink.device_sample_rate_hz() > 0);
            assert!(sink.device_channels() >= 1);
            // Enqueue a tiny silent buffer -- must not panic.
            sink.enqueue(&vec![0.0_f32; 320], 16_000);
        }
    }

    /// Opt-in live playback proof:
    ///   RALLEH_LIVE_PLAYBACK=1 cargo test -p ralleh-audio-core --features playback -- --ignored live_playback
    #[test]
    #[ignore = "requires RALLEH_LIVE_PLAYBACK=1, --features playback, and a working output device"]
    fn live_playback_smoke_when_explicitly_enabled() {
        assert!(
            live_playback_requested(),
            "set RALLEH_LIVE_PLAYBACK=1 to run this smoke"
        );
        let sink = CpalPlaybackSink::open_default().expect("output should open when live requested");
        // ~500 ms of a quiet 440 Hz tone.
        let rate = 16_000u32;
        let samples: Vec<f32> = (0..(rate as usize / 2))
            .map(|i| {
                let t = i as f32 / rate as f32;
                0.1 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect();
        sink.enqueue(&samples, rate);
        std::thread::sleep(std::time::Duration::from_millis(700));
    }
}
