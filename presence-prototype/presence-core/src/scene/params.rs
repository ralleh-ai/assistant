//! Template parameter schema + runtime bag for registry factories.

pub const MAX_SCENE_PARAMS: usize = 8;

/// Precipitation template param indices (Phase 0 rain).
pub const PARAM_DENSITY: usize = 0;
pub const PARAM_WIND: usize = 1;

#[derive(Clone, Copy, Debug)]
pub struct ParamDef {
    pub name: &'static str,
    pub default: f32,
    pub min: f32,
    pub max: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct ParamSchema {
    pub defs: &'static [ParamDef],
}

impl ParamSchema {
    pub fn empty() -> Self {
        Self { defs: &[] }
    }

    pub fn clamp_value(&self, index: usize, value: f32) -> f32 {
        let def = self.defs.get(index);
        match def {
            Some(d) => {
                if value.is_nan() {
                    d.default
                } else {
                    value.clamp(d.min, d.max)
                }
            }
            None => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SceneParams {
    pub values: [f32; MAX_SCENE_PARAMS],
}

impl SceneParams {
    pub fn from_schema(schema: &ParamSchema) -> Self {
        let mut values = [0.0; MAX_SCENE_PARAMS];
        for (i, def) in schema.defs.iter().enumerate() {
            if i < MAX_SCENE_PARAMS {
                values[i] = def.default;
            }
        }
        Self { values }
    }

    pub fn clamp_to(&mut self, schema: &ParamSchema) {
        for (i, _) in schema.defs.iter().enumerate() {
            if i < MAX_SCENE_PARAMS {
                self.values[i] = schema.clamp_value(i, self.values[i]);
            }
        }
    }

    pub fn get(&self, index: usize) -> f32 {
        self.values.get(index).copied().unwrap_or(0.0)
    }

    pub fn set(&mut self, index: usize, value: f32) {
        if index < MAX_SCENE_PARAMS {
            self.values[index] = value;
        }
    }
}
