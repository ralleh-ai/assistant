//! Speech and cursor as physics — the bounded, bandwidth-aware mappings from
//! two live signals onto shell motion (ADR-014 M7).
//!
//! # The spring-bandwidth split (ADR-012)
//!
//! The surface spring passes only slow geometry (~0.7 Hz corner). So speech is
//! split by frequency, not routed whole into one channel:
//!
//! - **Expansion** is geometry, and must ride the *phrase* envelope
//!   (`EntityParams::audio_envelope`, ~0.35 Hz) — a slow swell of the shell
//!   while a phrase is spoken. Driving it from the raw level would push a 4–7 Hz
//!   syllable rate into a spring that attenuates it to nothing.
//! - **Brightness** is not sprung, so the fast syllable-rate level goes there —
//!   the shimmer that makes the shell read as *articulating* rather than merely
//!   inflating.
//!
//! Both are gated by the speaking weight so silence leaves the shell alone.
//!
//! # Cursor look-at
//!
//! Cursor awareness arrives as a bounded screen-space bias (`cursor_dir`) and a
//! `cursor_proximity`. It becomes a small lean of the whole presence toward the
//! pointer and an attention bias. The lean is a *translation*, not a surface
//! deformation, so it never fights the shape's spring; the caller eases it over
//! time for the same reason.

use glam::{Vec2, Vec3};

/// Largest fractional scale swell speech may add to the shell, at full phrase
/// envelope. Small on purpose — the shell breathes with speech, it does not
/// balloon.
pub const MAX_VOICE_EXPANSION: f32 = 0.06;

/// Largest lean, in the entity's local units at unit scale, at full proximity
/// and a fully off-centre cursor.
pub const MAX_CURSOR_LEAN: f32 = 0.12;

/// How speech amplitude expresses on the shell this frame, split by what the
/// surface spring can carry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioResponse {
    /// Fractional scale swell (geometry) — apply as `scale *= 1 + expansion`.
    /// Rides the slow phrase envelope, so it is spring-safe.
    pub expansion: f32,
    /// Instant brightness lift (never sprung) — the fast syllable channel.
    pub brightness: f32,
}

/// Maps the speaking weight and the two audio channels onto the bandwidth-split
/// response. `speaking` gates both; `audio_envelope` is the slow phrase channel
/// that drives geometry; `audio_level` is the fast syllable channel that drives
/// brightness. All inputs are clamped, so a misbehaving sender can only fail to
/// drive the shell, never overdrive it.
pub fn audio_response(speaking: f32, audio_envelope: f32, audio_level: f32) -> AudioResponse {
    let gate = speaking.clamp(0.0, 1.0);
    let expansion = gate * audio_envelope.clamp(0.0, 1.0) * MAX_VOICE_EXPANSION;
    let brightness = gate * audio_level.clamp(0.0, 1.0);
    AudioResponse {
        expansion,
        brightness,
    }
}

/// How the cursor pulls on the presence this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorAim {
    /// A bounded lean of the whole entity toward the cursor, in local units at
    /// unit scale. A translation — the caller scales it by the entity scale and
    /// eases it, and it never touches the surface shape.
    pub lean: Vec3,
    /// Attention bias in `0..1` — how strongly the cursor's nearness should
    /// raise the presence's notice-me response. The shell owner (the Brain)
    /// decides what to do with it; the engine only offers it.
    pub attention: f32,
}

/// Maps the screen-space cursor bias and proximity onto a lean + attention
/// bias. `cursor_dir` is `[x right, y down]`, each `[-1, 1]`; screen-down is
/// flipped to world-up for the lean. The lean scales with proximity so a
/// far-away cursor barely tugs and one on top of the droplet pulls fully.
pub fn cursor_aim(cursor_dir: [f32; 2], proximity: f32) -> CursorAim {
    let prox = proximity.clamp(0.0, 1.0);
    let dir = Vec2::new(
        cursor_dir[0].clamp(-1.0, 1.0),
        cursor_dir[1].clamp(-1.0, 1.0),
    );
    // Screen y is down; the shell leans *up* toward a cursor above it.
    let lean = Vec3::new(dir.x, -dir.y, 0.0) * (MAX_CURSOR_LEAN * prox);
    CursorAim {
        lean,
        attention: prox,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_leaves_the_shell_alone() {
        let r = audio_response(0.0, 1.0, 1.0);
        assert_eq!(r.expansion, 0.0, "speech gated off but still expanded");
        assert_eq!(r.brightness, 0.0, "speech gated off but still brightened");
    }

    #[test]
    fn expansion_follows_the_slow_envelope_brightness_the_fast_level() {
        // Loud syllable, but mid-phrase envelope: brightness leads geometry.
        let r = audio_response(1.0, 0.3, 1.0);
        assert!(
            r.brightness > r.expansion,
            "fast channel did not lead geometry"
        );
        // Expansion tracks the envelope specifically.
        let quiet_phrase = audio_response(1.0, 0.0, 1.0);
        assert_eq!(
            quiet_phrase.expansion, 0.0,
            "expansion ignored the envelope"
        );
    }

    #[test]
    fn expansion_is_bounded() {
        let r = audio_response(2.0, 2.0, 2.0); // hostile, unclamped inputs
        assert!(r.expansion <= MAX_VOICE_EXPANSION + 1e-6);
        assert!(r.brightness <= 1.0 + 1e-6);
    }

    #[test]
    fn cursor_leans_toward_the_pointer_and_scales_with_proximity() {
        // Cursor to the right and below the droplet, right on top of it.
        let near = cursor_aim([1.0, 1.0], 1.0);
        assert!(near.lean.x > 0.0, "did not lean right toward the cursor");
        assert!(near.lean.y < 0.0, "screen-down cursor should lean world-up");
        assert_eq!(near.attention, 1.0);

        // Same direction, far away: a much smaller tug.
        let far = cursor_aim([1.0, 1.0], 0.1);
        assert!(
            far.lean.length() < near.lean.length() * 0.5,
            "proximity did not scale the lean"
        );
    }

    #[test]
    fn cursor_lean_is_bounded() {
        let aim = cursor_aim([9.0, -9.0], 5.0); // unclamped, hostile
        assert!(aim.lean.length() <= MAX_CURSOR_LEAN * 2.0_f32.sqrt() + 1e-6);
        assert!(aim.attention <= 1.0 + 1e-6);
    }

    #[test]
    fn a_centred_cursor_does_not_lean() {
        let aim = cursor_aim([0.0, 0.0], 1.0);
        assert_eq!(aim.lean, Vec3::ZERO);
    }
}
