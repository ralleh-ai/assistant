//! Declarative scene grammar — `PRESENCE_ADAPTIVE_SCENES` §3.1.
//!
//! A scene is *data*: a [`SceneSpec`] naming a base and a bounded list of
//! allow-listed [`SceneTerm`] primitives. One generic realizer
//! (`crate::scene::realize`) interprets any spec into an `EntityInstance`, so
//! new scenes are new data — never a new `.rs` file. The term set is the safety
//! boundary and the consistency standard: it grows only by review, never from a
//! model or from user input.

use crate::scene::params::{ParamSchema, SceneParams};

/// Upper bound on terms in a single spec (`PRESENCE_ADAPTIVE_SCENES` §6:
/// "bounded everything"). Keeps realize cost and the validator surface finite.
pub const MAX_TERMS: usize = 6;

/// The declarative, engine-realizable description of a scene.
#[derive(Clone, Copy, Debug)]
pub struct SceneSpec {
    pub base: SceneBase,
    pub terms: &'static [SceneTerm],
    pub motion: MotionProfile,
    pub palette_role: PaletteRole,
    /// Fraction `0..1` of surface-eligible points that are born from the shell
    /// skin snapshot (`crate::scene::surface_seed`) rather than procedurally.
    /// Only terms that participate in surface seeding (mist, filament) read it;
    /// pure emitters (cloud, rain) ignore it.
    pub surface_affinity: f32,
}

impl SceneSpec {
    /// Total budget weight across terms, used to split the global point budget
    /// (`crate::scene::realize`). Never zero, so an empty spec still allocates.
    pub fn total_weight(&self) -> f32 {
        let sum: f32 = self.terms.iter().map(|t| t.weight()).sum();
        sum.max(f32::EPSILON)
    }

    /// Resolve a spec's authored term defaults against runtime `SceneParams`
    /// (clamped by `schema`), yielding owned terms the realizer can hold.
    pub fn resolved_terms(&self, params: &SceneParams, schema: &ParamSchema) -> Vec<SceneTerm> {
        self.terms
            .iter()
            .map(|t| t.overridden(params, schema))
            .collect()
    }
}

/// The point substrate a spec is realized onto. `Emitter` scenes are pure term
/// populations (each term owns its particles). `Surface` is reserved for the
/// shell/plate shapes that terms *modulate*; those remain builtin templates
/// for now (see `templates::builtins`), so this enum is deliberately small and
/// grows when surface-modulator terms land.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SceneBase {
    #[default]
    Emitter,
}

/// One allow-listed primitive. Each carries typed, range-bounded params
/// (mirrors `ShellDrive`'s per-term weights). The realizer maps a term to a
/// pure generator/behavior in `crate::sim::terms`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneTerm {
    /// A soft, drifting cloud mass (diffuse `Halo` points).
    CloudBand { coverage: f32, wind: f32 },
    /// Falling rain streaks (sharp `Core` trails).
    Rain { density: f32, wind: f32 },
    /// Soft mist born from the shell skin, clinging to folds and slowly rising
    /// off the surface (diffuse `Halo` points; surface-seeded).
    SurfaceMist { coverage: f32, rise: f32 },
    /// Bright filaments born from high-crease shell spots, lifting slightly off
    /// the skin and reabsorbing (sharp `Core` points; surface-seeded).
    CreaseFilament { density: f32, lift: f32 },
}

impl SceneTerm {
    /// Share of the scene's point budget this term wants, relative to the
    /// other terms in the same spec.
    pub fn weight(&self) -> f32 {
        match self {
            SceneTerm::CloudBand { coverage, .. } => 0.30 + 0.30 * coverage.clamp(0.0, 1.0),
            SceneTerm::Rain { density, .. } => 0.30 + 0.40 * density.clamp(0.0, 1.0),
            SceneTerm::SurfaceMist { coverage, .. } => 0.30 + 0.40 * coverage.clamp(0.0, 1.0),
            // Filaments are sparse by nature: a little goes a long way, so they
            // take a smaller slice than the diffuse mist they accompany.
            SceneTerm::CreaseFilament { density, .. } => 0.15 + 0.25 * density.clamp(0.0, 1.0),
        }
    }

    /// Override authored defaults with the two generic knobs a template's
    /// `ParamSchema` exposes: index 0 = primary (density/coverage), index 1 =
    /// secondary (wind). A knob is applied only when the schema declares it, so
    /// a spec with an empty schema keeps its authored values.
    pub fn overridden(self, params: &SceneParams, schema: &ParamSchema) -> SceneTerm {
        let n = schema.defs.len();
        let p0 = (n >= 1).then(|| params.get(0));
        let p1 = (n >= 2).then(|| params.get(1));
        match self {
            SceneTerm::CloudBand { coverage, wind } => SceneTerm::CloudBand {
                coverage: p0.unwrap_or(coverage),
                wind: p1.unwrap_or(wind),
            },
            SceneTerm::Rain { density, wind } => SceneTerm::Rain {
                density: p0.unwrap_or(density),
                wind: p1.unwrap_or(wind),
            },
            SceneTerm::SurfaceMist { coverage, rise } => SceneTerm::SurfaceMist {
                coverage: p0.unwrap_or(coverage),
                rise: p1.unwrap_or(rise),
            },
            SceneTerm::CreaseFilament { density, lift } => SceneTerm::CreaseFilament {
                density: p0.unwrap_or(density),
                lift: p1.unwrap_or(lift),
            },
        }
    }
}

/// Time-domain shaping applied to every term in the spec.
#[derive(Clone, Copy, Debug)]
pub struct MotionProfile {
    /// Baseline animation rate multiplier (`1.0` = authored speed). Reduced
    /// motion multiplies this down at the director.
    pub time_scale: f32,
}

impl Default for MotionProfile {
    fn default() -> Self {
        Self { time_scale: 1.0 }
    }
}

/// Which end of the palette a scene leans on. Maps to a baseline `color_bias`
/// (0 = warm/calm tint, 1 = cool/active tint); the actual hues are the user's
/// active `PaletteId`, never chosen here (ADR-011).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaletteRole {
    #[default]
    Neutral,
    Cool,
    Warm,
    Accent,
}

impl PaletteRole {
    pub fn base_color_bias(self) -> f32 {
        match self {
            PaletteRole::Neutral => 0.45,
            PaletteRole::Cool => 0.85,
            PaletteRole::Warm => 0.12,
            PaletteRole::Accent => 0.6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_is_always_positive() {
        let spec = SceneSpec {
            base: SceneBase::Emitter,
            terms: &[],
            motion: MotionProfile::default(),
            palette_role: PaletteRole::Cool,
            surface_affinity: 0.0,
        };
        assert!(spec.total_weight() > 0.0);
    }

    #[test]
    fn overridden_respects_schema_arity() {
        let schema = ParamSchema {
            defs: &[crate::scene::params::ParamDef {
                name: "density",
                default: 0.5,
                min: 0.0,
                max: 1.0,
            }],
        };
        let mut params = SceneParams::default();
        params.set(0, 0.9);
        params.set(1, 0.42);
        // Only index 0 declared -> density overridden, wind kept.
        let term = SceneTerm::Rain {
            density: 0.5,
            wind: 0.1,
        }
        .overridden(&params, &schema);
        assert_eq!(
            term,
            SceneTerm::Rain {
                density: 0.9,
                wind: 0.1
            }
        );
    }
}
