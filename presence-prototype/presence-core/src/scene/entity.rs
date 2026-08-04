//! `EntityInstance` — `docs/PRESENCE_SCENES.md` §5.1.

use crate::scene::disposition::Disposition;
use crate::scene::placement::Placement;
use crate::scene::provenance::Provenance;
use crate::sim::{EntityParams, Particle, PointBehavior, PointGenerator, PresenceSignals};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    AssistantCloud,
    LoadingRing,
    /// Any data-defined scene realized from a `SceneSpec`.
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
    /// Hard ceiling the director's budget allocation cannot exceed for this
    /// entity, regardless of how much of the global budget is free. `None`
    /// leaves the entity free to take its whole share. Free-space field
    /// entities set this low: each of their points integrates curl noise every
    /// step and never caches it, so a point costs many times what a cached
    /// surface point does, and a nebula reads fine at a fraction of the count.
    pub max_budget: Option<usize>,
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
            max_budget: None,
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

    /// Resize the particle set to a new budget (global allocator / tier change).
    ///
    /// Shrinking thins by truncation rather than regenerating: both generators
    /// emit points in independent order (the surface generator draws each
    /// point's seed from an RNG; the field generator hashes per index), so
    /// dropping the tail keeps a uniform random subset instead of carving a
    /// contiguous bald patch — and it skips the noise a full regenerate
    /// re-evaluates. That regenerate is the frame-long hitch a scene appearing
    /// used to cost, since the shell drops to its budget floor the moment any
    /// scene shares the global budget. Growing still regenerates: there are no
    /// extra points to keep, and the settled ones are preserved by generating
    /// a fresh, larger set. `max_budget` caps the request either way.
    pub fn set_point_budget(&mut self, budget: usize, tier_stride: usize) {
        let budget = self.max_budget.map_or(budget, |cap| budget.min(cap));
        self.behavior.set_deform_stride(tier_stride);
        if budget == self.particles.len() {
            self.point_budget = budget;
            return;
        }
        self.point_budget = budget;
        if budget < self.particles.len() {
            self.particles.truncate(budget);
        } else {
            self.particles = self.generator.generate(budget, &self.params);
        }
    }
}
