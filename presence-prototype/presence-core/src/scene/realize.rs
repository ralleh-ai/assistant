//! Generic spec realizer — `PRESENCE_ADAPTIVE_SCENES` Phase 2.5 T-2.5.2.
//!
//! One `PointGenerator`/`PointBehavior` pair interprets *any* [`SceneSpec`] by
//! splitting the point budget across its terms and dispatching each slice to
//! the matching primitive in `crate::sim::terms`. There is no per-scene code:
//! a scene is the data in its spec.
//!
//! Each generated particle is tagged with its term index in `velocity.x` (an
//! unused scratch field for emitter particles — it is never uploaded to the
//! GPU, see `render::InstanceRaw`). The behavior walks the contiguous per-term
//! runs by that tag, so generator and behavior always agree on which term owns
//! which particle even when a term produces fewer points than allocated (e.g.
//! the rain streak cap).

use crate::scene::entity::{EntityInstance, EntityKind};
use crate::scene::quality::QualityTier;
use crate::scene::spec::{PaletteRole, SceneBase, SceneSpec, SceneTerm};
use crate::scene::surface_seed::SurfaceSeed;
use crate::sim::terms::{self, cloud, filament, mist, rain, TermCtx};
use crate::sim::types::{EntityParams, Particle};
use crate::sim::{PointBehavior, PointGenerator, PresenceSignals};

fn term_ctx<'a>(
    params: &EntityParams,
    role: PaletteRole,
    seeds: &'a [SurfaceSeed],
    surface_affinity: f32,
) -> TermCtx<'a> {
    TermCtx {
        center: params.center,
        scale: params.scale,
        time: params.time,
        presence: params.presence,
        color_bias: role.base_color_bias(),
        seeds,
        surface_affinity,
    }
}

/// Runs each term's generator over its share of the budget.
pub struct SpecGenerator {
    terms: Vec<SceneTerm>,
    role: PaletteRole,
    #[allow(dead_code)]
    base: SceneBase,
    /// Shell-skin snapshot captured at present time; surface-native terms birth
    /// points from it. Owned so budget regeneration reuses the same snapshot.
    seeds: Vec<SurfaceSeed>,
    surface_affinity: f32,
}

impl SpecGenerator {
    pub fn new(
        terms: Vec<SceneTerm>,
        role: PaletteRole,
        base: SceneBase,
        seeds: Vec<SurfaceSeed>,
        surface_affinity: f32,
    ) -> Self {
        Self {
            terms,
            role,
            base,
            seeds,
            surface_affinity,
        }
    }
}

impl PointGenerator for SpecGenerator {
    fn generate(&self, count: usize, params: &EntityParams) -> Vec<Particle> {
        let ctx = term_ctx(params, self.role, &self.seeds, self.surface_affinity);
        let weights: Vec<f32> = self.terms.iter().map(|t| t.weight()).collect();
        let allocs = terms::split(count, &weights);

        let mut out: Vec<Particle> = Vec::with_capacity(count);
        for (idx, (term, alloc)) in self.terms.iter().zip(allocs).enumerate() {
            let start = out.len();
            match term {
                SceneTerm::CloudBand { coverage, .. } => {
                    cloud::generate(alloc, *coverage, &ctx, &mut out)
                }
                SceneTerm::Rain { density, .. } => rain::generate(alloc, *density, &ctx, &mut out),
                SceneTerm::SurfaceMist { coverage, rise } => {
                    mist::generate(alloc, *coverage, *rise, &ctx, &mut out)
                }
                SceneTerm::CreaseFilament { density, lift } => {
                    filament::generate(alloc, *density, *lift, &ctx, &mut out)
                }
            }
            for p in &mut out[start..] {
                p.velocity.x = idx as f32;
            }
        }
        out
    }
}

/// Updates each term's slice each frame.
pub struct SpecBehavior {
    terms: Vec<SceneTerm>,
    role: PaletteRole,
    seeds: Vec<SurfaceSeed>,
    surface_affinity: f32,
}

impl SpecBehavior {
    pub fn new(
        terms: Vec<SceneTerm>,
        role: PaletteRole,
        seeds: Vec<SurfaceSeed>,
        surface_affinity: f32,
    ) -> Self {
        Self {
            terms,
            role,
            seeds,
            surface_affinity,
        }
    }
}

impl PointBehavior for SpecBehavior {
    fn update(
        &mut self,
        particles: &mut [Particle],
        _dt: f32,
        params: &EntityParams,
        _signals: &PresenceSignals,
    ) {
        let ctx = term_ctx(params, self.role, &self.seeds, self.surface_affinity);
        let mut i = 0;
        while i < particles.len() {
            let tag = particles[i].velocity.x as usize;
            let mut j = i + 1;
            while j < particles.len() && particles[j].velocity.x as usize == tag {
                j += 1;
            }
            let run = &mut particles[i..j];
            match self.terms.get(tag) {
                Some(SceneTerm::CloudBand { wind, .. }) => cloud::update(run, *wind, &ctx),
                Some(SceneTerm::Rain { wind, .. }) => rain::update(run, *wind, &ctx),
                Some(SceneTerm::SurfaceMist { rise, .. }) => mist::update(run, *rise, &ctx),
                Some(SceneTerm::CreaseFilament { lift, .. }) => filament::update(run, *lift, &ctx),
                None => {}
            }
            i = j;
        }
    }
}

/// Build a live `EntityInstance` for a resolved spec. `scene_id`, disposition,
/// placement, and ttl are set by the caller (registry / director). `seeds` is
/// the shell-skin snapshot; empty when no shell was available.
pub fn realize(
    spec: &SceneSpec,
    terms: Vec<SceneTerm>,
    budget: usize,
    _tier: QualityTier,
    priority: u8,
    base_scale: f32,
    seeds: Vec<SurfaceSeed>,
) -> EntityInstance {
    let mut params = EntityParams::new(glam::Vec3::ZERO, base_scale);
    params.time_scale = spec.motion.time_scale;

    let generator = SpecGenerator::new(
        terms.clone(),
        spec.palette_role,
        spec.base,
        seeds.clone(),
        spec.surface_affinity,
    );
    let behavior = SpecBehavior::new(terms, spec.palette_role, seeds, spec.surface_affinity);

    EntityInstance::new(
        EntityKind::Scene,
        Box::new(generator),
        Box::new(behavior),
        budget,
        priority,
        params,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::spec::MotionProfile;

    fn precip_spec() -> SceneSpec {
        SceneSpec {
            base: SceneBase::Emitter,
            terms: &[
                SceneTerm::CloudBand {
                    coverage: 0.8,
                    wind: 0.1,
                },
                SceneTerm::Rain {
                    density: 0.7,
                    wind: 0.1,
                },
            ],
            motion: MotionProfile::default(),
            palette_role: PaletteRole::Cool,
            surface_affinity: 0.0,
        }
    }

    fn aura_spec() -> SceneSpec {
        SceneSpec {
            base: SceneBase::Emitter,
            terms: &[
                SceneTerm::SurfaceMist {
                    coverage: 0.8,
                    rise: 0.5,
                },
                SceneTerm::CreaseFilament {
                    density: 0.6,
                    lift: 0.4,
                },
            ],
            motion: MotionProfile::default(),
            palette_role: PaletteRole::Cool,
            surface_affinity: 1.0,
        }
    }

    #[test]
    fn realizer_is_deterministic() {
        let spec = precip_spec();
        let terms = spec.terms.to_vec();
        let gen = SpecGenerator::new(terms, spec.palette_role, spec.base, Vec::new(), 0.0);
        let params = EntityParams::new(glam::Vec3::ZERO, 0.8);
        let a = gen.generate(30_000, &params);
        let b = gen.generate(30_000, &params);
        assert_eq!(a.len(), b.len());
        for (pa, pb) in a.iter().zip(b.iter()) {
            assert_eq!(pa.position, pb.position);
            assert_eq!(pa.velocity.x, pb.velocity.x);
        }
    }

    #[test]
    fn both_terms_produce_tagged_runs() {
        let spec = precip_spec();
        let gen = SpecGenerator::new(
            spec.terms.to_vec(),
            spec.palette_role,
            spec.base,
            Vec::new(),
            0.0,
        );
        let params = EntityParams::new(glam::Vec3::ZERO, 0.8);
        let particles = gen.generate(30_000, &params);
        let tag0 = particles
            .iter()
            .filter(|p| p.velocity.x as usize == 0)
            .count();
        let tag1 = particles
            .iter()
            .filter(|p| p.velocity.x as usize == 1)
            .count();
        assert!(tag0 > 0, "cloud term produced nothing");
        assert!(tag1 > 0, "rain term produced nothing");
        assert_eq!(tag0 + tag1, particles.len());
    }

    #[test]
    fn behavior_updates_without_panicking_on_tagged_runs() {
        let spec = precip_spec();
        let gen = SpecGenerator::new(
            spec.terms.to_vec(),
            spec.palette_role,
            spec.base,
            Vec::new(),
            0.0,
        );
        let mut behavior =
            SpecBehavior::new(spec.terms.to_vec(), spec.palette_role, Vec::new(), 0.0);
        let mut params = EntityParams::new(glam::Vec3::new(0.0, 0.0, 0.0), 0.7);
        params.presence = 1.0;
        let mut particles = gen.generate(30_000, &params);
        for _ in 0..30 {
            params.time += 1.0 / 60.0;
            behavior.update(
                &mut particles,
                1.0 / 60.0,
                &params,
                &PresenceSignals::default(),
            );
        }
    }

    #[test]
    fn surface_native_scene_births_from_seeds_and_writes_material() {
        let spec = aura_spec();
        let seeds = vec![
            SurfaceSeed {
                local: glam::Vec3::X,
                normal: glam::Vec3::X,
                crease: 0.9,
            },
            SurfaceSeed {
                local: glam::Vec3::Y,
                normal: glam::Vec3::Y,
                crease: 0.6,
            },
        ];
        let gen = SpecGenerator::new(
            spec.terms.to_vec(),
            spec.palette_role,
            spec.base,
            seeds.clone(),
            spec.surface_affinity,
        );
        let mut params = EntityParams::new(glam::Vec3::ZERO, 0.8);
        params.presence = 1.0;
        let particles = gen.generate(20_000, &params);
        // Mist and filament both emit non-zero normal (grazing) and crease.
        assert!(particles.iter().any(|p| p.normal.length() > 0.4));
        assert!(particles.iter().any(|p| p.crease > 0.3));
    }
}
