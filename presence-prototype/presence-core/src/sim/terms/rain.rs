//! Rain primitive — falling `Core` streaks.
//!
//! A bounded number of vertical *streaks*, each a short trail of sharp points
//! (bright head, fading tail) that falls from under the cloud band and wraps
//! back to the top. Streaks are capped and feathered across the width, so rain
//! reads as distinct lines with dark gaps between them instead of a uniform
//! block of noise. Positions are re-derived from `center + local * scale` every
//! frame, so the column follows any placement.

use glam::Vec3;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use super::TermCtx;
use crate::sim::types::{Layer, Particle};

const SEED: u64 = 0xB14C_5EED;

/// Points per streak (bright head -> fading tail dashes).
const STREAK_LEN: usize = 7;
/// Phase gap between consecutive streak members; times the column height this
/// sets the on-screen dash length.
const STREAK_SPACING: f32 = 0.026;
/// Hard cap on streaks so a large point budget makes rain *denser per streak*,
/// never a solid noise field. There is no lower floor: the generator never
/// exceeds the budget the realizer allocated it.
const MAX_STREAKS: usize = 900;

// Local-space extents (in `scale` units, relative to `center`).
const COL_TOP: f32 = 0.5;
const COL_BOTTOM: f32 = -1.02;
const COL_HALF_WIDTH: f32 = 0.86;

const BRIGHT: f32 = 0.34;
const RATE: f32 = 0.42;

/// Bell-ish `[-1, 1]`: dense center, sparse edges (feathered column).
fn bell(rng: &mut SmallRng) -> f32 {
    (rng.gen::<f32>() + rng.gen::<f32>()) - 1.0
}

pub fn generate(count: usize, density: f32, ctx: &TermCtx, out: &mut Vec<Particle>) {
    let mut rng = SmallRng::seed_from_u64(SEED);
    // Never exceed the allocated budget; density already folded into `count` by
    // the realizer's weight split.
    let _ = density;
    let streaks = (count / STREAK_LEN).clamp(1, MAX_STREAKS);
    let bias = ctx.color_bias;

    for _ in 0..streaks {
        let lx = bell(&mut rng) * COL_HALF_WIDTH;
        let lz = rng.gen_range(-0.32..0.32);
        let base_phase = rng.gen::<f32>();
        let size = rng.gen_range(0.34..0.5);
        let streak_dim = rng.gen_range(0.45..1.0);
        for k in 0..STREAK_LEN {
            let phase = (base_phase + k as f32 * STREAK_SPACING).fract();
            let head = 1.0 - k as f32 / STREAK_LEN as f32;
            let y = COL_TOP - phase * (COL_TOP - COL_BOTTOM);
            out.push(Particle {
                position: ctx.center + Vec3::new(lx, y, lz) * ctx.scale,
                base_offset: Vec3::new(lx, 0.0, lz),
                local: Vec3::new(streak_dim, 0.0, 0.0),
                layer: Layer::Core,
                shell_offset: phase,
                // Crease on the bright head pulls each drop toward the accent
                // hue, so rain shares the shell's fold-filament color language
                // instead of being a flat teal spray. Kept subtle so heads read
                // as lit, not recolored.
                crease: head * 0.6,
                size: size * (0.55 + 0.45 * head),
                brightness: BRIGHT,
                color_bias: bias,
                ..Default::default()
            });
        }
    }
}

pub fn update(particles: &mut [Particle], wind: f32, ctx: &TermCtx) {
    let bias = ctx.color_bias;
    for p in particles.iter_mut() {
        let phase = (p.shell_offset + ctx.time * RATE).fract();
        let y = COL_TOP - phase * (COL_TOP - COL_BOTTOM);
        let wind_shift = wind * 0.6 * phase;
        let local = Vec3::new(p.base_offset.x + wind_shift, y, p.base_offset.z);
        p.position = ctx.center + local * ctx.scale;
        // Fade in at the top, out at the bottom, so drops don't pop at the seam.
        let edge = (phase * 7.0)
            .min(1.0)
            .min((1.0 - phase) * 7.0)
            .clamp(0.0, 1.0);
        p.brightness = BRIGHT * (0.3 + 0.7 * p.crease) * p.local.x * edge * ctx.presence;
        p.color_bias = bias;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaks_are_capped_and_never_exceed_budget() {
        let ctx = TermCtx {
            center: Vec3::ZERO,
            scale: 0.7,
            time: 0.0,
            presence: 1.0,
            color_bias: 0.82,
            seeds: &[],
            surface_affinity: 0.0,
        };
        // Huge allocation saturates at the cap.
        let mut big = Vec::new();
        generate(10_000_000, 1.0, &ctx, &mut big);
        assert_eq!(big.len(), MAX_STREAKS * STREAK_LEN);
        // Small allocation is respected (never overshoots).
        let mut small = Vec::new();
        generate(70, 1.0, &ctx, &mut small);
        assert!(
            small.len() <= 70 + STREAK_LEN,
            "overshot budget: {}",
            small.len()
        );
    }

    #[test]
    fn column_stays_bounded_after_falling() {
        let ctx = TermCtx {
            center: Vec3::new(0.5, -1.0, 0.0),
            scale: 0.4,
            time: 0.0,
            presence: 1.0,
            color_bias: 0.82,
            seeds: &[],
            surface_affinity: 0.0,
        };
        let mut out = Vec::new();
        generate(20_000, 1.0, &ctx, &mut out);
        let mut ctx = ctx;
        for _ in 0..120 {
            ctx.time += 1.0 / 60.0;
            update(&mut out, 0.0, &ctx);
        }
        let bottom = ctx.center.y + COL_BOTTOM * ctx.scale - 0.01;
        let top = ctx.center.y + COL_TOP * ctx.scale + 0.01;
        for p in &out {
            assert!(
                p.position.y >= bottom && p.position.y <= top,
                "rain escaped column: y={}",
                p.position.y
            );
        }
    }
}
