//! Cloud-band primitive — soft, drifting `Halo` puffs.
//!
//! Overlapping puffs across a top band merge into a continuous mass with a
//! lumpy underside; `Halo` points render with the softest falloff (see
//! `render/shader.wgsl`), which is what reads as cloud rather than confetti.
//! Positions are re-derived from `center + local * scale` every frame, so the
//! band follows any placement.

use glam::Vec3;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use super::TermCtx;
use crate::sim::types::{Layer, Particle};

const SEED: u64 = 0xC10D_5EED;

const PUFFS: usize = 9;
const Y_MIN: f32 = 0.58;
const Y_MAX: f32 = 0.96;
const HALF_WIDTH: f32 = 1.02;
const BRIGHT: f32 = 0.12;

const TAU: f32 = std::f32::consts::TAU;

/// Bell-ish `[-1, 1]` (sum of two uniforms): denser at the center than a flat
/// box, so puffs taper at their edges.
fn bell(rng: &mut SmallRng) -> f32 {
    (rng.gen::<f32>() + rng.gen::<f32>()) - 1.0
}

/// Cloud's baseline `color_bias`: pulled toward neutral relative to the scene's
/// palette role, so clouds read greyer than the rain beneath them.
fn cloud_bias(ctx: &TermCtx) -> f32 {
    (ctx.color_bias * 0.45).clamp(0.18, 0.55)
}

pub fn generate(count: usize, coverage: f32, ctx: &TermCtx, out: &mut Vec<Particle>) {
    let mut rng = SmallRng::seed_from_u64(SEED);
    let coverage = coverage.clamp(0.1, 1.0);
    let half_width = HALF_WIDTH * (0.7 + 0.3 * coverage);
    let bias = cloud_bias(ctx);

    // Overlapping puff anchors across the top band.
    let mut puffs = [Vec3::ZERO; PUFFS];
    for (i, puff) in puffs.iter_mut().enumerate() {
        let f = i as f32 / (PUFFS - 1) as f32;
        let x = (-1.0 + f * 2.0) * half_width + bell(&mut rng) * 0.08;
        let y = Y_MIN + rng.gen::<f32>() * (Y_MAX - Y_MIN);
        let z = bell(&mut rng) * 0.30;
        *puff = Vec3::new(x, y, z);
    }

    for _ in 0..count {
        let puff = puffs[rng.gen_range(0..PUFFS)];
        // Flattened ellipsoid: wide, shallow, soft belly.
        let local = puff
            + Vec3::new(
                bell(&mut rng) * 0.42,
                bell(&mut rng) * 0.15,
                bell(&mut rng) * 0.26,
            );
        let jitter = rng.gen_range(0.6..1.15);
        out.push(Particle {
            position: ctx.center + local * ctx.scale,
            base_offset: local,
            local: Vec3::new(jitter, 0.0, 0.0),
            layer: Layer::Halo,
            shell_offset: rng.gen::<f32>() * TAU,
            size: rng.gen_range(1.8..3.2),
            brightness: BRIGHT * jitter,
            color_bias: bias,
            ..Default::default()
        });
    }
}

pub fn update(particles: &mut [Particle], wind: f32, ctx: &TermCtx) {
    let bias = cloud_bias(ctx);
    for p in particles.iter_mut() {
        let drift = (ctx.time * 0.12 + p.shell_offset).sin() * 0.05 + wind * 0.10;
        let bob = (ctx.time * 0.09 + p.shell_offset * 1.3).sin() * 0.02;
        let local = p.base_offset + Vec3::new(drift, bob, 0.0);
        p.position = ctx.center + local * ctx.scale;
        p.brightness = BRIGHT * p.local.x * ctx.presence;
        p.color_bias = bias;
    }
}
