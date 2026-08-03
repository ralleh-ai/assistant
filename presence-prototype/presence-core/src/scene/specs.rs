//! Built-in scene *data* — `PRESENCE_ADAPTIVE_SCENES` §3.1.
//!
//! Each scene here is a `SceneSpec` const composed from the primitive term
//! vocabulary (`crate::sim::terms`). Adding a scene means adding data here (and
//! registering it), never a new generator/behavior file. New *terms* are the
//! only code, and they are a small reviewed allowlist.

use crate::scene::params::{ParamDef, ParamSchema};
use crate::scene::spec::{MotionProfile, PaletteRole, SceneBase, SceneSpec, SceneTerm};

pub const PRECIPITATION_ID: &str = "precipitation";
pub const FOG_ID: &str = "fog";

/// Primary knob = density/coverage (index 0), secondary = wind (index 1).
/// Shared by the emitter scenes so the realizer's generic override mapping
/// (`SceneTerm::overridden`) applies uniformly.
pub const EMITTER_PARAM_SCHEMA: ParamSchema = ParamSchema {
    defs: &[
        ParamDef {
            name: "density",
            default: 0.7,
            min: 0.3,
            max: 1.0,
        },
        ParamDef {
            name: "wind",
            default: 0.1,
            min: -0.5,
            max: 0.5,
        },
    ],
};

/// Rain cloud: a drifting cloud band with rain falling beneath it.
pub const PRECIPITATION_SPEC: SceneSpec = SceneSpec {
    base: SceneBase::Emitter,
    terms: &[
        SceneTerm::CloudBand {
            coverage: 0.85,
            wind: 0.1,
        },
        SceneTerm::Rain {
            density: 0.7,
            wind: 0.1,
        },
    ],
    motion: MotionProfile { time_scale: 1.0 },
    palette_role: PaletteRole::Cool,
};

/// Fog: the same cloud-band primitive, alone — proves term reuse across scenes.
pub const FOG_SPEC: SceneSpec = SceneSpec {
    base: SceneBase::Emitter,
    terms: &[SceneTerm::CloudBand {
        coverage: 1.0,
        wind: 0.05,
    }],
    motion: MotionProfile { time_scale: 0.8 },
    palette_role: PaletteRole::Neutral,
};
