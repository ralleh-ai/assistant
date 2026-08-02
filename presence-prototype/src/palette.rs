//! Presence colour, as runtime-selectable data.
//!
//! The brand hexes below are taken directly from `desktop-edge/src/App.css`'s
//! custom properties so the prototype and the shipped UI never drift into two
//! different "teal"s. See `docs/PRESENCE_VISUAL_ENTITY.md` §3.1.
//!
//! Colour is a **user setting**, not a compile-time constant: the presence is
//! the assistant's visual character, and which hue that character wears is the
//! operator's choice. `PresencePalette` is therefore a value threaded to the
//! renderer each frame rather than a set of constants read inside it, and
//! `PaletteId` is the serializable name that
//! `docs/PRESENCE_INTEGRATION_PLAN.md` records as landing in
//! `EdgeSettings.presence_palette` in Phase 2. Baking the palette in would
//! have to be undone to ship the setting at all.

/// `--ink` — the shell's near-black background. Shared by every palette: the
/// field is the window's background, not part of the entity's identity.
pub const INK_HEX: u32 = 0x0e_16_14;

/// A selectable presence colour scheme.
///
/// Kept as a small closed enum rather than free-form hex input so it can be
/// round-tripped through settings and validated the same way
/// `EdgeSettings.voice_style` is validated against a fixed list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteId {
    /// Brand-reconciled dark teal — the default, per §3.1's argument that a
    /// lime scanner inside a teal-branded product reads as an unrelated
    /// overlay rather than "the entity".
    Teal,
    /// The source concept's original LiDAR lime/yellow-green, kept as a
    /// first-class option because it is what the visual concept was designed
    /// around.
    Lime,
    /// Cool blue-white, for a colder more clinical read.
    Ice,
    /// Warm amber, reusing the shell's existing "needs attention" accent as a
    /// whole-entity scheme.
    Ember,
}

impl PaletteId {
    /// Every selectable palette, in the order a settings UI should list them.
    pub const ALL: [PaletteId; 4] = [
        PaletteId::Teal,
        PaletteId::Lime,
        PaletteId::Ice,
        PaletteId::Ember,
    ];

    /// Stable serialized name. This is what gets persisted, so these strings
    /// are part of the settings contract and must not be renamed casually.
    pub fn as_str(self) -> &'static str {
        match self {
            PaletteId::Teal => "teal",
            PaletteId::Lime => "lime",
            PaletteId::Ice => "ice",
            PaletteId::Ember => "ember",
        }
    }

    /// Parses a persisted name, falling back to the default for anything
    /// unrecognised. Colour is cosmetic, so an unknown value must never be an
    /// error that blocks startup — hence a fallback rather than a `Result`.
    pub fn from_str_or_default(name: &str) -> Self {
        PaletteId::ALL
            .into_iter()
            .find(|id| id.as_str() == name)
            .unwrap_or(PaletteId::Teal)
    }

    pub fn palette(self) -> PresencePalette {
        match self {
            // `--foam`, `--teal`, `--teal-deep`, `--mist`.
            PaletteId::Teal => {
                PresencePalette::from_hexes(self, 0xf3f7f5, 0x1f8a7a, 0x146257, 0xd7e4df)
            }
            // The concept art's lime body with a hot yellow-green accent.
            PaletteId::Lime => {
                PresencePalette::from_hexes(self, 0xf4ffd6, 0xb8e03a, 0x5e7d16, 0xe8ffa8)
            }
            PaletteId::Ice => {
                PresencePalette::from_hexes(self, 0xf2f8ff, 0x5ca8d8, 0x1e446b, 0xcfe6ff)
            }
            PaletteId::Ember => {
                PresencePalette::from_hexes(self, 0xfff4e0, 0xc4a574, 0x7a4a1e, 0xffe2b0)
            }
        }
    }
}

/// The colour stops the point shader interpolates between.
///
/// Every stop is stored as **pure chroma** (linear RGB rescaled so its largest
/// channel is 1.0). Points are emissive: their lightness comes from the
/// simulation's energy term, so a tint must carry hue only. Storing
/// `--teal-deep` as-is would dim every point using it by ~7x on top of
/// whatever brightness the behaviour assigned, making hue and brightness
/// impossible to tune independently.
#[derive(Clone, Copy, Debug)]
pub struct PresencePalette {
    pub id: PaletteId,
    /// Calm/idle end of the state axis — near-neutral, per §3.1's "`--foam` at
    /// low brightness".
    pub calm: [f32; 3],
    /// The scheme's signature hue. `calm` is pulled toward this by
    /// `PointMaterial::calm_undertone` to give §3.1's "faint undertone".
    pub body: [f32; 3],
    /// Heavy-compute end of the state axis — the deepest, coolest stop.
    pub cool: [f32; 3],
    /// Ceiling of the density axis. Deliberately not pure white: the last step
    /// to white is left to the composite's highlight desaturation so that white
    /// means "genuinely dense" rather than "assigned white".
    pub hot: [f32; 3],
    /// Fold/crease filaments. Brightest and most saturated stop — this is what
    /// draws the structure lines across the surface.
    pub accent: [f32; 3],
    /// Near-black field colour, added after tonemapping.
    pub ink: [f32; 3],
}

impl PresencePalette {
    fn from_hexes(id: PaletteId, calm: u32, body: u32, cool: u32, hot: u32) -> Self {
        Self {
            id,
            calm: hex_to_chroma(calm),
            body: hex_to_chroma(body),
            cool: hex_to_chroma(cool),
            hot: hex_to_chroma(hot),
            // The accent is the body hue driven to full chroma rather than a
            // sixth hand-picked hex. Creases are the same material catching
            // more light, not a different material, so deriving it guarantees
            // it stays in family for every palette including future ones.
            accent: saturate(hex_to_chroma(body), 0.55),
            ink: hex_to_linear(INK_HEX),
        }
    }

    /// The calm stop pulled `undertone` of the way toward the signature hue —
    /// §3.1's "`--foam` at low brightness, faint `--teal` undertone". At 0 the
    /// entity is neutral white and reads as unbranded; at 1 idle is fully
    /// saturated and becomes indistinguishable from the compute states.
    pub fn calm_tint(&self, undertone: f32) -> [f32; 3] {
        let t = undertone.clamp(0.0, 1.0);
        [
            self.calm[0] + (self.body[0] - self.calm[0]) * t,
            self.calm[1] + (self.body[1] - self.calm[1]) * t,
            self.calm[2] + (self.body[2] - self.calm[2]) * t,
        ]
    }
}

impl Default for PresencePalette {
    fn default() -> Self {
        PaletteId::Teal.palette()
    }
}

/// Pushes a chroma away from neutral, deepening the hue without changing which
/// channel dominates.
fn saturate(c: [f32; 3], amount: f32) -> [f32; 3] {
    let peak = c[0].max(c[1]).max(c[2]);
    let mut out = [0.0; 3];
    for i in 0..3 {
        // Pull each non-dominant channel toward zero; the dominant one is
        // already at `peak` and stays there, so the result is still normalized.
        out[i] = peak - (peak - c[i]) * (1.0 + amount);
        out[i] = out[i].max(0.0);
    }
    out
}

/// sRGB 0-255 hex to linear f32 RGB, since `wgpu` surfaces and shader math
/// both expect linear color, not the gamma-encoded values CSS hex codes are
/// written in.
pub fn hex_to_linear(hex: u32) -> [f32; 3] {
    let to_channel = |byte: u32| -> f32 {
        let c = (byte as f32) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    [
        to_channel((hex >> 16) & 0xff),
        to_channel((hex >> 8) & 0xff),
        to_channel(hex & 0xff),
    ]
}

/// A brand color reduced to pure chroma: linear RGB rescaled so its largest
/// channel is 1.0. See `PresencePalette`'s note on why tints carry hue only.
pub fn hex_to_chroma(hex: u32) -> [f32; 3] {
    let linear = hex_to_linear(hex);
    let peak = linear[0].max(linear[1]).max(linear[2]);
    if peak <= f32::EPSILON {
        return [0.0, 0.0, 0.0];
    }
    [linear[0] / peak, linear[1] / peak, linear[2] / peak]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_to_linear_endpoints() {
        assert_eq!(hex_to_linear(0x00_00_00), [0.0, 0.0, 0.0]);
        let white = hex_to_linear(0xff_ff_ff);
        for channel in white {
            assert!((channel - 1.0).abs() < 1e-5, "expected ~1.0, got {channel}");
        }
    }

    #[test]
    fn chroma_peaks_at_one_and_keeps_hue_order() {
        let deep = hex_to_chroma(0x14_62_57);
        let peak = deep[0].max(deep[1]).max(deep[2]);
        assert!((peak - 1.0).abs() < 1e-6, "expected peak 1.0, got {peak}");
        // Still a teal: green dominant, blue next, red minimal.
        assert!(deep[1] > deep[2] && deep[2] > deep[0]);
    }

    #[test]
    fn hex_to_linear_is_darker_than_srgb_value() {
        // Linear-light values for mid-gray sRGB should be well below the
        // naive 0-1 fraction, since sRGB gamma boosts midtones.
        let [r, _, _] = hex_to_linear(0x80_80_80);
        assert!(r < 0.8_f32.powi(2) && r > 0.0);
    }

    #[test]
    fn every_palette_is_normalized_and_has_a_distinct_name() {
        let mut names = Vec::new();
        for id in PaletteId::ALL {
            let p = id.palette();
            for (label, stop) in [
                ("calm", p.calm),
                ("body", p.body),
                ("cool", p.cool),
                ("hot", p.hot),
                ("accent", p.accent),
            ] {
                let peak = stop[0].max(stop[1]).max(stop[2]);
                assert!(
                    (peak - 1.0).abs() < 1e-5,
                    "{} {label} stop is not normalized: {stop:?}",
                    id.as_str()
                );
            }
            assert!(
                !names.contains(&id.as_str()),
                "duplicate name {}",
                id.as_str()
            );
            names.push(id.as_str());
        }
    }

    #[test]
    fn palette_names_round_trip_and_unknown_falls_back() {
        for id in PaletteId::ALL {
            assert_eq!(PaletteId::from_str_or_default(id.as_str()), id);
        }
        // Cosmetic setting: an unrecognised persisted value must degrade to
        // the default rather than fail.
        assert_eq!(
            PaletteId::from_str_or_default("chartreuse"),
            PaletteId::Teal
        );
        assert_eq!(PaletteId::from_str_or_default(""), PaletteId::Teal);
    }

    #[test]
    fn accent_is_more_saturated_than_the_body_hue() {
        let p = PaletteId::Teal.palette();
        // Same dominant channel, but the others pulled further down.
        assert!(p.accent[0] < p.body[0]);
        assert!(p.accent[2] < p.body[2]);
    }

    #[test]
    fn calm_tint_interpolates_between_neutral_and_body() {
        let p = PaletteId::Teal.palette();
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5);
        assert!(close(p.calm_tint(0.0), p.calm));
        assert!(close(p.calm_tint(1.0), p.body));
        let mid = p.calm_tint(0.5);
        assert!(mid[0] < p.calm[0] && mid[0] > p.body[0]);
    }
}
