//! `EntityInstance` — `docs/PRESENCE_SCENES.md` §5.1.

use crate::scene::disposition::Disposition;
use crate::scene::placement::Placement;
use crate::scene::provenance::Provenance;
use crate::sim::{EntityParams, Particle, PointBehavior, PointGenerator, PresenceSignals};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    AssistantCloud,
    LoadingRing,
    /// Any data-defined scene realized from a `SceneSpec` (rain, fog, …).
    Scene,
}

impl EntityKind {
    pub fn label(&self) -> &'static str {
        match self {
            EntityKind::AssistantCloud => "AssistantCloud",
            EntityKind::LoadingRing => "LoadingRing",
            EntityKind::Scene => "Scene",
        }
    }
}

pub struct EntityInstance {
    pub kind: EntityKind,
    /// Registry id when this entity was spawned from a template.
    pub scene_id: Option<&'static str>,
    pub generator: Box<dyn PointGenerator>,
    pub behavior: Box<dyn PointBehavior>,
    pub particles: Vec<Particle>,
    pub point_budget: usize,
    pub priority: u8,
    pub active: bool,
    pub presence: f32,
    pub params: EntityParams,
    pub disposition: Disposition,
    pub placement: Placement,
    pub provenance: Provenance,
    /// Auto-dismiss after this many seconds; `None` for persistent builtins.
    pub ttl: Option<f32>,
    /// Director clock when this entity was presented.
    pub spawned_at: f32,
    /// Graceful fade-out before removal from the live stack.
    pub dismiss_pending: bool,
}

impl EntityInstance {
    pub fn new(
        kind: EntityKind,
        generator: Box<dyn PointGenerator>,
        behavior: Box<dyn PointBehavior>,
        point_budget: usize,
        priority: u8,
        params: EntityParams,
    ) -> Self {
        let particles = generator.generate(point_budget, &params);
        Self {
            kind,
            scene_id: None,
            generator,
            behavior,
            particles,
            point_budget,
            priority,
            active: true,
            presence: 1.0,
            params,
            disposition: Disposition::default(),
            placement: Placement::default(),
            provenance: Provenance::default(),
            ttl: None,
            spawned_at: 0.0,
            dismiss_pending: false,
        }
    }

    pub fn update(&mut self, dt: f32, signals: &PresenceSignals) {
        self.params.dt = dt;
        self.params.time += dt * self.params.time_scale;
        self.params.presence = self.presence;
        self.behavior
            .update(&mut self.particles, dt, &self.params, signals);
    }

    /// Regenerate particles at a new budget (global allocator / tier change).
    pub fn set_point_budget(&mut self, budget: usize, tier_stride: usize) {
        if budget == self.point_budget && self.particles.len() == budget {
            return;
        }
        self.point_budget = budget;
        self.particles = self.generator.generate(budget, &self.params);
        self.behavior.set_deform_stride(tier_stride);
    }
}
