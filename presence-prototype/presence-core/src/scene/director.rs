//! Scene Director — registry-driven stack with TTL overlays (`PRESENCE_ADAPTIVE_SCENES` P0).

use crate::behavior::{audio_response, cursor_aim, CognitiveState};
use crate::scene::disposition::Disposition;
use crate::scene::entity::EntityInstance;
use crate::scene::mode::{step_toward, ModeLayer, PresenceMode};
use crate::scene::params::SceneParams;
use crate::scene::placement::{Placement, ViewportExtent};
use crate::scene::provenance::{Provenance, ProvenanceSource};
use crate::scene::quality::QualityTier;
use crate::scene::registry::SceneRegistry;
use crate::scene::templates::builtins::{IDLE_ID, LOADING_ID};
use crate::sim::{EntityParams, PresenceSignals};

/// Maximum simultaneous dynamic scenes (excludes cloud + loading builtins).
pub const MAX_LIVE_SCENES: usize = 4;

const TRANSITION_SECONDS: f32 = 0.7;
const SUBDUED_PRESENCE: f32 = 0.45;
const LOADING_ACTIVITY_SCALE: f32 = 0.55;
const REDUCED_MOTION_TIME_SCALE: f32 = 0.12;
const REDUCED_MOTION_ACTIVITY_SCALE: f32 = 0.4;
const PRESENCE_EPSILON: f32 = 0.01;
/// Time constant for easing the cursor look-at lean. Inside the transition
/// window so the presence turns toward the pointer at the product's cadence
/// rather than snapping to it.
const CURSOR_LEAN_SECONDS: f32 = 0.25;

pub struct SceneDirector {
    pub registry: SceneRegistry,
    pub assistant_cloud: EntityInstance,
    pub loading_ring: EntityInstance,
    pub live_scenes: Vec<EntityInstance>,
    pub signals: PresenceSignals,
    pub ring_wanted: bool,
    pub modes: ModeLayer,
    /// The cognitive snapshot the Behavior Graph modulates the mode output
    /// with. Defaults to neutral, so a director that has never received a
    /// `PresenceState` behaves exactly as it did before the graph existed.
    pub cognition: CognitiveState,
    /// Latest cursor bias from the shell (`[x right, y down]`, each `[-1, 1]`)
    /// and its proximity, from `SetPresenceState`. Drives the shell's look-at
    /// lean (M7).
    cursor_dir: [f32; 2],
    cursor_proximity: f32,
    /// Eased lean applied to the shell centre — a translation toward the
    /// cursor, smoothed so the pointer never yanks the presence.
    cursor_lean: glam::Vec3,
    cloud_resting: EntityParams,
    activity_scale: f32,
    pub reduced_motion: bool,
    tier: QualityTier,
    #[allow(dead_code)]
    pub(crate) pending_palette: Option<crate::palette::PaletteId>,
    #[allow(dead_code)]
    pub(crate) pending_hittest: Option<bool>,
    #[allow(dead_code)]
    pub(crate) pending_position: Option<(i32, i32)>,
    viewport_extent: ViewportExtent,
    clock: f32,
}

impl SceneDirector {
    pub fn new() -> Self {
        let registry = SceneRegistry::with_builtin_scenes();
        let tier = QualityTier::default();
        let ceiling = tier.global_ceiling();
        let cloud_budget = tier.shell_budget().min(ceiling);

        let idle = registry.get(IDLE_ID).expect("idle template");
        let assistant_cloud = idle.build(SceneParams::default(), cloud_budget, tier, &[]);
        let cloud_resting = assistant_cloud.params;

        let loading = registry.get(LOADING_ID).expect("loading template");
        let loading_ring = loading.build(SceneParams::default(), tier.plate_budget(), tier, &[]);

        Self {
            registry,
            assistant_cloud,
            loading_ring,
            live_scenes: Vec::new(),
            signals: PresenceSignals::default(),
            ring_wanted: false,
            modes: ModeLayer::new(),
            cognition: CognitiveState::default(),
            cursor_dir: [0.0, 0.0],
            cursor_proximity: 0.0,
            cursor_lean: glam::Vec3::ZERO,
            cloud_resting,
            activity_scale: 1.0,
            reduced_motion: false,
            tier,
            pending_palette: None,
            pending_hittest: None,
            pending_position: None,
            viewport_extent: ViewportExtent::from_pixels(800, 600),
            clock: 0.0,
        }
    }

    pub fn tier(&self) -> QualityTier {
        self.tier
    }

    pub fn set_viewport_extent(&mut self, extent: ViewportExtent) {
        self.viewport_extent = extent;
    }

    pub fn set_quality_tier(&mut self, tier: QualityTier) {
        if tier == self.tier {
            return;
        }
        self.tier = tier;
        self.apply_budget_allocation();
    }

    pub fn activity_scale(&self) -> f32 {
        self.activity_scale
    }

    pub fn set_ring_wanted(&mut self, wanted: bool) {
        self.ring_wanted = wanted;
        self.loading_ring.active = wanted;
        self.apply_budget_allocation();
    }

    pub fn toggle_ring(&mut self) {
        self.set_ring_wanted(!self.ring_wanted);
    }

    /// True while a named scene is live (active, not pending dismiss).
    pub fn is_scene_live(&self, id: &str) -> bool {
        self.live_scenes
            .iter()
            .any(|e| e.scene_id == Some(id) && e.active && !e.dismiss_pending)
    }

    /// Generic dev-harness toggle for any registered scene id at a chosen
    /// disposition and placement. No TTL — the caller (debug panel / hotkey)
    /// owns the lifetime. If already live, it is dismissed.
    pub fn toggle_scene(&mut self, id: &str, disposition: Disposition, placement: Placement) {
        if self.is_scene_live(id) {
            self.dismiss_scene(id);
            return;
        }
        self.present_scene(
            id,
            SceneParams::default(),
            disposition,
            placement,
            None,
            Provenance {
                source: ProvenanceSource::Builtin,
            },
        );
    }

    pub fn set_mode(&mut self, mode: PresenceMode, engaged: bool) {
        self.modes.set(mode, engaged);
    }

    /// Records the latest cursor bias + proximity that drives the shell's
    /// look-at lean (M7). Clamped on receipt (NaN → neutral), so a misbehaving
    /// sender can only fail to aim the presence, never send it off-screen.
    pub fn set_cursor(&mut self, dir: [f32; 2], proximity: f32) {
        let clamp_signed = |v: f32| if v.is_nan() { 0.0 } else { v.clamp(-1.0, 1.0) };
        let clamp_unit = |v: f32| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
        self.cursor_dir = [clamp_signed(dir[0]), clamp_signed(dir[1])];
        self.cursor_proximity = clamp_unit(proximity);
    }

    pub fn toggle_mode(&mut self, mode: PresenceMode) {
        self.modes.toggle(mode);
    }

    /// Present a registered template on the live stack.
    pub fn present_scene(
        &mut self,
        id: &str,
        mut params: SceneParams,
        disposition: Disposition,
        placement: Placement,
        ttl: Option<f32>,
        provenance: Provenance,
    ) -> bool {
        let template = self.registry.get(id);
        let template = match template {
            Some(t) => t,
            None => {
                log::warn!("present_scene: unknown id {id}");
                return false;
            }
        };

        if id == IDLE_ID || id == LOADING_ID {
            log::warn!("present_scene: cannot present builtin shell id {id}");
            return false;
        }

        if self.live_scenes.len() >= MAX_LIVE_SCENES {
            if let Some(pos) = self.live_scenes.iter().position(|e| e.scene_id == Some(id)) {
                self.live_scenes.remove(pos);
            } else {
                log::warn!("present_scene: live stack cap {MAX_LIVE_SCENES}");
                return false;
            }
        } else if let Some(pos) = self.live_scenes.iter().position(|e| e.scene_id == Some(id)) {
            self.live_scenes.remove(pos);
        }

        params.clamp_to(&template.param_schema);
        let (_, _, scene_budgets) = self.compute_budgets_with_extra(1);
        let budget = scene_budgets.first().copied().unwrap_or(2000);

        // Snapshot the live shell skin so surface-native terms can be born from
        // it (`crate::scene::surface_seed`). Taken before build so the scene
        // inherits the shell's current folds; emitter scenes ignore it.
        let seeds = crate::scene::surface_seed::snapshot(
            &self.assistant_cloud.particles,
            self.assistant_cloud.params.center,
            self.assistant_cloud.params.scale,
        );
        let mut entity = template.build(params, budget.max(500), self.tier, &seeds);
        entity.disposition = disposition;
        entity.placement = placement.clamped();
        entity.provenance = provenance;
        entity.ttl = ttl;
        entity.spawned_at = self.clock;
        entity.dismiss_pending = false;
        entity.active = true;
        entity.presence = 0.0;

        self.apply_placement_to_entity(&mut entity, template.base_scale);
        self.live_scenes.push(entity);
        self.apply_budget_allocation();
        true
    }

    pub fn dismiss_scene(&mut self, id: &str) -> bool {
        let pos = self.live_scenes.iter().position(|e| e.scene_id == Some(id));
        match pos {
            Some(i) => {
                self.live_scenes[i].active = false;
                self.live_scenes[i].dismiss_pending = true;
                true
            }
            None => false,
        }
    }

    fn compute_budgets_with_extra(&self, extra_scenes: usize) -> (usize, usize, Vec<usize>) {
        let ceiling = self.tier.global_ceiling();
        let n = self.live_scenes.len() + extra_scenes;
        let loading_needs = self.ring_wanted
            || self.loading_ring.active
            || self.loading_ring.presence > PRESENCE_EPSILON;

        if n == 0 && !loading_needs {
            let cloud = self.tier.shell_budget().min(ceiling);
            return (cloud, 0, vec![]);
        }

        let cloud = self.tier.cloud_budget_floor().min(ceiling);
        let mut remaining = ceiling.saturating_sub(cloud);

        let loading = if loading_needs {
            let l = self.tier.plate_budget().min(remaining / 2).min(remaining);
            remaining = remaining.saturating_sub(l);
            l
        } else {
            0
        };

        let per_scene = remaining / n.max(1);
        (cloud, loading, vec![per_scene; n])
    }

    fn compute_budgets(&self) -> (usize, usize, Vec<usize>) {
        self.compute_budgets_with_extra(0)
    }

    fn apply_budget_allocation(&mut self) {
        let (cloud_b, loading_b, scene_bs) = self.compute_budgets();
        let stride = self.tier.deform_stride();
        self.assistant_cloud.set_point_budget(cloud_b, stride);
        if loading_b > 0 {
            self.loading_ring.set_point_budget(loading_b, stride);
        }
        for (entity, budget) in self.live_scenes.iter_mut().zip(scene_bs) {
            if budget > 0 {
                entity.set_point_budget(budget, stride);
            }
        }
    }

    fn apply_placement_to_entity(&mut self, entity: &mut EntityInstance, base_scale: f32) {
        let cloud_center = self.assistant_cloud.params.center;
        entity.params.center = entity
            .placement
            .resolve_center(&self.viewport_extent, cloud_center);
        entity.params.scale = entity.placement.resolved_scale(base_scale);
    }

    fn apply_entity_placements(&mut self) {
        let cloud_center = self.assistant_cloud.params.center;
        let extent = self.viewport_extent;
        let bases: Vec<f32> = self
            .live_scenes
            .iter()
            .map(|e| {
                e.scene_id
                    .and_then(|sid| self.registry.get(sid))
                    .map(|t| t.base_scale)
                    .unwrap_or(1.0)
            })
            .collect();
        for (entity, base) in self.live_scenes.iter_mut().zip(bases) {
            entity.params.center = entity.placement.resolve_center(&extent, cloud_center);
            entity.params.scale = entity.placement.resolved_scale(base);
        }
    }

    fn has_active_replace_scene(&self) -> bool {
        self.live_scenes.iter().any(|e| {
            e.disposition == Disposition::Replace
                && !e.dismiss_pending
                && (e.active || e.presence > PRESENCE_EPSILON)
        })
    }

    fn overlay_scene_active(&self) -> bool {
        self.live_scenes.iter().any(|e| {
            e.disposition == Disposition::Overlay
                && !e.dismiss_pending
                && (e.active || e.presence > PRESENCE_EPSILON)
        })
    }

    fn preemption_active(&self) -> bool {
        self.modes.is_engaged(PresenceMode::Error) || self.modes.is_engaged(PresenceMode::Speaking)
    }

    pub fn tick(&mut self, dt: f32) {
        self.clock += dt;

        if self.preemption_active() {
            for scene in &mut self.live_scenes {
                if scene.disposition == Disposition::Replace {
                    scene.active = false;
                    scene.dismiss_pending = true;
                }
            }
        }

        for scene in &mut self.live_scenes {
            if let Some(ttl) = scene.ttl {
                if !scene.dismiss_pending && self.clock - scene.spawned_at >= ttl {
                    scene.active = false;
                    scene.dismiss_pending = true;
                }
            }
        }

        let loading_target = if self.loading_ring.active { 1.0 } else { 0.0 };
        self.loading_ring.presence = step_toward(
            self.loading_ring.presence,
            loading_target,
            dt,
            TRANSITION_SECONDS,
        );

        for scene in &mut self.live_scenes {
            let target = if scene.active && !scene.dismiss_pending {
                1.0
            } else {
                0.0
            };
            scene.presence = step_toward(scene.presence, target, dt, TRANSITION_SECONDS);
        }

        let cloud_target = if self.preemption_active() {
            1.0
        } else if self.has_active_replace_scene() {
            0.0
        } else if self.loading_ring.active || self.overlay_scene_active() {
            SUBDUED_PRESENCE
        } else {
            1.0
        };
        self.assistant_cloud.presence = step_toward(
            self.assistant_cloud.presence,
            cloud_target,
            dt,
            TRANSITION_SECONDS,
        );

        self.modes.tick(dt, &self.signals);
        let carried = self.assistant_cloud.params;
        self.assistant_cloud.params = EntityParams {
            time: carried.time,
            ..self.cloud_resting
        };
        // Behavior Graph composition (ADR-014 M3): the mode layer establishes
        // the activity baseline, then cognition modulates it. Cognition is a
        // no-op while neutral, so the resting shell is unchanged.
        self.modes.apply(&mut self.assistant_cloud.params);
        self.cognition.apply(&mut self.assistant_cloud.params);

        // Speech & cursor as physics (ADR-014 M7). Both are no-ops at rest
        // (silence + centred/absent cursor), so the resting shell is unchanged.
        // Expansion is a bounded uniform scale swell riding the slow phrase
        // envelope — geometry the surface spring can carry — while the fast
        // brightness channel is already applied inside the shell shape. The
        // cursor lean is a translation of the whole shell toward the pointer,
        // eased here so the pointer never yanks it, and never a deformation.
        let voice = audio_response(
            self.assistant_cloud.params.drive.pulse,
            self.assistant_cloud.params.audio_envelope,
            self.signals.audio_level,
        );
        self.assistant_cloud.params.scale *= 1.0 + voice.expansion;

        let aim = cursor_aim(self.cursor_dir, self.cursor_proximity);
        let lean_alpha = (dt / CURSOR_LEAN_SECONDS.max(1e-4)).clamp(0.0, 1.0);
        self.cursor_lean += (aim.lean - self.cursor_lean) * lean_alpha;
        self.assistant_cloud.params.center += self.cursor_lean * self.assistant_cloud.params.scale;

        let mut scale_target = if self.loading_ring.active || self.overlay_scene_active() {
            LOADING_ACTIVITY_SCALE
        } else {
            1.0
        };
        if self.preemption_active() && self.overlay_scene_active() {
            scale_target *= LOADING_ACTIVITY_SCALE;
        }
        if self.reduced_motion {
            scale_target *= REDUCED_MOTION_ACTIVITY_SCALE;
        }
        self.assistant_cloud.params.time_scale = if self.reduced_motion {
            REDUCED_MOTION_TIME_SCALE
        } else {
            1.0
        };
        self.activity_scale =
            step_toward(self.activity_scale, scale_target, dt, TRANSITION_SECONDS);
        let s = self.activity_scale;
        if s < 1.0 - 1e-4 {
            let base = self.cloud_resting;
            let p = &mut self.assistant_cloud.params;
            p.intensity = base.intensity + (p.intensity - base.intensity) * s;
            p.expand = base.expand + (p.expand - base.expand) * s;
            p.cool = base.cool + (p.cool - base.cool) * s;
            p.drive.fold = 1.0 - (1.0 - p.drive.fold) * s;
            p.drive.lobes *= s;
            p.drive.pulse *= s;
            p.drive.neck *= s;
            p.audio_envelope *= s;
        }

        self.apply_entity_placements();

        self.assistant_cloud.update(dt, &self.signals);
        if self.loading_ring.presence > PRESENCE_EPSILON || self.loading_ring.active {
            self.loading_ring.update(dt, &self.signals);
        }
        for scene in &mut self.live_scenes {
            if scene.presence > PRESENCE_EPSILON || scene.active {
                // Hand free-space entities the current cognition so their force
                // fields (the SDF morph attractor, M5) can read focus/confidence.
                // Surface entities ignore these fields.
                scene.params.focus = self.cognition.focus;
                scene.params.confidence = self.cognition.confidence;
                scene.update(dt, &self.signals);
            }
        }

        let before = self.live_scenes.len();
        self.live_scenes
            .retain(|e| !e.dismiss_pending || e.presence > PRESENCE_EPSILON);
        if self.live_scenes.len() != before {
            self.apply_budget_allocation();
        }
    }

    pub fn entities(&self) -> Vec<&EntityInstance> {
        let mut out: Vec<&EntityInstance> = Vec::new();
        out.push(&self.assistant_cloud);
        out.push(&self.loading_ring);
        for scene in &self.live_scenes {
            out.push(scene);
        }
        out.sort_by_key(|e| e.priority);
        out
    }

    pub fn total_point_count(&self) -> usize {
        self.assistant_cloud.particles.len()
            + self.loading_ring.particles.len()
            + self
                .live_scenes
                .iter()
                .map(|e| e.particles.len())
                .sum::<usize>()
    }

    #[cfg(test)]
    pub(crate) fn test_push_live_scene(&mut self, entity: EntityInstance) {
        self.live_scenes.push(entity);
    }
}

impl Default for SceneDirector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::entity::EntityKind;
    use crate::scene::mode::TRANSITION_WINDOW_SECONDS;
    use crate::scene::placement::Anchor;
    use crate::scene::provenance::ProvenanceSource;
    use crate::scene::registry::TEST_SCENE_ID;

    #[test]
    fn ring_starts_hidden() {
        let director = SceneDirector::new();
        assert!(!director.ring_wanted);
        assert_eq!(director.loading_ring.presence, 0.0);
    }

    #[test]
    fn toggle_ring_flips_the_wanted_flag_and_active_flag() {
        let mut director = SceneDirector::new();
        assert!(!director.ring_wanted);
        director.toggle_ring();
        assert!(director.ring_wanted);
        assert!(director.loading_ring.active);
        director.toggle_ring();
        assert!(!director.ring_wanted);
        assert!(!director.loading_ring.active);
    }

    #[test]
    fn showing_loading_subdues_the_idle_shell_and_restoring_brings_it_back() {
        let mut director = SceneDirector::new();
        assert_eq!(director.assistant_cloud.presence, 1.0);

        director.toggle_ring();
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        assert!(
            (director.assistant_cloud.presence - SUBDUED_PRESENCE).abs() < 1e-3,
            "shell did not subdue: {}",
            director.assistant_cloud.presence
        );
        assert!(director.loading_ring.presence > 0.99);

        director.toggle_ring();
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        assert!(
            director.assistant_cloud.presence > 0.99,
            "shell did not recover"
        );
        assert!(director.loading_ring.presence < 0.01);
    }

    #[test]
    fn default_active_builtins_match_the_director() {
        let registry = SceneRegistry::with_builtin_scenes();
        let director = SceneDirector::new();

        for template in registry.default_active() {
            let matches = match template.entity_kind {
                EntityKind::AssistantCloud => director.assistant_cloud.kind == template.entity_kind,
                EntityKind::LoadingRing => director.loading_ring.kind == template.entity_kind,
                EntityKind::Scene => director
                    .live_scenes
                    .iter()
                    .any(|e| e.kind == template.entity_kind),
            };
            assert!(
                matches,
                "{} is default_active but the director did not build one",
                template.entity_kind.label(),
            );
        }
        assert_eq!(director.assistant_cloud.kind, EntityKind::AssistantCloud);
        assert_eq!(director.loading_ring.kind, EntityKind::LoadingRing);
    }

    #[test]
    fn set_quality_tier_regenerates_the_point_sets_at_the_new_budget() {
        let mut director = SceneDirector::new();
        assert_eq!(director.tier(), QualityTier::Balanced);
        assert_eq!(
            director.assistant_cloud.particles.len(),
            QualityTier::Balanced.shell_budget(),
        );
        assert_eq!(
            director.loading_ring.particles.len(),
            QualityTier::Balanced.plate_budget(),
        );

        director.set_quality_tier(QualityTier::Low);
        assert_eq!(director.tier(), QualityTier::Low);
        assert_eq!(
            director.assistant_cloud.particles.len(),
            QualityTier::Low.shell_budget(),
        );

        director.set_quality_tier(QualityTier::Low);
        assert_eq!(director.tier(), QualityTier::Low);
    }

    #[test]
    fn reduced_motion_slows_the_shells_clock_and_dampens_activity() {
        let mut director = SceneDirector::new();
        assert_eq!(director.assistant_cloud.params.time_scale, 1.0);

        director.toggle_mode(PresenceMode::Thinking);
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        let full = director.assistant_cloud.params;

        director.reduced_motion = true;
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        let reduced = director.assistant_cloud.params;

        assert!(
            (reduced.time_scale - REDUCED_MOTION_TIME_SCALE).abs() < 1e-4,
            "time_scale did not follow reduced_motion: {}",
            reduced.time_scale,
        );
        assert!(
            reduced.drive.lobes < full.drive.lobes,
            "reduced motion did not dampen the thinking term: {} vs {}",
            reduced.drive.lobes,
            full.drive.lobes,
        );
        assert!(
            reduced.drive.lobes > 0.0,
            "reduced motion silenced the term entirely; state cannot be read",
        );

        director.reduced_motion = false;
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        assert_eq!(
            director.assistant_cloud.params.time_scale, 1.0,
            "time_scale did not recover when reduced_motion was cleared",
        );
    }

    #[test]
    fn presence_fade_is_within_the_transition_window() {
        assert!(
            TRANSITION_WINDOW_SECONDS.contains(&TRANSITION_SECONDS),
            "entity presence fade is outside the shared transition window: {}",
            TRANSITION_SECONDS,
        );
    }

    #[test]
    fn tick_with_zero_dt_does_not_panic_or_move_presence() {
        let mut director = SceneDirector::new();
        director.toggle_ring();
        let before = director.loading_ring.presence;
        director.tick(0.0);
        assert_eq!(director.loading_ring.presence, before);
    }

    #[test]
    fn the_shell_leans_toward_the_cursor_and_returns_to_centre() {
        use crate::behavior::response::MAX_CURSOR_LEAN;

        let mut director = SceneDirector::new();
        let resting_center = director.assistant_cloud.params.center;

        // Cursor to the right, right on top of the droplet.
        director.set_cursor([1.0, 0.0], 1.0);
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        let leaned = director.assistant_cloud.params.center;
        assert!(
            leaned.x > resting_center.x + 1e-3,
            "shell did not lean toward the cursor: {leaned:?}"
        );
        // Bounded: the lean never exceeds its cap (scaled by the shell scale).
        let cap = MAX_CURSOR_LEAN * director.assistant_cloud.params.scale + 1e-3;
        assert!(
            (leaned - resting_center).length() <= cap,
            "lean exceeded its cap"
        );

        // Cursor leaves → the shell eases back to centre.
        director.set_cursor([0.0, 0.0], 0.0);
        for _ in 0..240 {
            director.tick(1.0 / 60.0);
        }
        let recovered = director.assistant_cloud.params.center;
        assert!(
            (recovered - resting_center).length() < 1e-3,
            "shell did not return to centre: {recovered:?}"
        );
    }

    #[test]
    fn speech_swells_the_shell_within_bounds_and_silence_leaves_it_at_rest() {
        use crate::behavior::response::MAX_VOICE_EXPANSION;

        let mut director = SceneDirector::new();
        let rest_scale = director.assistant_cloud.params.scale;

        // Silence: even after ticking, the shell keeps its resting scale.
        for _ in 0..60 {
            director.tick(1.0 / 60.0);
        }
        assert!(
            (director.assistant_cloud.params.scale - rest_scale).abs() < 1e-4,
            "silence changed the shell scale"
        );

        // Speaking with a loud phrase swells the shell, bounded by the cap.
        director.set_mode(PresenceMode::Speaking, true);
        director.signals.audio_level = 1.0;
        for _ in 0..180 {
            director.tick(1.0 / 60.0);
        }
        let swelled = director.assistant_cloud.params.scale;
        assert!(
            swelled > rest_scale + 1e-3,
            "speech did not swell the shell"
        );
        assert!(
            swelled <= rest_scale * (1.0 + MAX_VOICE_EXPANSION) + 1e-3,
            "speech swell exceeded its cap: {swelled} vs {rest_scale}"
        );
    }

    #[test]
    fn presence_converges_and_never_overshoots() {
        let mut presence = 0.0_f32;
        for _ in 0..600 {
            presence = step_toward(presence, 1.0, 1.0 / 60.0, TRANSITION_SECONDS);
            assert!((0.0..=1.0).contains(&presence), "escaped [0,1]: {presence}");
        }
        assert!((presence - 1.0).abs() < 1e-4);
    }

    #[test]
    fn presence_decays_back_to_zero() {
        let mut presence = 1.0_f32;
        for _ in 0..600 {
            presence = step_toward(presence, 0.0, 1.0 / 60.0, TRANSITION_SECONDS);
        }
        assert!(presence.abs() < 1e-4);
    }

    #[test]
    fn engaging_and_releasing_a_mode_returns_the_shell_to_rest() {
        let mut director = SceneDirector::new();
        let resting = director.assistant_cloud.params;

        director.toggle_mode(PresenceMode::Thinking);
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        let engaged = director.assistant_cloud.params;
        assert!(
            engaged.intensity > resting.intensity,
            "mode did not raise intensity"
        );
        assert!(engaged.drive.lobes > 0.99, "mode did not raise its term");

        director.toggle_mode(PresenceMode::Thinking);
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        let back = director.assistant_cloud.params;
        assert_eq!(back.intensity, resting.intensity, "intensity ratcheted");
        assert_eq!(back.cool, resting.cool);
        assert_eq!(
            back.drive, resting.drive,
            "a term stayed live after release"
        );
    }

    #[test]
    fn loading_dampens_active_modes_without_stopping_them() {
        let mut director = SceneDirector::new();
        director.toggle_mode(PresenceMode::Thinking);
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        let solo = director.assistant_cloud.params;
        assert!(solo.drive.lobes > 0.99);

        director.toggle_ring();
        for _ in 0..180 {
            director.tick(1.0 / 60.0);
        }
        let with_loading = director.assistant_cloud.params;

        assert!(
            (with_loading.drive.lobes - solo.drive.lobes * LOADING_ACTIVITY_SCALE).abs() < 5e-2,
            "lobes should scale by activity_scale, got {} vs solo {}",
            with_loading.drive.lobes,
            solo.drive.lobes,
        );
        assert!(
            with_loading.drive.lobes > 0.3,
            "thinking was fully quenched by loading"
        );
        assert!(
            with_loading.intensity < solo.intensity,
            "intensity did not subdue when Loading came up",
        );
        assert!(
            (director.activity_scale() - LOADING_ACTIVITY_SCALE).abs() < 1e-2,
            "activity_scale did not settle at LOADING_ACTIVITY_SCALE: {}",
            director.activity_scale(),
        );

        director.toggle_ring();
        for _ in 0..180 {
            director.tick(1.0 / 60.0);
        }
        assert!(
            (director.activity_scale() - 1.0).abs() < 1e-3,
            "activity_scale did not recover after Loading closed",
        );
    }

    #[test]
    fn modes_do_not_touch_the_loading_plate() {
        let mut director = SceneDirector::new();
        let plate_before = director.loading_ring.params;

        director.toggle_mode(PresenceMode::Speaking);
        director.toggle_mode(PresenceMode::ToolUse);
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }

        let plate = director.loading_ring.params;
        assert_eq!(plate.intensity, plate_before.intensity);
        assert_eq!(plate.drive, plate_before.drive);
    }

    #[test]
    fn the_shell_keeps_its_clock_across_a_mode_change() {
        let mut director = SceneDirector::new();
        for _ in 0..60 {
            director.tick(1.0 / 60.0);
        }
        let before = director.assistant_cloud.params.time;
        assert!(before > 0.9);

        director.toggle_mode(PresenceMode::Thinking);
        director.tick(1.0 / 60.0);
        assert!(director.assistant_cloud.params.time > before);
    }

    #[test]
    fn present_scene_adds_a_scene_and_dismiss_removes_it() {
        let mut director = SceneDirector::new();
        assert!(director.present_scene(
            TEST_SCENE_ID,
            SceneParams::default(),
            Disposition::Overlay,
            Placement::default(),
            None,
            Provenance {
                source: ProvenanceSource::Ipc,
            },
        ));
        assert_eq!(director.live_scenes.len(), 1);
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        assert!(director.live_scenes[0].presence > 0.9);

        director.dismiss_scene(TEST_SCENE_ID);
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        assert!(director.live_scenes.is_empty());
    }

    #[test]
    fn ttl_auto_dismisses_a_scene() {
        let mut director = SceneDirector::new();
        director.present_scene(
            TEST_SCENE_ID,
            SceneParams::default(),
            Disposition::Overlay,
            Placement::default(),
            Some(0.5),
            Provenance {
                source: ProvenanceSource::Ipc,
            },
        );
        for _ in 0..200 {
            director.tick(1.0 / 60.0);
        }
        assert!(director.live_scenes.is_empty());
    }

    #[test]
    fn max_live_scenes_cap_blocks_another_present() {
        let mut director = SceneDirector::new();
        let mut entities = Vec::new();
        let registry = SceneRegistry::with_builtin_scenes();
        let template = registry.get(TEST_SCENE_ID).unwrap();
        for i in 0..MAX_LIVE_SCENES {
            let mut entity =
                template.build(SceneParams::default(), 500, QualityTier::Balanced, &[]);
            entity.scene_id = Some(match i {
                0 => "slot_a",
                1 => "slot_b",
                2 => "slot_c",
                _ => "slot_d",
            });
            entities.push(entity);
        }
        for entity in entities {
            director.test_push_live_scene(entity);
        }
        assert_eq!(director.live_scenes.len(), MAX_LIVE_SCENES);
        assert!(!director.present_scene(
            TEST_SCENE_ID,
            SceneParams::default(),
            Disposition::Overlay,
            Placement::default(),
            None,
            Provenance {
                source: ProvenanceSource::Ipc,
            },
        ));
    }

    #[test]
    fn global_budget_sum_stays_within_tier_ceiling_with_scenes() {
        let mut director = SceneDirector::new();
        director.present_scene(
            TEST_SCENE_ID,
            SceneParams::default(),
            Disposition::Overlay,
            Placement::default(),
            None,
            Provenance {
                source: ProvenanceSource::Ipc,
            },
        );
        director.toggle_ring();
        director.apply_budget_allocation();
        let ceiling = director.tier().global_ceiling();
        assert!(
            director.total_point_count() <= ceiling,
            "total {} > ceiling {}",
            director.total_point_count(),
            ceiling
        );
    }

    #[test]
    fn replace_scene_crossfades_cloud_presence_down() {
        let mut director = SceneDirector::new();
        director.present_scene(
            TEST_SCENE_ID,
            SceneParams::default(),
            Disposition::Replace,
            Placement::default(),
            None,
            Provenance {
                source: ProvenanceSource::Ipc,
            },
        );
        for _ in 0..120 {
            director.tick(1.0 / 60.0);
        }
        assert!(director.assistant_cloud.presence < 0.2);
        assert!(director.live_scenes[0].presence > 0.8);
    }

    #[test]
    fn corner_placement_offsets_scene_center() {
        let mut director = SceneDirector::new();
        director.set_viewport_extent(ViewportExtent::from_pixels(800, 600));
        let placement = Placement {
            anchor: Anchor::BottomRight,
            offset: glam::Vec2::ZERO,
            scale: 0.5,
        };
        director.present_scene(
            TEST_SCENE_ID,
            SceneParams::default(),
            Disposition::Overlay,
            placement,
            None,
            Provenance {
                source: ProvenanceSource::Ipc,
            },
        );
        director.tick(1.0 / 60.0);
        let center = director.live_scenes[0].params.center;
        assert!(
            center.x > 0.5,
            "expected corner placement on +X: {center:?}"
        );
        assert!(
            center.y < 0.0,
            "expected corner placement on -Y: {center:?}"
        );
    }
}
