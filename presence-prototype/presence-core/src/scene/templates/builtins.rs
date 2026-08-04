//! Builtin `SceneTemplate` factories for idle shell and loading plate.

use glam::Vec3;

use crate::scene::disposition::Disposition;
use crate::scene::entity::{EntityInstance, EntityKind};
use crate::scene::params::SceneParams;
use crate::scene::placement::Placement;
use crate::scene::provenance::Provenance;
use crate::scene::quality::QualityTier;
use crate::sim::field::{FieldBehavior, FieldCloudGenerator, MorphTarget};
use crate::sim::shapes::{
    PresenceShell, ResonancePlate, SurfaceBehavior, SurfaceGenerator, SurfaceShape,
};
use crate::sim::{EntityParams, MorphBehavior, PointBehavior};

/// Seeds the shell's noise and, through `MorphBehavior`, the ambient field the
/// body drifts in once it leaves the droplet. One seed for both so a given
/// presence is one deterministic thing across its whole shape vocabulary.
const SHELL_SEED: u32 = 0x1DEE;

const IDLE_INTENSITY: f32 = 0.15;
const IDLE_SWIRL: f32 = 0.0;
const IDLE_EXPAND: f32 = 0.0;
const IDLE_COOL: f32 = 0.0;

pub const IDLE_ID: &str = "idle";
pub const LOADING_ID: &str = "loading";
/// First free-space (force-field) entity — a nebula. Not `default_active`; it
/// ships as the end-to-end proof of the M4 field substrate and the base the
/// morph milestone (M5) builds on, presented on demand rather than at rest.
pub const FIELD_CLOUD_ID: &str = "field_cloud";

const FIELD_CLOUD_SEED: u32 = 0x0FF5E7;
const FIELD_CLOUD_RADIUS: f32 = 1.0;

/// Point ceiling for the free-space cloud, per tier — deliberately far below
/// the surface budgets. A field point integrates the composite field (an
/// 18-tap curl plus drift and the SDF pull) every step and, unlike a surface
/// point, never caches that work behind the deform stride, so it costs many
/// times what a shell point does. A morphing nebula also reads well far
/// sparser than a scanned skin. Left uncapped the director hands this half the
/// global budget (~40k on Balanced), which both stalls on generation and drags
/// the frame; these caps keep it cheap to present and cheap to run.
fn field_cloud_budget(tier: QualityTier) -> usize {
    match tier {
        QualityTier::Balanced => 10_000,
        QualityTier::Low => 5_000,
    }
}

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

    // `MorphBehavior` rather than a bare `SurfaceBehavior`: this is the one
    // body ADR-015 is about, and it has to be able to leave the droplet without
    // a second entity being born to take over. At the resting form it *is* a
    // `SurfaceBehavior` — see `the_resting_body_is_bit_identical_to_the_surface
    // _behavior` — so nothing about the idle shell changes by adopting it.
    let shell = PresenceShell::new(SHELL_SEED);
    let shell_domain = shell.domain();
    let mut shell_behavior = MorphBehavior::new(shell, SHELL_SEED);
    shell_behavior.set_deform_stride(tier.deform_stride());
    shell_behavior.field_stride = tier.field_stride();

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

/// A free-space nebula driven by the force-field substrate (`sim::field`).
///
/// Unlike the shell and plate, this entity has no surface: its points are a
/// volume the composite field carries. It is the first consumer of the curl
/// noise ADR-011 left dead. Its SDF morph attractor (M5) condenses it onto a
/// sphere as cognitive focus rises and lets it relax to a loose, suggestive
/// cloud at rest. Presented on demand (never `default_active`), so the running
/// app is still just the idle shell and loading plate until something asks.
pub fn build_field_cloud(
    params: SceneParams,
    point_budget: usize,
    tier: QualityTier,
) -> EntityInstance {
    let _ = params;
    let cloud_params = {
        let mut p = EntityParams::new(Vec3::ZERO, 1.0);
        p.intensity = 0.5;
        p.cool = 0.6;
        p.core_density_bias = 0.0;
        p
    };

    // Cap the request up front so the very first generation is already cheap,
    // then carry the cap on the entity so the director's later reallocations
    // cannot grow it back past what a free-space cloud should cost.
    let cap = field_cloud_budget(tier);
    let budget = point_budget.min(cap);

    let mut entity = EntityInstance::new(
        EntityKind::Scene,
        Box::new(FieldCloudGenerator::new(
            FIELD_CLOUD_SEED,
            FIELD_CLOUD_RADIUS,
        )),
        Box::new(FieldBehavior::morph(
            FIELD_CLOUD_SEED,
            MorphTarget::Sphere { radius: 1.0 },
        )),
        budget,
        2,
        cloud_params,
    );
    entity.max_budget = Some(cap);
    entity.scene_id = Some(FIELD_CLOUD_ID);
    entity.active = false;
    entity.presence = 0.0;
    entity.disposition = Disposition::Overlay;
    entity.placement = Placement::default();
    entity.provenance = Provenance::default();
    entity
}
