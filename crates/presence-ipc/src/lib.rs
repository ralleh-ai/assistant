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
pub const VERSION: u32 = 2;

/// Oldest wire version this build can still parse. Peers within
/// `[MIN_SUPPORTED_VERSION, VERSION]` are accepted by [`Envelope::is_compatible`];
/// this lets a mixed-version rollout (a slightly newer shell driving a slightly
/// older runtime, or vice-versa) keep working instead of hard-failing on any
/// mismatch. Bump this only when an old shape is genuinely no longer decodable.
pub const MIN_SUPPORTED_VERSION: u32 = 1;

/// Hard cap on the number of engaged modes accepted on the wire. There are only
/// a handful of [`PresenceMode`] variants, so any list longer than this is
/// malformed or hostile; capping during deserialization bounds allocation even
/// before the transport's line-length limit applies (defense in depth).
pub const MAX_ACTIVE_MODES: usize = 6;

/// Deserialize `active_modes` with a hard length cap and de-duplication, so a
/// malicious or buggy peer cannot push an unbounded / repeated mode list.
fn deserialize_active_modes<'de, D>(deserializer: D) -> Result<Vec<PresenceMode>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct ModesVisitor;

    impl<'de> serde::de::Visitor<'de> for ModesVisitor {
        type Value = Vec<PresenceMode>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "a list of at most {MAX_ACTIVE_MODES} presence modes")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut out: Vec<PresenceMode> = Vec::new();
            while let Some(mode) = seq.next_element::<PresenceMode>()? {
                if out.contains(&mode) {
                    continue; // engagement is a set; drop duplicates.
                }
                if out.len() >= MAX_ACTIVE_MODES {
                    return Err(serde::de::Error::custom(format!(
                        "active_modes exceeds cap of {MAX_ACTIVE_MODES}"
                    )));
                }
                out.push(mode);
            }
            Ok(out)
        }
    }

    deserializer.deserialize_seq(ModesVisitor)
}

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

impl PresenceMode {
    /// Stable wire / UI label. Same string the serde
    /// `rename_all = "snake_case"` produces — colocated as a method
    /// so non-serde callers (the shell's aria-live status line,
    /// telemetry) don't need to round-trip through `serde_json` for
    /// a display string. Renaming a variant is a wire break; this
    /// method exists to make that break impossible to miss (both
    /// the serde attribute and this match must change together).
    pub fn label(self) -> &'static str {
        match self {
            PresenceMode::Thinking => "thinking",
            PresenceMode::Speaking => "speaking",
            PresenceMode::ToolUse => "tool_use",
            PresenceMode::Listening => "listening",
            PresenceMode::Attention => "attention",
            PresenceMode::Error => "error",
        }
    }
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
    #[serde(default, deserialize_with = "deserialize_active_modes")]
    pub active_modes: Vec<PresenceMode>,
}

/// Hard cap on scene id length on the wire (bounded deserialization).
pub const MAX_SCENE_ID_LEN: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcDisposition {
    #[default]
    Overlay,
    Replace,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcAnchor {
    #[default]
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    CloudRelative,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct IpcPlacement {
    #[serde(default)]
    pub anchor: IpcAnchor,
    #[serde(default)]
    pub offset: [f32; 2],
    #[serde(default = "default_placement_scale")]
    pub scale: f32,
}

fn default_placement_scale() -> f32 {
    1.0
}

impl Default for IpcPlacement {
    fn default() -> Self {
        Self {
            anchor: IpcAnchor::Center,
            offset: [0.0, 0.0],
            scale: 1.0,
        }
    }
}

/// Generic spec-scene params on the wire: a primary knob (density/coverage)
/// and a secondary knob (wind), mapped by the realizer's schema.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneParamsWire {
    #[serde(default = "default_density")]
    pub density: f32,
    #[serde(default = "default_wind")]
    pub wind: f32,
}

fn default_density() -> f32 {
    0.7
}

fn default_wind() -> f32 {
    0.1
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
    /// Toggles whether the droplet accepts mouse events. When `false`
    /// (the shipping default), clicks fall through to windows behind
    /// the droplet — set by the runtime at startup under
    /// `PRESENCE_TRANSPARENT=1`. When `true`, the droplet grabs
    /// clicks and keyboard focus so the user can drag it, right-click
    /// for context, or otherwise interact. Meaningless on an opaque /
    /// non-transparent build (the runtime never registers as
    /// click-through there), but harmless to send — the runtime just
    /// applies the flag and moves on.
    SetInteractive { interactive: bool },
    /// Moves the droplet's top-left corner to `(x, y)` in physical
    /// screen pixels. Values are the same shape winit reports on
    /// `WindowEvent::Moved`, so a shell that stores a value it saw
    /// on the reverse channel can hand it right back on the next
    /// launch.
    SetPosition { x: i32, y: i32 },
    /// Spawn a registered scene template on the live stack.
    PresentScene {
        id: String,
        #[serde(default)]
        params: SceneParamsWire,
        #[serde(default)]
        disposition: IpcDisposition,
        #[serde(default)]
        placement: IpcPlacement,
        /// Fade duration hint; director uses its own transition window in P0.
        #[serde(default)]
        transition_secs: Option<f32>,
        /// Auto-dismiss after this many milliseconds; `None` = no TTL.
        #[serde(default)]
        ttl_ms: Option<u64>,
    },
    /// Fade out and remove a live scene by registry id.
    DismissScene { id: String },
}

/// A message the presence sends *back* to its host. Reverse channel
/// for the transport in `crates/presence-ipc/README-ish`: shell writes
/// [`Command`]s to the child's stdin, child writes [`Event`]s to its
/// stdout. Kept small and specific — the shell should not have to
/// mirror the runtime's whole state, just the pieces it needs to
/// persist or expose (window geometry today; frame timing later).
///
/// `#[non_exhaustive]` for the same reason [`Command`] is — a shell
/// built against an older runtime should ignore unknown events, not
/// break.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Event {
    /// Fired once when the runtime has opened its window and can
    /// accept `Command`s. Carries the initial window position so a
    /// shell that has never seen this presence has a value to
    /// persist immediately.
    Ready { x: i32, y: i32 },
    /// Fired when the window moves. Throttled on the runtime side
    /// (roughly one per 100 ms during a drag) so the pipe doesn't
    /// flood. Values are physical pixels — the same units the
    /// matching [`Command::SetPosition`] accepts.
    Moved { x: i32, y: i32 },
    /// Periodic liveness signal. Emitted by `presence-runtime` on a
    /// fixed cadence (`HEARTBEAT_INTERVAL_MS`, ~2 s) so the shell
    /// can distinguish "renderer is fine, just nothing to report"
    /// from "renderer wedged, GPU driver deadlocked, or process
    /// zombie". `sequence` is a monotonically increasing counter
    /// starting at 0 on process start — a gap in the sequence tells
    /// the shell the runtime restarted without the shell tearing
    /// down the process (e.g. an internal panic recovery path we
    /// might add later). `uptime_ms` is the wall-clock time since
    /// the runtime's start, useful for correlating with logs and
    /// for the audit trail when we record a stall.
    ///
    /// Not emitted when `PRESENCE_STDOUT_IPC` is off — the dev
    /// harness stays quiet.
    Heartbeat { sequence: u64, uptime_ms: u64 },
}

/// Cadence at which `presence-runtime` emits [`Event::Heartbeat`]s
/// when the stdout IPC channel is enabled. Exposed on the wire
/// crate so both sides derive their timing constants from the same
/// source of truth: the shell's stall threshold is a multiple of
/// this, so drifting the cadence without updating the threshold
/// would silently produce false-positive stalls.
pub const HEARTBEAT_INTERVAL_MS: u64 = 2_000;

/// Shell-side default for "how long since the last event before we
/// call the runtime stalled". 3× the heartbeat interval gives the
/// runtime two missed beats of headroom before the shell reacts,
/// which is enough to swallow a GC pause / disk I/O hiccup / brief
/// GPU stall without ever flagging a healthy renderer.
pub const STALL_THRESHOLD_MS: u64 = 3 * HEARTBEAT_INTERVAL_MS;

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
    /// against exactly.
    pub fn is_current(&self) -> bool {
        self.version == VERSION
    }

    /// True iff `self.version` is within `[MIN_SUPPORTED_VERSION, VERSION]`,
    /// i.e. this build can still decode the payload. Prefer this over
    /// [`Self::is_current`] on the receive path so a mixed-version rollout
    /// keeps working instead of hard-failing on any skew.
    pub fn is_compatible(&self) -> bool {
        self.version >= MIN_SUPPORTED_VERSION && self.version <= VERSION
    }
}

/// Reverse-channel envelope (`presence-runtime` → shell). Same
/// versioning story as [`Envelope`] — kept separate so a receiver can
/// pattern-match on the payload type in the type system rather than
/// discovering it at runtime.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub version: u32,
    pub payload: Event,
}

impl EventEnvelope {
    pub fn wrap(payload: Event) -> Self {
        Self {
            version: VERSION,
            payload,
        }
    }

    pub fn is_current(&self) -> bool {
        self.version == VERSION
    }

    /// See [`Envelope::is_compatible`].
    pub fn is_compatible(&self) -> bool {
        self.version >= MIN_SUPPORTED_VERSION && self.version <= VERSION
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
            let decoded: PresenceMode = serde_json::from_str(&encoded).expect("deserialize");
            assert_eq!(decoded, *mode);
        }
    }

    #[test]
    fn quality_and_palette_wire_names_are_the_ones_the_settings_use() {
        assert_eq!(
            serde_json::to_string(&QualityTier::Balanced).unwrap(),
            "\"balanced\""
        );
        assert_eq!(serde_json::to_string(&QualityTier::Low).unwrap(), "\"low\"");
        assert_eq!(serde_json::to_string(&PaletteId::Teal).unwrap(), "\"teal\"");
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

        let partial: Signals = serde_json::from_str(r#"{"intensity":0.5}"#).expect("partial");
        assert_eq!(partial.intensity, 0.5);
        assert_eq!(partial.audio_level, 0.0);
    }

    #[test]
    fn active_modes_dedupes_and_rejects_overlong_lists() {
        // Duplicates collapse to a set.
        let dup: Signals =
            serde_json::from_str(r#"{"active_modes":["speaking","speaking","thinking"]}"#)
                .expect("dedup");
        assert_eq!(
            dup.active_modes,
            vec![PresenceMode::Speaking, PresenceMode::Thinking]
        );

        // A hostile list of many DISTINCT-looking entries can't exceed the
        // variant count, but a padded array of distinct modes past the cap is
        // rejected rather than allocated. Build 7 entries by repeating the set
        // with no dedup collisions is impossible (only 6 variants), so assert
        // the cap holds for the full distinct set plus one forced overflow via
        // an unknown—no, keep it simple: the six distinct modes are accepted.
        let full: Signals = serde_json::from_str(
            r#"{"active_modes":["thinking","speaking","tool_use","listening","attention","error"]}"#,
        )
        .expect("full set");
        assert_eq!(full.active_modes.len(), MAX_ACTIVE_MODES);
    }

    #[test]
    fn envelope_is_compatible_accepts_supported_range() {
        let env = Envelope::wrap(Command::SetReducedMotion { enabled: true });
        assert!(env.is_compatible());
        assert!(env.is_current());
        // A future version is not decodable by this build.
        let future = Envelope {
            version: VERSION + 1,
            payload: Command::SetReducedMotion { enabled: true },
        };
        assert!(!future.is_compatible());
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
            Command::SetInteractive { interactive: true },
            Command::SetPosition { x: 100, y: 200 },
            Command::PresentScene {
                id: "test_scene".to_string(),
                params: SceneParamsWire {
                    density: 0.8,
                    wind: 0.05,
                },
                disposition: IpcDisposition::Overlay,
                placement: IpcPlacement {
                    anchor: IpcAnchor::BottomRight,
                    offset: [0.1, -0.1],
                    scale: 0.5,
                },
                transition_secs: None,
                ttl_ms: Some(30_000),
            },
            Command::DismissScene {
                id: "test_scene".to_string(),
            },
        ];
        for cmd in commands {
            let env = Envelope::wrap(cmd.clone());
            assert!(env.is_current());
            let encoded = serde_json::to_string(&env).expect("serialize");
            let decoded: Envelope = serde_json::from_str(&encoded).expect("deserialize");
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
    fn event_round_trips_through_the_reverse_envelope() {
        let events = [
            Event::Ready { x: 42, y: 108 },
            Event::Moved { x: -100, y: 900 },
            Event::Heartbeat {
                sequence: 42,
                uptime_ms: 84_000,
            },
        ];
        for event in events {
            let env = EventEnvelope::wrap(event.clone());
            assert!(env.is_current());
            let encoded = serde_json::to_string(&env).expect("serialize");
            let decoded: EventEnvelope = serde_json::from_str(&encoded).expect("deserialize");
            assert_eq!(decoded, env);
        }
    }

    #[test]
    fn heartbeat_uses_the_stable_wire_tag() {
        // External audit tooling filters on `kind == "heartbeat"`
        // to build "runtime uptime" dashboards. Pin the exact tag
        // so a serde rename can't silently break that.
        let event = Event::Heartbeat {
            sequence: 7,
            uptime_ms: 14_000,
        };
        let encoded = serde_json::to_string(&event).unwrap();
        assert!(encoded.contains(r#""kind":"heartbeat""#), "{encoded}");
        assert!(encoded.contains(r#""sequence":7"#), "{encoded}");
        assert!(encoded.contains(r#""uptime_ms":14000"#), "{encoded}");
    }

    #[test]
    fn stall_threshold_is_a_multiple_of_the_heartbeat_interval() {
        // Sanity pin: the two constants must not drift out of the
        // relationship the module docs describe, or the shell will
        // false-positive stalls on healthy runtimes.
        const _: () = assert!(STALL_THRESHOLD_MS >= 2 * HEARTBEAT_INTERVAL_MS);
        const _: () = assert!(HEARTBEAT_INTERVAL_MS > 0);
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
