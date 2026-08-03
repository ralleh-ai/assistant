//! Scene registration — factory-driven templates (`PRESENCE_ADAPTIVE_SCENES` Phase 0).

use std::collections::HashMap;

use crate::scene::disposition::Disposition;
use crate::scene::entity::{EntityInstance, EntityKind};
use crate::scene::params::{ParamSchema, SceneParams};
use crate::scene::placement::Placement;
use crate::scene::quality::QualityTier;
use crate::scene::realize;
use crate::scene::spec::SceneSpec;
use crate::scene::surface_seed::SurfaceSeed;
use crate::scene::templates::builtins::{build_idle, build_loading, IDLE_ID, LOADING_ID};

pub type SceneId = &'static str;

pub type SceneBuildFn = fn(SceneParams, usize, QualityTier) -> EntityInstance;

/// How a template turns into a live entity: a hand-built surface factory
/// (idle/loading shells) or a data-defined `SceneSpec` realized generically.
#[derive(Clone, Copy, Debug)]
pub enum SceneSource {
    Builtin(SceneBuildFn),
    Spec(&'static SceneSpec),
}

#[derive(Clone, Copy, Debug)]
pub struct SceneTemplate {
    pub id: SceneId,
    pub label: &'static str,
    pub summary: &'static str,
    pub entity_kind: EntityKind,
    pub priority: u8,
    pub default_active: bool,
    pub param_schema: ParamSchema,
    pub default_disposition: Disposition,
    pub default_placement: Placement,
    pub base_scale: f32,
    pub source: SceneSource,
}

impl SceneTemplate {
    pub fn build(
        &self,
        params: SceneParams,
        budget: usize,
        tier: QualityTier,
        seeds: &[SurfaceSeed],
    ) -> EntityInstance {
        match self.source {
            SceneSource::Builtin(build_fn) => build_fn(params, budget, tier),
            SceneSource::Spec(spec) => {
                let mut params = params;
                params.clamp_to(&self.param_schema);
                let terms = spec.resolved_terms(&params, &self.param_schema);
                let mut entity = realize::realize(
                    spec,
                    terms,
                    budget,
                    tier,
                    self.priority,
                    self.base_scale,
                    seeds.to_vec(),
                );
                entity.scene_id = Some(self.id);
                entity.disposition = self.default_disposition;
                entity.placement = self.default_placement;
                // Spec scenes are ephemeral overlays; the director's
                // `present_scene` sets active/presence/placement/ttl when it
                // actually goes live.
                entity.active = false;
                entity.presence = 0.0;
                entity
            }
        }
    }
}

pub struct SceneRegistry {
    scenes: HashMap<SceneId, SceneTemplate>,
}

impl SceneRegistry {
    pub fn with_builtin_scenes() -> Self {
        let mut registry = Self {
            scenes: HashMap::new(),
        };
        registry.register(SceneTemplate {
            id: IDLE_ID,
            label: "Idle — Presence Shell",
            summary: "Always active. Folded surface with the mode terms \
                      resolving to zero; the shell any mode raises weights on.",
            entity_kind: EntityKind::AssistantCloud,
            priority: 0,
            default_active: true,
            param_schema: ParamSchema::empty(),
            default_disposition: Disposition::Overlay,
            default_placement: Placement::default(),
            base_scale: 1.32,
            source: SceneSource::Builtin(build_idle),
        });
        registry.register(SceneTemplate {
            id: LOADING_ID,
            label: "Loading — Chladni Plate",
            summary: "Secondary entity. Grains migrating onto the nodal \
                      lines of a driven square plate; toggled on/off.",
            entity_kind: EntityKind::LoadingRing,
            priority: 1,
            default_active: false,
            param_schema: ParamSchema::empty(),
            default_disposition: Disposition::Overlay,
            default_placement: Placement::default(),
            base_scale: 1.5,
            source: SceneSource::Builtin(build_loading),
        });
        // Data-defined scene *presets* (precipitation, fog, aura, aurora, …) are
        // intentionally not registered: only the idle shell and loading plate
        // ship for now. The spec/term realizer above stays wired so scenes can
        // still be selected/created at runtime — a preset is data, not code.
        // Tests register a single generic spec scene to exercise that path.
        #[cfg(test)]
        registry.register(test_scene_template());
        registry
    }

    pub fn register(&mut self, template: SceneTemplate) {
        self.scenes.insert(template.id, template);
    }

    pub fn get(&self, id: &str) -> Option<&SceneTemplate> {
        self.scenes.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = &SceneTemplate> {
        self.scenes.values()
    }

    pub fn default_active(&self) -> impl Iterator<Item = &SceneTemplate> {
        self.all().filter(|t| t.default_active)
    }

    pub fn len(&self) -> usize {
        self.scenes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scenes.is_empty()
    }
}

/// Alias for transitional readers.
pub type SceneDescriptor = SceneTemplate;

/// A single generic spec scene used only by tests to exercise the data-driven
/// realizer + director present/dismiss/budget/placement machinery. No preset
/// scenes ship in the running app; this keeps that path under test.
#[cfg(test)]
pub const TEST_SCENE_ID: &str = "test_scene";

#[cfg(test)]
const TEST_PARAM_SCHEMA: ParamSchema = ParamSchema {
    defs: &[
        crate::scene::params::ParamDef {
            name: "density",
            default: 0.7,
            min: 0.3,
            max: 1.0,
        },
        crate::scene::params::ParamDef {
            name: "wind",
            default: 0.1,
            min: -0.5,
            max: 0.5,
        },
    ],
};

#[cfg(test)]
const TEST_SCENE_SPEC: SceneSpec = SceneSpec {
    base: crate::scene::spec::SceneBase::Emitter,
    terms: &[crate::scene::spec::SceneTerm::CloudBand {
        coverage: 0.85,
        wind: 0.1,
    }],
    motion: crate::scene::spec::MotionProfile { time_scale: 1.0 },
    palette_role: crate::scene::spec::PaletteRole::Cool,
    surface_affinity: 0.0,
};

#[cfg(test)]
fn test_scene_template() -> SceneTemplate {
    SceneTemplate {
        id: TEST_SCENE_ID,
        label: "Test Scene",
        summary: "Generic spec scene for tests only.",
        entity_kind: EntityKind::Scene,
        priority: 2,
        default_active: false,
        param_schema: TEST_PARAM_SCHEMA,
        default_disposition: Disposition::Overlay,
        default_placement: Placement::default(),
        base_scale: 0.85,
        source: SceneSource::Spec(&TEST_SCENE_SPEC),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_expose_only_idle_and_loading() {
        let registry = SceneRegistry::with_builtin_scenes();
        // idle + loading + the cfg(test) generic scene.
        assert_eq!(registry.len(), 3);
        let idle = registry.get(IDLE_ID).expect("idle scene missing");
        assert_eq!(idle.entity_kind, EntityKind::AssistantCloud);
        assert!(idle.default_active);
        let loading = registry.get(LOADING_ID).expect("loading scene missing");
        assert_eq!(loading.entity_kind, EntityKind::LoadingRing);
        assert!(!loading.default_active);
        assert!(loading.priority > idle.priority);
        // No preset ambient scenes ship.
        assert!(registry.get("precipitation").is_none());
        assert!(registry.get("fog").is_none());
        assert!(registry.get("aura_mist").is_none());
        assert!(registry.get("aurora").is_none());
    }

    #[test]
    fn spec_template_builds_deterministically() {
        let registry = SceneRegistry::with_builtin_scenes();
        let template = registry.get(TEST_SCENE_ID).unwrap();
        let params = SceneParams::from_schema(&template.param_schema);
        let a = template.build(params, 400, QualityTier::Balanced, &[]);
        let b = template.build(params, 400, QualityTier::Balanced, &[]);
        assert_eq!(a.particles.len(), b.particles.len());
        assert_eq!(
            a.particles.first().map(|p| p.position),
            b.particles.first().map(|p| p.position)
        );
    }

    #[test]
    fn register_replaces_an_existing_template_by_id() {
        let mut registry = SceneRegistry::with_builtin_scenes();
        let before = registry.get(IDLE_ID).unwrap().label;
        registry.register(SceneTemplate {
            id: IDLE_ID,
            label: "Idle — reshaped",
            summary: "replacement",
            entity_kind: EntityKind::AssistantCloud,
            priority: 0,
            default_active: true,
            param_schema: ParamSchema::empty(),
            default_disposition: Disposition::Overlay,
            default_placement: Placement::default(),
            base_scale: 1.32,
            source: SceneSource::Builtin(build_idle),
        });
        let after = registry.get(IDLE_ID).unwrap().label;
        assert_ne!(before, after);
        assert_eq!(after, "Idle — reshaped");
    }
}
