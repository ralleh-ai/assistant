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
    /// Denser, slower, more coherent. Carries the visual centre of mass.
    Core,
    /// The main volume.
    Body,
    /// Sparse outer points that expand and contract more freely.
    Halo,
}

impl Layer {
    /// Encoded for the instance buffer. Kept as a float rather than a `u32`
    /// so the shader can also use it to interpolate material properties.
    pub fn as_f32(self) -> f32 {
        match self {
            Layer::Core => 0.0,
            Layer::Body => 1.0,
            Layer::Halo => 2.0,
        }
    }

    /// Multiplier on how strongly a layer follows its spring anchor. The
    /// core is stiff and coherent; the halo is loose and free.
    pub fn spring_scale(self) -> f32 {
        match self {
            Layer::Core => 1.35,
            Layer::Body => 1.0,
            Layer::Halo => 0.55,
        }
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
}
