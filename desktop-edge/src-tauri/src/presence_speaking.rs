//! Speaking-side audio-level pump (Phase 3 §3.3 follow-up).
//!
//! `Presence::pulse_speaking` engages the mode for the wall-clock
//! duration of a synthesized utterance, but until now the visual
//! was flat inside that hold — a constant "speaking" pose rather
//! than one that follows the syllable envelope. This module fills
//! the gap by chunking PCM into ~30 Hz windows, computing an RMS
//! per window, and pushing the result through the shell's
//! `Command::SetSignalsScalars` channel — exactly the same wire
//! path `presence_mic::MicPump` uses on the listening side.
//!
//! # Why a pre-computed buffer instead of real playback
//!
//! Real cpal playback isn't wired yet — the shell's TTS runs on
//! `MockTts`, which produces its PCM in one shot. Feeding the pump
//! the pre-computed buffer today proves the wire end-to-end and
//! turns "speaking" into a modulated visual immediately. When
//! playback lands, the pump moves from `Vec<f32>` samples to a
//! ringbuffer tap on the output stream without the presence
//! runtime noticing — same envelope shape, same cadence.
//!
//! # Coexistence with `MicPump`
//!
//! Both pumps write to `SetSignalsScalars`. That's intentional: mic
//! level and speaking level are mutually exclusive in the visual
//! grammar (the assistant isn't listening while it's talking), so
//! the fact that one overwrites the other is the correct behavior,
//! not a race. Sequencing is guaranteed by the wall-clock: the
//! speaking pulse only starts after `voice_smoke` returns success,
//! at which point the mic pump — if enabled — is still driving
//! low-level `idle-ish` scalars that the speaking pump immediately
//! stomps for the duration of the utterance.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use presence_ipc::{Command, Envelope};

/// Target scalar-update rate. Matches `MicPump` so the presence
/// runtime sees one consistent cadence across all scalar sources.
const TARGET_HZ: u32 = 30;

/// EMA smoothing on the per-chunk RMS so a single quiet or loud
/// sample doesn't produce a step function in the visual. 0.35
/// mirrors the mic pump; matching them keeps the two paths visually
/// interchangeable when the operator toggles between listening and
/// speaking states.
const EMA_ALPHA: f32 = 0.35;

/// Multiplier applied to raw RMS before clamping into `[0, 1]`.
/// `MockTts` emits alternating `±0.2` samples, whose RMS is 0.2 —
/// scaling by 3.0 lifts that to a visible ~0.6 so the mock utterance
/// reads as speech rather than as background noise. Real playback
/// will have a wider dynamic range and this constant will move to
/// a per-source config.
const RMS_GAIN: f32 = 3.0;

/// Baseline `intensity` published alongside `audio_level` while a
/// speaking pulse is active. Matches the "energized but not loud"
/// reading that the mode itself already carries — the scalar is
/// there so the surface behavior has a term to modulate against
/// rather than a bare 0.
const SPEAKING_INTENSITY: f32 = 0.4;

/// Baseline `intensity` returned after the pulse ends. Mirrors
/// `MicPump`'s idle-ish default so a mic-off shutdown does not
/// leave the presence stuck at speaking-level intensity.
const IDLE_INTENSITY: f32 = 0.15;

/// Spawns a detached background thread that pumps `audio_level`
/// from `samples` at ~30 Hz. No handle is returned: the pump
/// runs to completion (samples exhausted) or exits on a broken
/// pipe. Callers are expected to have already fired
/// `Presence::pulse_speaking` on the same wall-clock so the mode
/// engagement and the scalar envelope share a lifetime — see
/// module docs.
///
/// A cap keeps the pump from monopolizing scalars on a
/// pathological-length utterance: after 30 s, the pump exits
/// regardless. Real speech longer than that has bigger problems
/// than scalar cadence.
pub fn spawn(samples: Vec<f32>, sample_rate_hz: u32, tx: Sender<Envelope>) {
    if samples.is_empty() || sample_rate_hz == 0 {
        return;
    }
    thread::Builder::new()
        .name("presence-speaking-pump".into())
        .spawn(move || pump(samples, sample_rate_hz, tx))
        .expect("spawn presence-speaking-pump thread");
}

fn pump(samples: Vec<f32>, sample_rate_hz: u32, tx: Sender<Envelope>) {
    let chunk_samples = ((sample_rate_hz / TARGET_HZ) as usize).max(1);
    let chunk_dur = Duration::from_millis((1_000 / TARGET_HZ) as u64);
    // Wall-clock cap. See fn docs.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut ema: f32 = 0.0;

    for chunk in samples.chunks(chunk_samples) {
        if Instant::now() >= deadline {
            break;
        }
        let level = rms_scaled(chunk);
        ema += (level - ema) * EMA_ALPHA;
        let env = Envelope::wrap(Command::SetSignalsScalars {
            intensity: SPEAKING_INTENSITY,
            audio_level: ema.clamp(0.0, 1.0),
            progress: 0.0,
        });
        if tx.send(env).is_err() {
            return;
        }
        thread::sleep(chunk_dur);
    }

    // Final zero — otherwise the last EMA sample lingers as a
    // constant `audio_level` after the mode releases, which reads
    // as the assistant still speaking silently.
    let _ = tx.send(Envelope::wrap(Command::SetSignalsScalars {
        intensity: IDLE_INTENSITY,
        audio_level: 0.0,
        progress: 0.0,
    }));
}

fn rms_scaled(chunk: &[f32]) -> f32 {
    if chunk.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = chunk.iter().map(|s| s * s).sum();
    let rms = (sum_sq / (chunk.len() as f32)).sqrt();
    (rms * RMS_GAIN).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_scaled_matches_mock_tts_shape() {
        // MockTts emits alternating +/-0.2 samples. RMS = 0.2.
        // With the 3.0 gain, this lifts to ~0.6 — the value the
        // visual should read on a mock utterance.
        let chunk: Vec<f32> = (0..1024)
            .map(|i| if i % 2 == 0 { 0.2 } else { -0.2 })
            .collect();
        let level = rms_scaled(&chunk);
        assert!((level - 0.6).abs() < 1e-3, "expected ~0.6, got {level}");
    }

    #[test]
    fn rms_scaled_clamps_at_one_for_loud_input() {
        let chunk = vec![1.0_f32; 512];
        assert_eq!(rms_scaled(&chunk), 1.0);
    }

    #[test]
    fn rms_scaled_returns_zero_for_empty_chunk() {
        assert_eq!(rms_scaled(&[]), 0.0);
    }

    #[test]
    fn spawn_is_a_noop_on_empty_samples() {
        // No panic, no thread — the caller should not need to
        // guard around this. `sender_disconnected` isn't observed
        // because no thread ever starts.
        let (tx, _rx) = std::sync::mpsc::channel();
        spawn(vec![], 16_000, tx);
    }

    #[test]
    fn spawn_is_a_noop_on_zero_sample_rate() {
        let (tx, _rx) = std::sync::mpsc::channel();
        spawn(vec![0.1, 0.2, 0.3], 0, tx);
    }

    #[test]
    fn pump_emits_scalars_and_a_final_zero() {
        // Short synthetic buffer: 3 chunks worth at 30 Hz / 16 kHz.
        // That's 3 * (16_000 / 30) ≈ 1600 samples. Use 2000 to
        // guarantee at least 3 sends plus the terminal zero.
        let samples: Vec<f32> = (0..2000)
            .map(|i| if i % 2 == 0 { 0.2 } else { -0.2 })
            .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        // Run the pump inline rather than spawning so the test does
        // not depend on thread scheduling — `pump` is pub(crate)
        // via the module boundary and takes ownership of `samples`
        // the same way `spawn` does.
        pump(samples, 16_000, tx);

        let envs: Vec<Envelope> = rx.into_iter().collect();
        assert!(
            envs.len() >= 4,
            "expected at least 3 scalar sends + 1 terminal zero, got {}",
            envs.len()
        );
        // Final envelope must zero audio_level so the visual does
        // not linger on the last EMA sample.
        let last = envs.last().unwrap();
        match &last.payload {
            Command::SetSignalsScalars {
                audio_level,
                intensity,
                ..
            } => {
                assert_eq!(*audio_level, 0.0, "final audio_level must be 0");
                assert_eq!(*intensity, IDLE_INTENSITY);
            }
            other => panic!("expected SetSignalsScalars, got {other:?}"),
        }
    }
}
