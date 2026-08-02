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

/// How long a presence fade (entity appearing/disappearing) takes, within
/// the 400-1200ms range `docs/PRESENCE_SCENES.md` §6.1 recommends.
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

/// Points in the idle shell.
///
/// Well above the 8-12k the design documents originally suggested, and the
/// reason is the switch from a volume to a surface: 12,000 points spread over a
/// skin resolve as countable dots, because a surface concentrates them where
/// they are individually visible instead of hiding most of them behind the
/// front of a volume. The number is set by measurement — see
/// `presence-prototype/README.md`'s performance section.
const IDLE_POINT_BUDGET: usize = 80_000;

/// Points in the loading plate. Smaller than the shell's: a disk seen face-on
/// spends every point on visible area, where a closed shell hides roughly half
/// of its own behind the front.
///
/// Not proportionally smaller, though. The plate is wider than the shell and
/// flat, so its points spread over several times the area, and grains migrating
/// onto nodal lines only concentrate them where the pattern already is — it
/// does not make the plate as a whole any denser. Below roughly this count the
/// nodal lines resolve as dotted rather than drawn.
const LOADING_POINT_BUDGET: usize = 40_000;

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
        let shell = PresenceShell::new(0x1DEE);
        let assistant_cloud = EntityInstance::new(
            EntityKind::AssistantCloud,
            Box::new(SurfaceGenerator::new(shell.domain())),
            Box::new(SurfaceBehavior::new(shell)),
            IDLE_POINT_BUDGET,
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
        let mut loading_ring = EntityInstance::new(
            EntityKind::LoadingRing,
            Box::new(SurfaceGenerator::new(plate.domain())),
            Box::new(SurfaceBehavior::new(plate)),
            // A modal pattern is legible only if grains are dense enough to
            // draw its nodal lines. Showing this at full density alongside a
            // full-density idle shell is a dev-harness artifact of toggling both
            // at once — in production Loading is a scene that reduces the shell
            // rather than an overlay on top of it.
            LOADING_POINT_BUDGET,
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
        }
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
