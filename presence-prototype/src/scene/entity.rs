//! `EntityInstance` — `docs/PRESENCE_SCENES.md` §5.1.

use crate::sim::{EntityParams, Particle, PointBehavior, PointGenerator, PresenceSignals};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    AssistantCloud,
    LoadingRing,
}

impl EntityKind {
    pub fn label(&self) -> &'static str {
        match self {
            EntityKind::AssistantCloud => "AssistantCloud",
            EntityKind::LoadingRing => "LoadingRing",
        }
    }
}

pub struct EntityInstance {
    pub kind: EntityKind,
    /// Used by the scene director's quality-tier path to regenerate points
    /// at a new budget without rebuilding the entity from scratch.
    pub generator: Box<dyn PointGenerator>,
    pub behavior: Box<dyn PointBehavior>,
    pub particles: Vec<Particle>,
    /// Kept in sync with the last budget the generator was asked for. Not
    /// the source of truth for the current point count (that is
    /// `particles.len()`) — it is what the tier switch reads to decide
    /// whether a regeneration would change anything.
    pub point_budget: usize,
    /// Hierarchy ordering for future multi-entity crowding rules
    /// (`docs/PRESENCE_VISUAL_ENTITY.md` §4.3) — not yet enforced.
    #[allow(dead_code)]
    pub priority: u8,
    /// Whether the Scene Director currently wants this entity visible.
    /// Distinct from `presence` (§ below) — `active` is the target,
    /// `presence` is the (possibly still-transitioning) current fade level.
    pub active: bool,
    /// 0.0 (fully dissolved) .. 1.0 (fully present). Lerped toward
    /// `active`'s target each frame by the director — this is what makes
    /// entities fade in/out instead of popping.
    pub presence: f32,
    pub params: EntityParams,
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
            generator,
            behavior,
            particles,
            point_budget,
            priority,
            active: true,
            presence: 1.0,
            params,
        }
    }

    pub fn update(&mut self, dt: f32, signals: &PresenceSignals) {
        self.params.dt = dt;
        // The animation clock advances at whatever fraction of real time the
        // scene has asked for — see `EntityParams::time_scale`. The physics
        // clock passed to the behavior below is always real time.
        self.params.time += dt * self.params.time_scale;
        self.params.presence = self.presence;
        self.behavior
            .update(&mut self.particles, dt, &self.params, signals);
    }
}
