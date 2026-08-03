//! Live-mic → presence pump.
//!
//! Owns a `CpalMicSource` on a background thread, computes a smoothed
//! RMS level from every frame, and pushes it to the presence runtime as
//! `Command::SetSignalsScalars` at ~30 Hz. Scalars-only on purpose —
//! the pump has no business touching mode engagement, and using the
//! authoritative-modes `SetSignals` here would let a stray mic packet
//! race with a user-initiated mode toggle.
//!
//! # Feature-gating
//!
//! Requires the crate-level `mic` feature (default in this shell). Under
//! `--no-default-features` the whole module compiles as an inert stub
//! whose `start` always returns an "unavailable" error — so a headless
//! build can still link against `desktop-edge` without pulling in
//! `cpal`.
//!
//! # Policy
//!
//! The caller is responsible for verifying `EdgeSettings::mic_acknowledged`
//! before calling `start` (the Tauri command wrapping this module does
//! that check). This module itself is signal-plumbing — the same
//! separation as `run_mic_smoke` uses.

use std::sync::mpsc::Sender;

use presence_ipc::{Command, Envelope};

#[cfg(feature = "mic")]
use presence_ipc::PresenceMode;
#[cfg(feature = "mic")]
use ralleh_audio_core::{AudioSource, CpalMicSource, VadConfig, VadState, VoiceActivityDetector};
#[cfg(feature = "mic")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "mic")]
use std::sync::Arc;
#[cfg(feature = "mic")]
use std::thread::{self, JoinHandle};
#[cfg(feature = "mic")]
use std::time::{Duration, Instant};

/// Nominal send cadence. 30 Hz is the same figure the React slider
/// throttle uses; anything higher is wasted (the runtime ticks the
/// simulation at 60 Hz and any two consecutive `SetSignalsScalars`
/// within one sim tick collapse to the last one). Keeping the two
/// consumers aligned makes it easier to reason about latency later.
#[cfg(feature = "mic")]
const SEND_INTERVAL: Duration = Duration::from_millis(33);

/// EMA smoothing factor for the audio level. Fast enough that a spoken
/// word visibly modulates the shell, slow enough that a single spike
/// from clothing rustle does not fire a "user is talking" pulse. The
/// value is deliberately close to the shell's own internal audio
/// smoothing so a peak sees the same envelope shape on both sides.
#[cfg(feature = "mic")]
const LEVEL_ATTACK: f32 = 0.25;
#[cfg(feature = "mic")]
const LEVEL_RELEASE: f32 = 0.08;

/// Gain applied to raw RMS. Voice RMS on a nearfield mic sits around
/// 0.05..0.15 with occasional peaks near 0.3; multiplying by ~5 lands
/// speaking peaks near 1.0 which is where the shell's audio path
/// expects them. Not a calibrated figure — this is a first-cut that
/// gets the visual response into the right region and can be tuned
/// against real captures later.
#[cfg(feature = "mic")]
const LEVEL_GAIN: f32 = 5.0;

/// Handle to a running mic pump. Dropping it stops the thread.
pub struct MicPump {
    #[cfg(feature = "mic")]
    stop: Arc<AtomicBool>,
    #[cfg(feature = "mic")]
    join: Option<JoinHandle<()>>,
}

impl MicPump {
    /// Open the default input device and start pumping. On success,
    /// SetSignalsScalars packets start reaching the presence within a
    /// few frames (~40 ms). Errors are one of: no device on this host,
    /// `mic` feature disabled at compile time, or the input backend
    /// failed to build the stream.
    #[cfg(feature = "mic")]
    pub fn start(sender: Sender<Envelope>) -> Result<Self, String> {
        // `CpalMicSource` owns a `cpal::Stream`, which is `!Send` on
        // Windows (WASAPI) and on macOS. That means the device has to
        // be opened *inside* the pump thread, not before spawning it.
        // To keep the "device failure is a synchronous error" contract
        // that the Tauri command relies on, the thread opens the
        // source and reports the outcome on a one-shot channel; this
        // function blocks on that channel before returning.
        use std::sync::mpsc as std_mpsc;

        let (open_tx, open_rx) = std_mpsc::sync_channel::<Result<u32, String>>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let join = thread::Builder::new()
            .name("presence-mic-pump".into())
            .spawn(move || {
                let source = match CpalMicSource::open_default() {
                    Ok(s) => {
                        let _ = open_tx.send(Ok(s.sample_rate_hz()));
                        s
                    }
                    Err(e) => {
                        let _ = open_tx.send(Err(format!("presence mic: open failed: {e}")));
                        return;
                    }
                };
                pump_loop(source, sender, stop_thread);
            })
            .map_err(|e| format!("presence mic: spawn: {e}"))?;

        match open_rx.recv() {
            Ok(Ok(rate_hz)) => {
                log::info!("desktop-edge: presence mic pump started, {rate_hz} Hz");
                Ok(Self {
                    stop,
                    join: Some(join),
                })
            }
            Ok(Err(err)) => {
                // Thread already returned; joining is cheap and lets us
                // surface any panic that might have escaped the match.
                let _ = join.join();
                Err(err)
            }
            Err(_) => {
                let _ = join.join();
                Err("presence mic: open handshake dropped".into())
            }
        }
    }

    #[cfg(not(feature = "mic"))]
    pub fn start(_sender: Sender<Envelope>) -> Result<Self, String> {
        Err(
            "presence mic: this shell was built without the `mic` feature — \
             rebuild with scripts/tauri-dev.cmd (mic is default there)"
                .into(),
        )
    }

    /// Requests the pump thread to stop and joins it. Idempotent.
    #[cfg(feature = "mic")]
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    #[cfg(not(feature = "mic"))]
    pub fn stop(&mut self) {}
}

impl Drop for MicPump {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(feature = "mic")]
fn pump_loop(mut source: CpalMicSource, sender: Sender<Envelope>, stop: Arc<AtomicBool>) {
    // Two-timescale smoother: attack fast (level rises quickly on a
    // spoken syllable), release slow (falls gently so the shell does not
    // strobe on gaps between words). Distinct constants matter — a
    // symmetric EMA either misses the peak or lingers at it.
    let mut level: f32 = 0.0;
    let mut last_sent = Instant::now();

    // Phase 3 kickoff: a real signal drives `PresenceMode::Listening`.
    // Same detector `run_mic_smoke` uses, run alongside the RMS
    // integrator so the visual has *two* sources of truth from one
    // frame — the audio level (continuous, smooth) and the debounced
    // VAD verdict (discrete, hysteretic). We only send a mode change
    // when the debounced state crosses the Speech boundary; the
    // detector's own hysteresis (2 loud frames in, 3 silent frames
    // out with `VadConfig::default`) is what keeps the presence from
    // flickering mid-sentence or on a cough.
    //
    // Reusing `VadConfig::default()` keeps this in step with the mic
    // smoke — if that threshold ever proves wrong in real captures,
    // both paths get corrected together rather than drifting.
    let mut vad = VoiceActivityDetector::new(VadConfig::default());
    let mut last_listening = false;

    // Poll cadence: fast enough that the level's attack phase is not
    // aliased away, slow enough that this thread does not spin. 5 ms
    // is well below the 33 ms send interval and well above the ~20 ms
    // period between cpal frames on a typical config, so the RMS
    // integrator sees every frame at least once.
    let poll = Duration::from_millis(5);

    while !stop.load(Ordering::Relaxed) {
        // Drain everything the cpal thread has produced since the last
        // poll. Multiple frames per poll are normal on a fast host and
        // must all be folded into the smoother rather than the loop
        // picking one and dropping the rest.
        while let Some(frame) = source.next_frame() {
            let raw = (frame.rms_energy() * LEVEL_GAIN).min(1.0);
            let alpha = if raw > level {
                LEVEL_ATTACK
            } else {
                LEVEL_RELEASE
            };
            level += (raw - level) * alpha;

            // VAD updates on every frame, but we only fire a mode
            // change on the debounced Speech transition. `MaybeSpeech`
            // and `MaybeSilence` are held-off states — they exist so a
            // single-frame spike or a mid-word pause does not create a
            // visible flicker.
            let vad_state = vad.process_frame(&frame);
            let now_listening = matches!(vad_state, VadState::Speech);
            if now_listening != last_listening {
                let env = Envelope::wrap(Command::SetMode {
                    mode: PresenceMode::Listening,
                    engaged: now_listening,
                });
                if sender.send(env).is_err() {
                    log::warn!(
                        "desktop-edge: presence writer disconnected; \
                         mic pump exiting mid-VAD transition"
                    );
                    return;
                }
                last_listening = now_listening;
            }
        }

        let now = Instant::now();
        if now.duration_since(last_sent) >= SEND_INTERVAL {
            let env = Envelope::wrap(Command::SetSignalsScalars {
                // `intensity` and `progress` are not the mic's business
                // — leaving them at 0.0 would clobber whatever the UI
                // or a future subscriber has set. But the scalars-only
                // command is atomic on all three fields on the wire, so
                // we forward the current-audio-level with the other two
                // reset to *idle-ish* values only if no other source is
                // writing them. In the current build there is no other
                // source of `intensity` at this cadence, so mirroring
                // the shell's idle default here keeps the presence at
                // its resting brightness. Revisit when a second scalar
                // pump lands.
                intensity: 0.15,
                audio_level: level.clamp(0.0, 1.0),
                progress: 0.0,
            });
            if sender.send(env).is_err() {
                log::warn!("desktop-edge: presence writer disconnected; mic pump exiting");
                return;
            }
            last_sent = now;
        }

        thread::sleep(poll);
    }

    // Graceful stop: release Listening if we were holding it. Without
    // this the runtime would keep the mode engaged after the user
    // clicked "Mic pump" off, and it would only clear on the next
    // shell-authored `SetSignals` snapshot — which might not arrive
    // for a while.
    if last_listening {
        let _ = sender.send(Envelope::wrap(Command::SetMode {
            mode: PresenceMode::Listening,
            engaged: false,
        }));
    }
    log::info!("desktop-edge: presence mic pump stopped");
}

#[cfg(all(test, not(feature = "mic")))]
mod tests {
    use super::*;

    #[test]
    fn start_without_the_mic_feature_returns_a_clear_error() {
        // Under `--no-default-features` the pump must not compile in
        // cpal machinery; `start` should return a human-readable error
        // that a developer can act on rather than a silent no-op or a
        // link error.
        let (tx, _rx) = std::sync::mpsc::channel();
        let err = MicPump::start(tx).unwrap_err();
        assert!(err.contains("mic"), "{err}");
    }
}

// Live mic behaviour is exercised by the manual smoke path in the dev
// panel and by the ignored `live_mic_smoke_when_explicitly_enabled` test
// in `ralleh-audio-core` — reproducing it here would double the opt-in
// surface without adding coverage.
