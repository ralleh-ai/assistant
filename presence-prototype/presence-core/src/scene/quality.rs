//! Quality tiers — guide §5.5, "adaptive point count and update stride".
//!
//! Two things scale together per tier: the point budget for each entity and
//! the deform stride. They are dependent knobs, not independent ones — a
//! lower point count with an unchanged stride is a *sparser* shell but not
//! a cheaper one, since the per-particle cost is what changes with stride
//! and not with count. Doing both at once is what actually recovers frame
//! time on weak hardware.
//!
//! Two tiers rather than three. `High` at ~120k points is a mode nobody has
//! asked for and that measurement has not shown a use for; adding it would
//! be a lever with no purpose. `Low` at 30k is the one that matters — the
//! promise of "60 FPS on the hardware the assistant is expected to run on"
//! (guide §5.5) is what this exists for.

/// Selectable quality preset. See module docs for why there are two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum QualityTier {
    /// The measured default. ~150-190 FPS on the reference 2-core machine.
    #[default]
    Balanced,
    /// Fewer points, wider stride. Roughly halves the per-frame CPU cost
    /// for the shell, and reads as somewhat less crisp folds and a less
    /// dense halo — but the entity is unmistakably the same one.
    Low,
}

impl QualityTier {
    pub const ALL: [QualityTier; 2] = [QualityTier::Balanced, QualityTier::Low];

    pub fn label(self) -> &'static str {
        match self {
            QualityTier::Balanced => "balanced",
            QualityTier::Low => "low",
        }
    }

    /// Shell point count for this tier when it is the sole occupant of the budget.
    pub fn shell_budget(self) -> usize {
        match self {
            QualityTier::Balanced => 80_000,
            QualityTier::Low => 30_000,
        }
    }

    /// Loading plate point count. Scales in step with the shell — the plate
    /// is the second-biggest cost when it is showing, and cutting only the
    /// shell would leave Loading approximately at full price against a
    /// halved shell, which is the wrong shape of budget.
    pub fn plate_budget(self) -> usize {
        match self {
            QualityTier::Balanced => 40_000,
            QualityTier::Low => 15_000,
        }
    }

    /// Steps between refreshes of a given particle's cached deform (see
    /// `SurfaceBehavior::deform_stride`). Higher stride is cheaper and
    /// leaves the caching visible only when the deformation is faster than
    /// the refresh rate — see `DEFAULT_DEFORM_STRIDE`'s doc for the frame
    /// rate that has to be respected. At tier `Low` an 8-step stride puts
    /// per-particle refresh at 7-8 Hz on a 60-FPS frame, which is inside
    /// what idle's fold-evolution rate can hide but where a lobe migration
    /// or a pendant extension starts to alias visibly.
    pub fn deform_stride(self) -> usize {
        match self {
            QualityTier::Balanced => 4,
            QualityTier::Low => 8,
        }
    }

    /// The next lower tier, if any. Used by the adaptive downshifter.
    /// Global point ceiling across all live entities (`PRESENCE_ADAPTIVE_SCENES` §3.0).
    pub fn global_ceiling(self) -> usize {
        match self {
            QualityTier::Balanced => 80_000,
            QualityTier::Low => 30_000,
        }
    }

    /// Floor reserved for the cloud when overlays / replace scenes share the budget.
    pub fn cloud_budget_floor(self) -> usize {
        self.global_ceiling() / 2
    }

    pub fn lower(self) -> Option<Self> {
        match self {
            QualityTier::Balanced => Some(QualityTier::Low),
            QualityTier::Low => None,
        }
    }
}
