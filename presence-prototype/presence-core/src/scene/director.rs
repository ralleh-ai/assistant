//! Scene Director — `docs/PRESENCE_SCENES.md` §6.3 / `docs/PRESENCE_VISUAL_ENTITY.md` §8.
//!
//! Phase 1 scope (`docs/PRESENCE_SCENES.md` §9): drive `AssistantCloud`
//! (always present, Idle-only visual signature) and toggle `LoadingRing`
//! on/off as a secondary entity, with a continuous presence-fade
//! transition rather than a hard cut. "High-level state from the
//! assistant" is dev-control input here (keyboard/egui), not a real
//! signal — see `docs/PRESENCE_INTEGRATION_PLAN.md` §4, Phase 1.

use glam::Vec3;

use crate::scene::entity::{EntityInstance, EntityKind};
use crate::scene::mode::{step_toward, ModeLayer, PresenceMode};
use crate::scene::quality::QualityTier;
use crate::sim::shapes::{
    PresenceShell, ResonancePlate, SurfaceBehavior, SurfaceGenerator, SurfaceShape,
};
use crate::sim::{EntityParams, PresenceSignals};

/// Idle's per-mode multipliers, matching the `DEFAULTS.states.idle` entry
/// in `docs/PRESENCE_VISUAL_ENTITY.md` §9. These are the *resting* values the
/// mode layer raises from, not a state of their own — with nothing engaged the
/// presence sits here, and `ModeLayer::apply` leaves them untouched.
const IDLE_INTENSITY: f32 = 0.15;
/// Zero, not merely small: §6's usage table specifies idle as "low-frequency
/// Simplex + gentle scale, almost no curl". A token non-zero swirl bought
/// sub-pixel motion for 18 of the 19 noise evaluations per particle, so
/// idle's wander comes from `NoiseField::drift` instead. Curl re-enters with
/// `thinking` ("highest internal complexity"), which is a later phase.
const IDLE_SWIRL: f32 = 0.0;
const IDLE_EXPAND: f32 = 0.0;
const IDLE_COOL: f32 = 0.0;

/// How long a presence fade (entity appearing/disappearing) takes.
///
/// Inside the same `TRANSITION_WINDOW_SECONDS` the mode weights use, so an
/// entity coming or going has the same visual cadence as a mode engaging
/// or releasing — a shell speeding up its terms twice as fast as it fades
/// itself in would read as two motion languages spliced together. This is
/// at the slow end of the window on purpose: an entity vanishing is a
/// bigger event than a mode weight sliding, and the fade has to be
/// unmistakably deliberate rather than glitchy. Guarded by
/// `presence_fade_is_within_the_transition_window`.
const TRANSITION_SECONDS: f32 = 0.7;

/// What the idle shell fades to while Loading is showing.
///
/// Loading *reduces* the primary presence rather than compositing over a
/// full-strength one — `docs/PRESENCE_SCENES.md` §4.2, and §2.2's "hierarchy of
/// attention" applied to simultaneous entities. At full strength the two
/// entities are the same hue at similar scale and simply sum into a denser
/// blob, so the modal pattern that distinguishes Loading is not merely
/// harder to see, it is not visible at all.
///
/// Not lower, though: the shell has to stay legible as the thing Loading is
/// happening *to*. Taken far enough down it reads as the presence having been
/// replaced rather than occupied, which is the wrong story for a transient
/// state.
const SUBDUED_PRESENCE: f32 = 0.45;

/// How much the *modes* are pulled back toward the resting shell while
/// Loading is active. `presence` alone (above) fades the shell's brightness,
/// but leaves its geometry — a lobe or a pendant would still visibly deform
/// the silhouette at half brightness and steal attention from the plate.
///
/// This scales the additive mode contributions (intensity, expand, cool, and
/// the shell-drive weights) toward the resting values, so activity persists
/// *inside* the subdued shell rather than dominating it. Guide §5.2 asks for
/// exactly this — "automatically subdue the shell intensity" when Loading is
/// composited with activity — and P0.4 asks for the general rule the
/// SceneDirector enforces on its own.
///
/// Not zero: a shell that stops thinking the moment Loading appears would
/// read as the work being *paused* to load rather than continuing behind the
/// wait. The plate is signalling progress on top of the still-running task.
const LOADING_ACTIVITY_SCALE: f32 = 0.55;

/// How slowly the shell's animation clock advances in reduced-motion mode,
/// as a fraction of real time. Small, because the point of the mode is that
/// motion becomes non-distracting rather than merely slower — 0.5 is still
/// visibly animating, just laggily. Not zero either: a shell whose folds
/// never reshape at all reads as a frozen bug rather than as a considered
/// accessibility state, and the "am I looking at a static image" question
/// is exactly the ambiguity a live presence must not create.
const REDUCED_MOTION_TIME_SCALE: f32 = 0.12;

/// How far mode-added intensity/expand/cool is pulled back in reduced-motion
/// mode. Guide §5.5's reduced-motion preset says "collapse most deformation
/// to brightness + slow breathing" — this is the deformation half, applied
/// on top of `LOADING_ACTIVITY_SCALE`'s hierarchy pass so both rules take
/// their expected share when both are on at once.
const REDUCED_MOTION_ACTIVITY_SCALE: f32 = 0.4;

// Point budgets and refresh stride are supplied by `QualityTier` — the same
// values live there, and a comment describing the reasoning for `Balanced`
// (the previous compile-time constants) is at that module.

pub struct SceneDirector {
    pub assistant_cloud: EntityInstance,
    pub loading_ring: EntityInstance,
    pub signals: PresenceSignals,
    pub ring_wanted: bool,
    /// What the assistant is doing, and how far each mode's contribution has
    /// ramped. Drives the shell only — the loading plate is a separate entity
    /// on a different domain and is orthogonal to all of this.
    pub modes: ModeLayer,
    /// The shell's resting parameters, before the mode layer raises them.
    /// Held separately because `apply` blends *from* a baseline, so writing
    /// the result back into the entity's own params would let each frame's
    /// output become the next frame's floor and ratchet the presence upward.
    cloud_resting: EntityParams,
    /// Fraction of the additive mode contribution that is applied to the
    /// shell. `1.0` when nothing competes for attention, easing down toward
    /// `LOADING_ACTIVITY_SCALE` while Loading is showing. This is the
    /// hierarchy-of-attention rule made explicit rather than left to a
    /// coincidence of tuning.
    activity_scale: f32,
    /// Accessibility preference: minimise motion while keeping the presence
    /// legible. When true the shell's animation clock slows to
    /// `REDUCED_MOTION_TIME_SCALE` and mode contributions to
    /// `REDUCED_MOTION_ACTIVITY_SCALE`, so state is still communicated but
    /// the shell mostly stops moving.
    pub reduced_motion: bool,
    /// Active quality tier. Read-only from outside — mutate with
    /// `set_quality_tier` so the entities are regenerated at the new budget
    /// rather than the value drifting out of sync with the point sets.
    tier: QualityTier,
    /// A palette change requested by a shell command (`presence_ipc`'s
    /// `Command::SetPalette`) that the runtime has not yet copied into
    /// `Renderer::palette`. `None` when nothing is pending. Only touched
    /// by `SceneDirector::apply_command` / `take_pending_palette` in
    /// `crate::ipc` — those live behind the `ipc` feature, so this field
    /// is `#[allow(dead_code)]` under a no-features build.
    #[allow(dead_code)]
    pub(crate) pending_palette: Option<crate::palette::PaletteId>,
    /// A hittest / interactivity change requested by
    /// `Command::SetInteractive`. Same idiom as `pending_palette`
    /// because winit's `set_cursor_hittest` needs the `Window`
    /// handle, which lives in the runtime, not in this crate.
    #[allow(dead_code)]
    pub(crate) pending_hittest: Option<bool>,
}

impl SceneDirector {
    pub fn new() -> Self {
        let cloud_params = {
            // Scaled to leave margin inside the viewport: §2.1 frames this as
            // a volume observed inside a scanned space, which needs the entity
            // to sit within the frame rather than crop against it.
            //
            // The margin is sized for the *loudest* state, not for idle. A
            // shell scaled to fill the frame at rest has nowhere to put a lobe
            // or a pendant, so every mode would crop against the top of the
            // viewport — and cropping is the one thing that makes a presence
            // read as a texture behind the window rather than as an object in
            // it.
            let mut p = EntityParams::new(Vec3::ZERO, 1.32);
            p.intensity = IDLE_INTENSITY;
            p.swirl = IDLE_SWIRL;
            p.expand = IDLE_EXPAND;
            p.cool = IDLE_COOL;
            p
        };
        // One shell for every mode, not one shape per state. What the assistant
        // is doing raises weights on it (see `ModeLayer`); with nothing engaged
        // those weights are the resting fold and the shell is the idle
        // signature. The generator and behavior split from `PRESENCE_SCENES.md`
        // §5 is unchanged — the shape is what the behavior springs toward.
        let tier = QualityTier::default();
        let shell = PresenceShell::new(0x1DEE);
        let shell_domain = shell.domain();
        let mut shell_behavior = SurfaceBehavior::new(shell);
        shell_behavior.deform_stride = tier.deform_stride();
        let assistant_cloud = EntityInstance::new(
            EntityKind::AssistantCloud,
            Box::new(SurfaceGenerator::new(shell_domain)),
            Box::new(shell_behavior),
            tier.shell_budget(),
            0,
            cloud_params,
        );
        let cloud_resting = cloud_params;

        let ring_params = {
            // Wider than the shell, so the two entities read as a field with
            // the presence at its centre rather than as two overlapping
            // objects competing for the same space — §2.2's "hierarchy of
            // attention" applied to simultaneous entities.
            //
            // This is a *half-width*, not a radius: the sheet is square, so its
            // corners reach `scale * sqrt(2)`. Carried over from the round
            // plate unchanged it overflowed the viewport diagonally.
            let mut p = EntityParams::new(Vec3::ZERO, 1.5);
            p.intensity = 0.7;
            p.expand = 0.1;
            p
        };
        let plate = ResonancePlate::new(0x400D);
        let plate_domain = plate.domain();
        let mut plate_behavior = SurfaceBehavior::new(plate);
        plate_behavior.deform_stride = tier.deform_stride();
        let mut loading_ring = EntityInstance::new(
            EntityKind::LoadingRing,
            Box::new(SurfaceGenerator::new(plate_domain)),
            Box::new(plate_behavior),
            // A modal pattern is legible only if grains are dense enough to
            // draw its nodal lines. Showing this at full density alongside a
            // full-density idle shell is a dev-harness artifact of toggling both
            // at once — in production Loading is a scene that reduces the shell
            // rather than an overlay on top of it.
            tier.plate_budget(),
            1,
            ring_params,
        );
        // Starts hidden — Phase 1 opens on the calm Idle-only view.
        loading_ring.active = false;
        loading_ring.presence = 0.0;

        Self {
            assistant_cloud,
            loading_ring,
            signals: PresenceSignals::default(),
            ring_wanted: false,
            modes: ModeLayer::new(),
            cloud_resting,
            activity_scale: 1.0,
            reduced_motion: false,
            tier,
            pending_palette: None,
            pending_hittest: None,
        }
    }

    /// Currently active quality tier. Change with `set_quality_tier`, which
    /// regenerates the point sets — you cannot mutate this directly.
    pub fn tier(&self) -> QualityTier {
        self.tier
    }

    /// Switch to a different quality tier. This is deliberately an outside
    /// operation on the director rather than a field: changing tier means
    /// regenerating each entity's particles at the new budget, which is
    /// visible as a brief re-settling — and something the caller should be
    /// aware they are triggering rather than something a stray write can do.
    pub fn set_quality_tier(&mut self, tier: QualityTier) {
        if tier == self.tier {
            return;
        }
        self.tier = tier;
        // Point count is set at generation time and the vector is what the
        // renderer reads directly, so a tier change means regenerating both
        // entities. Cheap enough — 80k particles come out in a few
        // milliseconds and this is an infrequent action.
        self.assistant_cloud.particles = self
            .assistant_cloud
            .generator
            .generate(tier.shell_budget(), &self.assistant_cloud.params);
        self.loading_ring.particles = self
            .loading_ring
            .generator
            .generate(tier.plate_budget(), &self.loading_ring.params);
        self.assistant_cloud.point_budget = tier.shell_budget();
        self.loading_ring.point_budget = tier.plate_budget();
        self.assistant_cloud.behavior.set_deform_stride(tier.deform_stride());
        self.loading_ring.behavior.set_deform_stride(tier.deform_stride());
    }

    /// The current activity dampening factor. `1.0` at rest, easing toward
    /// `LOADING_ACTIVITY_SCALE` while Loading is active. Exposed so the
    /// debug overlay can show the resolved hierarchy value; the tick uses it
    /// internally.
    pub fn activity_scale(&self) -> f32 {
        self.activity_scale
    }

    pub fn set_ring_wanted(&mut self, wanted: bool) {
        self.ring_wanted = wanted;
        self.loading_ring.active = wanted;
    }

    pub fn toggle_ring(&mut self) {
        self.set_ring_wanted(!self.ring_wanted);
    }

    pub fn set_mode(&mut self, mode: PresenceMode, engaged: bool) {
        self.modes.set(mode, engaged);
    }

    pub fn toggle_mode(&mut self, mode: PresenceMode) {
        self.modes.toggle(mode);
    }

    pub fn tick(&mut self, dt: f32) {
        let target = if self.loading_ring.active { 1.0 } else { 0.0 };
        self.loading_ring.presence =
            step_toward(self.loading_ring.presence, target, dt, TRANSITION_SECONDS);

        let cloud_target = if self.loading_ring.active {
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

        // Resolved fresh from the resting baseline each frame rather than
        // nudged from wherever it ended up last frame, so a mode's weight is
        // the single source of how far the presence has departed from calm.
        self.modes.tick(dt, &self.signals);
        let carried = self.assistant_cloud.params;
        self.assistant_cloud.params = EntityParams {
            time: carried.time,
            ..self.cloud_resting
        };
        self.modes.apply(&mut self.assistant_cloud.params);

        // Hierarchy of attention: while Loading is showing, pull the mode's
        // added intensity/expand/cool and its drive weights back toward the
        // resting shell so the plate's modal pattern is not fighting a
        // full-strength thinking or tool-use signature for the eye. The
        // effect is *proportional* — a mode still shows through, just
        // subdued in step with the shell's own subdued brightness above.
        //
        // Reduced-motion multiplies the same scale further: an accessibility
        // preference should compose with the hierarchy rule, not race it. If
        // both are on the effective scale is the *product*.
        let mut scale_target = if self.loading_ring.active {
            LOADING_ACTIVITY_SCALE
        } else {
            1.0
        };
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
            // The additive-term weights scale directly. Fold is 1.0 at rest
            // and yields under load, so its subdue is `1 - (1 - fold) * s`
            // — pulling *toward* 1.0 rather than toward 0.0, so the shell's
            // identity does not evaporate along with the activity.
            p.drive.fold = 1.0 - (1.0 - p.drive.fold) * s;
            p.drive.lobes *= s;
            p.drive.pulse *= s;
            p.drive.neck *= s;
            // Speech's phrase envelope is the geometric driver of the pulse
            // term (§6, spring bandwidth), so subduing pulse without also
            // subduing the envelope would leave the wave amplitude untouched
            // while claiming to have quieted the shell.
            p.audio_envelope *= s;
        }

        self.assistant_cloud.update(dt, &self.signals);
        // Skip simulating the ring once it's fully invisible and settled —
        // no visible cost, and matches "calm by default" (don't spend
        // budget animating something nobody can see).
        if self.loading_ring.presence > 0.001 || self.loading_ring.active {
            self.loading_ring.update(dt, &self.signals);
        }
    }

    pub fn entities(&self) -> [&EntityInstance; 2] {
        [&self.assistant_cloud, &self.loading_ring]
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
    use crate::scene::mode::TRANSITION_WINDOW_SECONDS;

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

    /// Loading has to take attention *from* the shell rather than adding to it.
    /// Without this the two entities sum into a denser blob and Loading has no
    /// distinguishable signature at all.
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

    /// The registry is the description of what scenes exist; the director
    /// is the code that instantiates them. Anything that lives in one and
    /// not the other is a scene that is either invisible in the debug panel
    /// or invisible on the screen, and both failure modes stay silent until
    /// somebody hits them. This test asserts they cover the same set.
    #[test]
    fn builtins_match_the_scene_director() {
        use crate::scene::SceneRegistry;
        let registry = SceneRegistry::with_builtin_scenes();
        let director = SceneDirector::new();

        let director_kinds: Vec<_> = director.entities().iter().map(|e| e.kind).collect();
        let registry_kinds: Vec<_> = registry.all().map(|d| d.entity_kind).collect();
        for kind in &director_kinds {
            assert!(
                registry_kinds.contains(kind),
                "{} is instantiated by the director but not registered",
                kind.label(),
            );
        }
        for kind in &registry_kinds {
            assert!(
                director_kinds.contains(kind),
                "{} is registered but the director doesn't build one",
                kind.label(),
            );
        }
    }

    /// A tier switch is not a knob the caller mutates directly; it is an
    /// operation that regenerates the point sets to match the new budget.
    /// Testing it as an *operation* is what makes the invariant "tier and
    /// particle count agree" impossible to accidentally violate later.
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
        assert_eq!(
            director.loading_ring.particles.len(),
            QualityTier::Low.plate_budget(),
        );

        // Idempotent on the same tier — no regeneration, no work.
        director.set_quality_tier(QualityTier::Low);
        assert_eq!(director.tier(), QualityTier::Low);
    }

    /// Reduced-motion collapses shell dynamics to breath + slow crease
    /// updates while leaving the physics real, so modes still communicate
    /// state via brightness but nothing distracts.
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

    /// Enforces the family similarity with the mode transitions — the mode
    /// layer polices its own window, but a slow entity fade paired with fast
    /// mode ramps would give the presence two different tempos that any
    /// desktop-edge crossfade would then splice into.
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

    // Transition math (docs/PRESENCE_SCENES.md §6.1-6.2), tested directly
    // as a pure function rather than by running hundreds of full-particle
    // simulation ticks (which is what `SceneDirector::tick` does and is
    // deliberately not cheap in a debug build).
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

    /// The shell's params are rebuilt from the resting baseline every frame.
    /// Blending from last frame's *output* instead would make each frame's
    /// result the next frame's floor, so intensity would ratchet up and never
    /// come back down — a drift that only shows after minutes of running.
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

    /// Hierarchy of attention: Loading + Thinking is not two full-strength
    /// signatures competing for the eye, it is Loading in front of a subdued
    /// still-running shell. The check is that the shell's mode-added values
    /// are strictly *lower* when Loading is up than when it is not, not that
    /// they hit any specific number — the constant can move; the ordering
    /// cannot.
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
        assert!(with_loading.drive.lobes > 0.3, "thinking was fully quenched by loading");
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

    /// Loading is a separate entity on a different domain, so a mode must not
    /// reach it. If it did, the plate would inherit the shell's drive and its
    /// figures would change for reasons that have nothing to do with progress.
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
        // A shape's animation is a function of `time`, so resetting it while
        // rebuilding the params would teleport the folds on every state change.
        assert!(director.assistant_cloud.params.time > before);
    }
}
