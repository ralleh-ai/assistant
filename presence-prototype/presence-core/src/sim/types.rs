//! Core data shapes shared by generators, behaviors, and the renderer.
//!
//! Mirrors `docs/PRESENCE_SCENES.md` §7 / `docs/PRESENCE_VISUAL_ENTITY.md`
//! §7.3 — these are the literal Rust shapes described there, not a
//! translation from another language.

use glam::Vec3;

/// Which density/behavior gradient within an entity a point belongs to —
/// `docs/PRESENCE_VISUAL_ENTITY.md` §3.3. These are explicitly *not*
/// separate meshes or entities: one population carries a layer tag, and
/// generators, behaviors, and the point shader each read it to produce the
/// density, motion, and material gradient the spec describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    // --- Density gradient of a *surface* entity (§3.3). These three form a
    // continuous 0..2 ramp the point shader reads for its core→halo material.
    /// Denser, slower, more coherent. Carries the visual centre of mass.
    Core,
    /// The main volume.
    Body,
    /// Sparse outer points that expand and contract more freely.
    Halo,

    // --- Effect / free-space material classes (ADR-014 M6). These are *not*
    // part of the density ramp: each is its own material a template can mix in
    // (a nebula with sparks, a shell with an aura). Encoded past the ramp
    // (`as_f32` ≥ 3) so the shader can branch on them without disturbing the
    // core→halo interpolation. Surface generators never emit these.
    /// A large, faint outer glow — atmosphere around an entity.
    Aura,
    /// Bright, tight points — the "charged" look for high activity.
    Energy,
    /// Tiny, very bright motes — transient emphasis.
    Sparks,
    /// Soft medium points meant to read as motion streaks.
    Trails,
}

/// How a point of a given [`Layer`] is sized and lit relative to the base,
/// independent of the density ramp. Lets a template mix effect layers without
/// the shader needing a per-entity uniform for each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerMaterial {
    /// Point size multiplier.
    pub size: f32,
    /// Brightness multiplier.
    pub brightness: f32,
}

impl Layer {
    /// Encoded for the instance buffer. Kept as a float rather than a `u32`
    /// so the shader can also use it to interpolate material properties. The
    /// density layers occupy `0..2` (a ramp); the effect layers take discrete
    /// values from `3` up.
    pub fn as_f32(self) -> f32 {
        match self {
            Layer::Core => 0.0,
            Layer::Body => 1.0,
            Layer::Halo => 2.0,
            Layer::Aura => 3.0,
            Layer::Energy => 4.0,
            Layer::Sparks => 5.0,
            Layer::Trails => 6.0,
        }
    }

    /// Multiplier on how strongly a layer follows its spring anchor. The
    /// core is stiff and coherent; the halo is loose and free. The effect
    /// layers are free-space (field-driven, not spring-anchored), so their
    /// values are only a sensible fallback for any surface use.
    pub fn spring_scale(self) -> f32 {
        match self {
            Layer::Core => 1.35,
            Layer::Body => 1.0,
            Layer::Halo => 0.55,
            Layer::Aura => 0.4,
            Layer::Energy => 0.85,
            Layer::Sparks => 0.3,
            Layer::Trails => 0.5,
        }
    }

    /// Size / brightness multipliers for this layer's material — the
    /// "independent params" the effect layers carry. A generator applies these
    /// at birth so the point already reads as its class before the shader's own
    /// per-class falloff.
    pub fn material(self) -> LayerMaterial {
        match self {
            Layer::Core => LayerMaterial {
                size: 0.6,
                brightness: 1.0,
            },
            Layer::Body => LayerMaterial {
                size: 0.8,
                brightness: 0.85,
            },
            Layer::Halo => LayerMaterial {
                size: 1.0,
                brightness: 0.6,
            },
            Layer::Aura => LayerMaterial {
                size: 1.6,
                brightness: 0.35,
            },
            Layer::Energy => LayerMaterial {
                size: 0.7,
                brightness: 1.4,
            },
            Layer::Sparks => LayerMaterial {
                size: 0.35,
                brightness: 1.8,
            },
            Layer::Trails => LayerMaterial {
                size: 0.9,
                brightness: 0.7,
            },
        }
    }

    /// True for the density-gradient layers that surface entities seed and the
    /// point shader ramps between; false for the effect classes.
    pub fn is_surface(self) -> bool {
        matches!(self, Layer::Core | Layer::Body | Layer::Halo)
    }
}

/// A single rendered point. Kept small and `Copy` — thousands of these are
/// touched every frame.
#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub position: Vec3,
    pub velocity: Vec3,
    /// The particle's fixed surface coordinate: a unit direction for shells, a
    /// disk coordinate for plates. This is its identity on the skin, so it is
    /// set once at generation and never changes as the skin deforms — every
    /// other field below is derived from it.
    pub base_offset: Vec3,
    /// Outward surface normal at this point, for surface-based entities (see
    /// `crate::sim::shapes`). The point shader reads it for the grazing-angle
    /// silhouette that makes a scanned skin read as solid. Volume-based
    /// behaviors leave it at zero, which the shader treats as "no silhouette
    /// term" rather than as a degenerate normal.
    pub normal: Vec3,
    /// `0..1` fold-crease intensity. Drives the bright filaments that make
    /// surface structure legible.
    pub crease: f32,
    /// Cached surface point in the shape's own local space, plus how far off
    /// the skin this particle sits. Surface entities refresh these on a
    /// stagger rather than every step — see `crate::sim::shapes` for why that
    /// is what makes tens of thousands of points affordable.
    pub local: Vec3,
    pub shell_offset: f32,
    /// Which density gradient this point belongs to (§3.3).
    pub layer: Layer,
    pub size: f32,
    pub brightness: f32,
    /// 0.0 = warm/idle palette, 1.0 = cool/active palette (see
    /// `crate::palette`). A continuous value, not a discrete switch, so it
    /// can be lerped during transitions.
    pub color_bias: f32,
}

impl Default for Particle {
    /// An unlit point at the origin carrying no surface data.
    ///
    /// Exists for struct-update syntax (`..Default::default()`), so a generator
    /// names only the fields its model actually has. The surface cache fields in
    /// particular are meaningless to volume-based generators, and making them
    /// spell out zeroes for those turns every future field into a mechanical
    /// edit across generators that do not care about it.
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            base_offset: Vec3::ZERO,
            normal: Vec3::ZERO,
            crease: 0.0,
            local: Vec3::ZERO,
            shell_offset: 0.0,
            layer: Layer::Body,
            size: 1.0,
            brightness: 0.0,
            color_bias: 0.0,
        }
    }
}

/// Continuous signals that drive scenes, independent of which entity/mode
/// is active. Matches `docs/PRESENCE_SCENES.md` §7. Deliberately contains
/// only derived scalars — never raw audio/transcript content (see
/// `docs/PRESENCE_VISUAL_ENTITY.md` §5.2).
#[derive(Clone, Copy, Debug)]
pub struct PresenceSignals {
    pub intensity: f32,
    pub audio_level: f32,
    pub progress: f32,
}

impl Default for PresenceSignals {
    fn default() -> Self {
        Self {
            intensity: 0.15, // matches the `idle` default in PRESENCE_VISUAL_ENTITY.md §9
            audio_level: 0.0,
            progress: 0.0,
        }
    }
}

/// Per-term weights for the presence shell (`crate::sim::shapes::PresenceShell`).
///
/// A mode does not select a shape; it raises a weight. Two modes at once is
/// then two raised weights and needs no special case, and a transition is a
/// weight lerp rather than a cross-fade between two populations.
///
/// `fold` is the entity's resting identity and never reaches zero — a mode
/// that erased it would stop reading as the same living thing
/// (`docs/PRESENCE_VISUAL_ENTITY.md` §3.1). The other three start there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellDrive {
    /// Ridged fold displacement — the idle signature.
    pub fold: f32,
    /// Rising, reabsorbing bulges — `thinking`.
    pub lobes: f32,
    /// Travelling surface wave — `speaking`.
    pub pulse: f32,
    /// Extending pendants — `tool_use`.
    pub neck: f32,
}

impl ShellDrive {
    /// Below this a term is skipped entirely rather than evaluated and scaled
    /// to nothing. This gate is what keeps idle at the cost it has today
    /// however many terms the shell grows, and follows the precedent set by
    /// the curl gate in the volumetric behaviors.
    pub const GATE: f32 = 0.002;

    /// Resting weights: the fold shell and nothing else.
    pub const IDLE: Self = Self {
        fold: 1.0,
        lobes: 0.0,
        pulse: 0.0,
        neck: 0.0,
    };
}

impl Default for ShellDrive {
    fn default() -> Self {
        Self::IDLE
    }
}

/// Per-frame parameters passed into a generator/behavior. `EntityParams`
/// is where a `Scene`'s per-mode multipliers (intensity/swirl/expand/cool)
/// land after the `SceneDirector` resolves them for the current frame.
#[derive(Clone, Copy, Debug)]
pub struct EntityParams {
    pub time: f32,
    pub dt: f32,
    /// Fraction of real `dt` that advances `time`. `1.0` at rest; drops in
    /// reduced-motion mode so every time-based deformation on this entity
    /// (fold evolution, lobe migration, pulse travel, breathing, spin)
    /// slows together. Springs still integrate at real `dt`, because the
    /// point of a reduced-motion path is less *animation*, not laggier
    /// physics — a spring updated at 1/10th rate is a spring that visibly
    /// swims.
    pub time_scale: f32,
    pub center: Vec3,
    /// Overall scale of the entity's volume.
    pub scale: f32,
    pub intensity: f32,
    pub swirl: f32,
    pub expand: f32,
    pub cool: f32,
    /// Resolved `PresenceSignals::progress`. Signals are folded into these
    /// params by the behavior before shapes see them, so a shape reads one
    /// value per concept instead of having to remember to combine a per-mode
    /// multiplier with a live signal — a step that is easy to omit in one shape
    /// and not another.
    pub progress: f32,
    /// Speech loudness smoothed to a *phrase* envelope rather than a syllable
    /// one. Geometry has to be driven from this and not from the raw level:
    /// `SurfaceBehavior`'s spring sits near 0.7 Hz, so a 4-7 Hz syllable rate
    /// arrives at the skin attenuated to a couple of percent. Syllable-rate
    /// response goes to brightness instead, which is never sprung.
    pub audio_envelope: f32,
    /// Which shell terms are live this frame, and how strongly.
    pub drive: ShellDrive,
    /// `core_density_bias` from `docs/PRESENCE_VISUAL_ENTITY.md` §9. Pulls
    /// generated points inward so the entity has a genuinely denser core
    /// rather than a uniform spray. 0.0 = uniform, 1.0 = strongly centre-
    /// weighted.
    pub core_density_bias: f32,
    /// 0.0 (just spawned / fully dissolved) to 1.0 (fully present). Used by
    /// the transition system to fade entities in/out without popping.
    pub presence: f32,
    /// Cognitive focus, `0.0` diffuse .. `1.0` concentrated. Mirrors the
    /// Behavior Graph's `CognitiveState::focus`; the director copies it onto
    /// free-space entities each frame so their force fields (the SDF morph
    /// attractor, M5) can read it. Surface entities ignore it.
    pub focus: f32,
    /// Cognitive confidence, `0.0` unsure .. `0.5` neutral .. `1.0` certain.
    /// Same provenance as `focus`; a more confident presence morphs tighter.
    pub confidence: f32,
}

/// Deterministic `[0, 1)` hash of a spatial input, used to derive stable
/// per-particle/per-cluster phases and offsets without storing an RNG.
pub(crate) fn hash01(v: Vec3) -> f32 {
    let dot = v.dot(Vec3::new(12.9898, 78.233, 37.719));
    (dot.sin() * 43758.547).fract().abs()
}

impl EntityParams {
    pub fn new(center: Vec3, scale: f32) -> Self {
        Self {
            time: 0.0,
            dt: 0.0,
            time_scale: 1.0,
            center,
            scale,
            intensity: 0.0,
            swirl: 0.0,
            expand: 0.0,
            cool: 0.0,
            progress: 0.0,
            audio_envelope: 0.0,
            drive: ShellDrive::IDLE,
            core_density_bias: 0.5,
            presence: 1.0,
            // Neutral cognition — matches `behavior::CognitiveState::default`,
            // so an entity that never receives cognition morphs at its resting
            // coherence rather than snapping to or dissolving from a shape.
            focus: 0.0,
            confidence: 0.5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash01_is_deterministic_and_bounded() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        let a = hash01(v);
        let b = hash01(v);
        assert_eq!(a, b);
        assert!((0.0..1.0).contains(&a), "hash01 out of [0,1): {a}");
    }

    #[test]
    fn hash01_varies_across_inputs() {
        let a = hash01(Vec3::new(0.1, 0.2, 0.3));
        let b = hash01(Vec3::new(9.4, -3.2, 1.1));
        assert_ne!(a, b);
    }

    /// Every layer must encode to a distinct float — the shader branches on
    /// these, so a collision would render two classes identically.
    #[test]
    fn layer_encodings_are_distinct_and_ordered() {
        let layers = [
            Layer::Core,
            Layer::Body,
            Layer::Halo,
            Layer::Aura,
            Layer::Energy,
            Layer::Sparks,
            Layer::Trails,
        ];
        let codes: Vec<f32> = layers.iter().map(|l| l.as_f32()).collect();
        for (i, a) in codes.iter().enumerate() {
            for b in &codes[i + 1..] {
                assert_ne!(a, b, "two layers share an encoding");
            }
        }
        // Density layers occupy the 0..2 ramp; effect layers sit past it.
        assert!(Layer::Halo.as_f32() < Layer::Aura.as_f32());
    }

    #[test]
    fn only_the_density_layers_are_surface_layers() {
        assert!(Layer::Core.is_surface());
        assert!(Layer::Body.is_surface());
        assert!(Layer::Halo.is_surface());
        for effect in [Layer::Aura, Layer::Energy, Layer::Sparks, Layer::Trails] {
            assert!(
                !effect.is_surface(),
                "{effect:?} should not be a surface layer"
            );
        }
    }

    /// The effect classes carry their intended character: sparks are the
    /// smallest and brightest, aura the largest and faintest.
    #[test]
    fn effect_layer_materials_have_their_intended_character() {
        let halo = Layer::Halo.material();
        let sparks = Layer::Sparks.material();
        let aura = Layer::Aura.material();

        assert!(sparks.size < halo.size, "sparks should be tiny");
        assert!(
            sparks.brightness > halo.brightness,
            "sparks should be bright"
        );
        assert!(aura.size > halo.size, "aura should be large");
        assert!(aura.brightness < halo.brightness, "aura should be faint");
    }
}
