//! Wire types shared between the presence renderer (`presence-core` /
//! `presence-runtime`, at `../../presence-prototype/`) and its shell host
//! (Tauri edge at `../../desktop-edge/src-tauri/`).
//!
//! # Why this crate exists
//!
//! Phase 2 §2 of `../../docs/PRESENCE_INTEGRATION_PLAN.md`: the shell drives
//! the presence with a stream of signals ("intensity is now 0.7", "speaking
//! turned on") and occasional commands ("switch to Low quality", "use the
//! Ember palette"). Both sides need to agree on the exact shape of those
//! payloads, and neither of them is a natural owner:
//!
//! - `presence-core` is a `winit` + `wgpu` renderer and can't sit in a
//!   headless-CI workspace member.
//! - `desktop-edge/src-tauri` is a Tauri app and can't either.
//! - The wire types are pure `serde`, and belong somewhere that *does*
//!   build headless — this crate, in the root workspace's `crates/`
//!   directory.
//!
//! # Design notes
//!
//! - Enums are `#[serde(rename_all = "snake_case")]` so the persisted /
//!   over-the-wire spelling matches the labels shown in the debug panel
//!   (`PresenceMode::label`, `PaletteId::as_str`). Renaming a variant is a
//!   settings-migration event, not a refactor.
//! - Every message travels inside an [`Envelope`] carrying [`VERSION`]. The
//!   version is a plain `u32`, bumped when a variant changes shape or a
//!   field's semantic meaning shifts — additive fields on structs are
//!   backwards-compatible under `serde(default)` and do not need a bump.
//! - `Command` is `#[non_exhaustive]` so adding a new variant later is not a
//!   breaking change for downstream matches (they'll get a compile-time
//!   nudge to add an arm, which is the point of the annotation).
//! - This crate does *not* provide conversions to the equivalent types in
//!   `presence-core`. Those live in `presence-core` behind its `ipc`
//!   feature — putting them here would force `presence-ipc` to depend on
//!   `presence-core`, which cannot build headless.

use serde::{Deserialize, Serialize};

/// The current wire version. Bumped when a message's shape or a field's
/// semantic meaning changes; additive optional fields do not require a
/// bump. Peers that receive a version they don't recognise should treat
/// the message as unrecoverable and log rather than guess.
pub const VERSION: u32 = 1;

/// A visual mode the presence can be told is active. Mirrors
/// `presence_core::scene::mode::PresenceMode`, but is the canonical
/// serialization: the string spellings here are what get persisted and
/// what the shell sends over the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceMode {
    Thinking,
    Speaking,
    ToolUse,
    Listening,
    Attention,
    Error,
}

/// A performance tier — the shell picks it (or leaves auto-downshift to
/// the runtime) and the presence adopts it on the next tick. Mirrors
/// `presence_core::scene::QualityTier`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    Balanced,
    Low,
}

/// A colour scheme. Mirrors `presence_core::palette::PaletteId`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteId {
    Teal,
    Lime,
    Ice,
    Ember,
}

/// Continuous signals the shell pushes to the presence. Matches
/// `presence_core::sim::PresenceSignals` plus the set of currently
/// engaged modes (`active_modes` — the modes-are-signals framing lives
/// here rather than in `PresenceSignals` because the shell is authoritative
/// for engagement, and the mode set changes at a very different cadence
/// from the scalar signals).
///
/// All scalar fields are clamped by the receiver to `[0.0, 1.0]` (or to
/// `[0.0, 1.5]` for `intensity`, matching the debug slider's range) —
/// senders should still send in-range values, but receivers must never
/// panic on out-of-range input.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Signals {
    #[serde(default)]
    pub intensity: f32,
    #[serde(default)]
    pub audio_level: f32,
    #[serde(default)]
    pub progress: f32,
    #[serde(default)]
    pub active_modes: Vec<PresenceMode>,
}

/// A command the shell issues to the presence. Enum rather than a set of
/// separate messages because the presence should apply every command in
/// the order it arrived, and a single tagged type is the easiest way to
/// keep that ordering explicit.
///
/// `#[non_exhaustive]` — see the crate-level docs for the rationale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Command {
    /// Replaces the presence's continuous signal state wholesale.
    /// **Authoritative** on `active_modes` — modes not in the list are
    /// released. Use this when the shell has a snapshot of the whole
    /// desired state to converge on; use [`Command::SetSignalsScalars`]
    /// when you want to update only the scalars (e.g. a mic pump that
    /// has no business touching mode engagement).
    SetSignals(Signals),
    /// Scalars-only signal update. The presence updates
    /// `intensity`, `audio_level`, and `progress` but leaves the
    /// engaged-modes set untouched. This is the hot path the mic pump
    /// uses — it fires many times per second and must never
    /// accidentally release a mode the shell engaged for a different
    /// reason.
    SetSignalsScalars {
        intensity: f32,
        audio_level: f32,
        progress: f32,
    },
    /// Engages or disengages a single mode. The presence handles fades on
    /// its own timeline (`docs/adr/adr-012-additive-mode-composition.md`);
    /// this message just flips the "wanted" state.
    SetMode { mode: PresenceMode, engaged: bool },
    /// Shows or hides the Loading entity. Kept separate from `SetMode`
    /// because Loading is a second entity in `SceneRegistry`, not a mode
    /// on the main shell.
    //
    // Struct-form (`{ wanted: bool }`) rather than newtype (`(bool)`)
    // because serde's internally-tagged enum representation cannot serialize
    // a newtype variant that wraps a primitive — there is nowhere to put
    // the payload. Every variant on this enum is struct-form for that
    // reason and for consistency across the wire format.
    SetRingWanted { wanted: bool },
    /// Applies the reduced-motion accessibility preset.
    SetReducedMotion { enabled: bool },
    /// Manually pins the quality tier. Adaptive downshift keeps running
    /// after this; the runtime just starts its search from the pinned
    /// tier rather than `Balanced`.
    SetQualityTier { tier: QualityTier },
    /// Switches the colour scheme.
    SetPalette { palette: PaletteId },
}

/// Every message on the wire is wrapped in one of these so a peer can
/// reject a mismatched version cleanly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u32,
    pub payload: Command,
}

impl Envelope {
    /// Wraps `payload` in an envelope stamped with the current [`VERSION`].
    pub fn wrap(payload: Command) -> Self {
        Self {
            version: VERSION,
            payload,
        }
    }

    /// True iff `self.version` matches the version this build was compiled
    /// against. A receiver should call this before matching on `payload`.
    pub fn is_current(&self) -> bool {
        self.version == VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_round_trips_by_its_wire_name() {
        // Explicit list rather than a helper — the point of this test is
        // to nail down the exact strings that get persisted, so anything
        // that renames a mode has to update this list and see it.
        let pairs: &[(PresenceMode, &str)] = &[
            (PresenceMode::Thinking, "thinking"),
            (PresenceMode::Speaking, "speaking"),
            (PresenceMode::ToolUse, "tool_use"),
            (PresenceMode::Listening, "listening"),
            (PresenceMode::Attention, "attention"),
            (PresenceMode::Error, "error"),
        ];
        for (mode, wire) in pairs {
            let encoded = serde_json::to_string(mode).expect("serialize");
            assert_eq!(encoded, format!("\"{wire}\""), "wire name for {mode:?}");
            let decoded: PresenceMode =
                serde_json::from_str(&encoded).expect("deserialize");
            assert_eq!(decoded, *mode);
        }
    }

    #[test]
    fn quality_and_palette_wire_names_are_the_ones_the_settings_use() {
        assert_eq!(
            serde_json::to_string(&QualityTier::Balanced).unwrap(),
            "\"balanced\""
        );
        assert_eq!(
            serde_json::to_string(&QualityTier::Low).unwrap(),
            "\"low\""
        );
        assert_eq!(
            serde_json::to_string(&PaletteId::Teal).unwrap(),
            "\"teal\""
        );
        assert_eq!(
            serde_json::to_string(&PaletteId::Ember).unwrap(),
            "\"ember\""
        );
    }

    #[test]
    fn signals_defaults_are_the_idle_defaults() {
        // Not testing `PresenceSignals`'s in-crate default (0.15 intensity)
        // — that is presence-core's business. `Signals::default` here is
        // "everything zero" so an unset field on the wire is unambiguous.
        let s = Signals::default();
        assert_eq!(s.intensity, 0.0);
        assert_eq!(s.audio_level, 0.0);
        assert_eq!(s.progress, 0.0);
        assert!(s.active_modes.is_empty());
    }

    #[test]
    fn signals_field_omissions_deserialize_to_default() {
        // The `#[serde(default)]` on every field is the reason additive
        // fields are backwards-compatible without bumping VERSION; if this
        // test fails, so does the compat story in the crate-level docs.
        let s: Signals = serde_json::from_str("{}").expect("empty object");
        assert_eq!(s, Signals::default());

        let partial: Signals =
            serde_json::from_str(r#"{"intensity":0.5}"#).expect("partial");
        assert_eq!(partial.intensity, 0.5);
        assert_eq!(partial.audio_level, 0.0);
    }

    #[test]
    fn every_command_round_trips_through_the_envelope() {
        let commands = [
            Command::SetSignals(Signals {
                intensity: 0.7,
                audio_level: 0.3,
                progress: 0.0,
                active_modes: vec![PresenceMode::Speaking],
            }),
            Command::SetMode {
                mode: PresenceMode::Thinking,
                engaged: true,
            },
            Command::SetSignalsScalars {
                intensity: 0.5,
                audio_level: 0.9,
                progress: 0.0,
            },
            Command::SetRingWanted { wanted: true },
            Command::SetReducedMotion { enabled: false },
            Command::SetQualityTier {
                tier: QualityTier::Low,
            },
            Command::SetPalette {
                palette: PaletteId::Ember,
            },
        ];
        for cmd in commands {
            let env = Envelope::wrap(cmd.clone());
            assert!(env.is_current());
            let encoded = serde_json::to_string(&env).expect("serialize");
            let decoded: Envelope =
                serde_json::from_str(&encoded).expect("deserialize");
            assert_eq!(decoded, env);
        }
    }

    #[test]
    fn envelope_rejects_a_stale_version() {
        // What "reject" means is up to the caller — this crate's contract
        // is just that `is_current` returns false, not that decode itself
        // fails. That keeps the receiver in control of logging vs.
        // erroring out.
        let stale = Envelope {
            version: VERSION.wrapping_sub(1),
            payload: Command::SetReducedMotion { enabled: true },
        };
        let encoded = serde_json::to_string(&stale).unwrap();
        let decoded: Envelope = serde_json::from_str(&encoded).unwrap();
        assert!(!decoded.is_current());
    }

    #[test]
    fn command_serializes_with_a_kind_tag() {
        // The wire format is `{"kind":"...", ...}`, not
        // `{"SetMode":{...}}`. That is what the desktop-edge side is
        // going to pattern-match against and what a settings dump has to
        // look like, so it is worth pinning down here.
        let cmd = Command::SetMode {
            mode: PresenceMode::ToolUse,
            engaged: true,
        };
        let encoded = serde_json::to_string(&cmd).unwrap();
        assert!(
            encoded.contains(r#""kind":"set_mode""#),
            "expected tag: {encoded}"
        );
        assert!(encoded.contains(r#""mode":"tool_use""#), "mode: {encoded}");
    }
}
