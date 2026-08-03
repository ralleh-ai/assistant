//! Builtin `SceneTemplate` factories for idle shell and loading plate.

use glam::Vec3;

use crate::scene::disposition::Disposition;
use crate::scene::entity::{EntityInstance, EntityKind};
use crate::scene::params::SceneParams;
use crate::scene::placement::Placement;
use crate::scene::provenance::Provenance;
use crate::scene::quality::QualityTier;
use crate::sim::shapes::{
    PresenceShell, ResonancePlate, SurfaceBehavior, SurfaceGenerator, SurfaceShape,
};
use crate::sim::EntityParams;

const IDLE_INTENSITY: f32 = 0.15;
const IDLE_SWIRL: f32 = 0.0;
const IDLE_EXPAND: f32 = 0.0;
const IDLE_COOL: f32 = 0.0;

pub const IDLE_ID: &str = "idle";
pub const LOADING_ID: &str = "loading";

pub fn build_idle(params: SceneParams, point_budget: usize, tier: QualityTier) -> EntityInstance {
    let _ = params;
    let cloud_params = {
        let mut p = EntityParams::new(Vec3::ZERO, 1.32);
        p.intensity = IDLE_INTENSITY;
        p.swirl = IDLE_SWIRL;
        p.expand = IDLE_EXPAND;
        p.cool = IDLE_COOL;
        p
    };

    let shell = PresenceShell::new(0x1DEE);
    let shell_domain = shell.domain();
    let mut shell_behavior = SurfaceBehavior::new(shell);
    shell_behavior.deform_stride = tier.deform_stride();

    let mut entity = EntityInstance::new(
        EntityKind::AssistantCloud,
        Box::new(SurfaceGenerator::new(shell_domain)),
        Box::new(shell_behavior),
        point_budget,
        0,
        cloud_params,
    );
    entity.scene_id = Some(IDLE_ID);
    entity.disposition = Disposition::Overlay;
    entity.placement = Placement::default();
    entity.provenance = Provenance::default();
    entity
}

pub fn build_loading(
    params: SceneParams,
    point_budget: usize,
    tier: QualityTier,
) -> EntityInstance {
    let _ = params;
    let ring_params = {
        let mut p = EntityParams::new(Vec3::ZERO, 1.5);
        p.intensity = 0.7;
        p.expand = 0.1;
        p
    };

    let plate = ResonancePlate::new(0x400D);
    let plate_domain = plate.domain();
    let mut plate_behavior = SurfaceBehavior::new(plate);
    plate_behavior.deform_stride = tier.deform_stride();

    let mut entity = EntityInstance::new(
        EntityKind::LoadingRing,
        Box::new(SurfaceGenerator::new(plate_domain)),
        Box::new(plate_behavior),
        point_budget,
        1,
        ring_params,
    );
    entity.scene_id = Some(LOADING_ID);
    entity.active = false;
    entity.presence = 0.0;
    entity.disposition = Disposition::Overlay;
    entity.placement = Placement::default();
    entity.provenance = Provenance::default();
    entity
}
