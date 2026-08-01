//! ralleh-audio-core
//!
//! Audio pipeline core for Ralleh's edge agent: capture, VAD (voice activity
//! detection), and wake/push-to-talk triggering.
//!
//! ## Current status: MOCKED input, real pipeline logic
//!
//! The current build host has no microphone/audio hardware (headless VPS),
//! so live device capture is out of scope for now (per explicit decision,
//! 2026-08-01). This crate is built around an `AudioSource` trait boundary
//! so that:
//!   - Pipeline logic (VAD, buffering, trigger detection) can be fully
//!     unit-tested today using a `MockAudioSource` that plays back
//!     synthetic/simulated audio frames.
//!   - A real device backend (e.g. `cpal`) can be added later as a second
//!     `AudioSource` implementation with zero changes to consumers, once
//!     testing happens on a machine with a real microphone.
//!
//! Do not treat "mocked" as "untested" — the VAD/state-machine logic here
//! has the same bar for correctness as any other core module; only the
//! literal hardware I/O is simulated.

mod source;
mod vad;
mod wakeword;

pub use source::{AudioFrame, AudioSource, MockAudioSource};
pub use vad::{VadConfig, VadState, VoiceActivityDetector};
pub use wakeword::{
    MockWakeWordMatcher, WakeWordConfig, WakeWordDetector, WakeWordMatcher, WakeWordTrigger,
};
