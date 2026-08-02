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
    /// Kept for parity with `docs/PRESENCE_SCENES.md` §5.1's shape and a
    /// future "re-seed this entity" dev action — Phase 1 generates once at
    /// construction and never regenerates.
    #[allow(dead_code)]
    pub generator: Box<dyn PointGenerator>,
    pub behavior: Box<dyn PointBehavior>,
    pub particles: Vec<Particle>,
    /// Kept for parity with the documented shape; not yet read anywhere
    /// (particle count is read directly from `particles.len()` instead).
    #[allow(dead_code)]
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
        self.params.time += dt;
        self.params.presence = self.presence;
        self.behavior
            .update(&mut self.particles, dt, &self.params, signals);
    }
}
