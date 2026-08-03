//! Primitive scene-term vocabulary — `PRESENCE_ADAPTIVE_SCENES` Phase 1 T-1.1.
//!
//! Each module here is one *primitive*: a pure generator/behavior pair that
//! operates on plain `Particle`s given a [`TermCtx`]. Primitives know nothing
//! about scenes or specs — the realizer (`crate::scene::realize`) is what maps
//! a `SceneTerm` to the functions below and owns the budget split. This keeps
//! the vocabulary small and reviewable while any number of data-defined scenes
//! reuse it.

pub mod cloud;
pub mod rain;

use glam::Vec3;

/// Per-frame context shared by every term. Positions are always derived as
/// `center + local * scale`, so a term's particles follow their entity's
/// `Placement` for free.
#[derive(Clone, Copy, Debug)]
pub struct TermCtx {
    pub center: Vec3,
    pub scale: f32,
    /// Entity clock (already advanced at `time_scale`, so reduced motion slows
    /// every term together).
    pub time: f32,
    /// 0..1 transition fade for the whole entity.
    pub presence: f32,
    /// Baseline `color_bias` from the spec's `PaletteRole`.
    pub color_bias: f32,
}

/// Split `count` points across term `weights`, proportionally, with any
/// rounding remainder handed to the first term. The result always sums to
/// `count`, so the realizer's generator and behavior derive identical slice
/// boundaries without tagging particles.
pub fn split(count: usize, weights: &[f32]) -> Vec<usize> {
    if weights.is_empty() {
        return Vec::new();
    }
    let total: f32 = weights.iter().copied().map(|w| w.max(0.0)).sum();
    if total <= f32::EPSILON {
        // Degenerate: spread evenly.
        let each = count / weights.len();
        let mut out = vec![each; weights.len()];
        let used: usize = out.iter().sum();
        out[0] += count - used;
        return out;
    }
    let mut out: Vec<usize> = weights
        .iter()
        .map(|w| ((count as f32) * (w.max(0.0) / total)) as usize)
        .collect();
    let used: usize = out.iter().sum();
    if let Some(first) = out.first_mut() {
        *first += count - used;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sums_to_count() {
        let parts = split(1000, &[0.45, 0.55]);
        assert_eq!(parts.iter().sum::<usize>(), 1000);
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn split_handles_zero_weight_terms() {
        let parts = split(100, &[0.0, 0.0]);
        assert_eq!(parts.iter().sum::<usize>(), 100);
    }

    #[test]
    fn split_empty_is_empty() {
        assert!(split(100, &[]).is_empty());
    }
}
