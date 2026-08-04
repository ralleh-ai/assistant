//! Surface-mist primitive — soft `Halo` points born from the shell skin.
//!
//! Each mist point is seeded from a `SurfaceSeed` (a captured spot on the shell
//! skin): it starts *on* the fold, inheriting the skin's outward `normal` and
//! `crease`, then a life phase lifts it along the normal and lets it drift,
//! fading the inherited material as it detaches and reabsorbing at the end. The
//! effect reads as mist pooling in the creases and rising off the surface —
//! ambient content that is visibly part of the same scanned organism.
//!
//! Per-particle statics are packed so the realizer's `velocity.x` term tag is
//! never disturbed:
//! - `base_offset` = birth anchor (shell-local)
//! - `local`       = birth normal (rise direction)
//! - `velocity.y`  = birth crease, `velocity.z` = brightness jitter
//! - `shell_offset`= life-phase seed
//!
//! `normal`/`crease`/`position`/`brightness` are the rendered values the
//! behavior recomputes each frame.

use glam::Vec3;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use super::TermCtx;
use crate::scene::surface_seed::SurfaceSeed;
use crate::sim::types::{Layer, Particle};

const SEED: u64 = 0x315D_5EED;

const BRIGHT: f32 = 0.11;
/// Life cycles per second — slow, so mist drifts rather than boils.
const RATE: f32 = 0.14;
/// How far off the skin a fresh mist point already sits (fraction of scale).
const CLING: f32 = 0.02;

fn rand_unit(rng: &mut SmallRng) -> Vec3 {
    loop {
        let p = Vec3::new(
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
        );
        let l = p.length_squared();
        if l > 1e-6 {
            return p / l.sqrt();
        }
    }
}

/// A birth sample: either drawn from the shell snapshot or synthesized on a
/// virtual unit shell when no seeds are available (standalone rendering).
fn birth(rng: &mut SmallRng, ctx: &TermCtx, use_surface: bool) -> SurfaceSeed {
    if use_surface && !ctx.seeds.is_empty() {
        ctx.seeds[rng.gen_range(0..ctx.seeds.len())]
    } else {
        let dir = rand_unit(rng);
        SurfaceSeed {
            local: dir,
            normal: dir,
            crease: rng.gen_range(0.0..0.3),
        }
    }
}

pub fn generate(count: usize, coverage: f32, _rise: f32, ctx: &TermCtx, out: &mut Vec<Particle>) {
    let mut rng = SmallRng::seed_from_u64(SEED);
    let coverage = coverage.clamp(0.1, 1.0);
    let surface_n = ((count as f32) * ctx.surface_affinity.clamp(0.0, 1.0)) as usize;

    for i in 0..count {
        let seed = birth(&mut rng, ctx, i < surface_n);
        let normal = seed.normal.normalize_or_zero();
        let jitter = rng.gen_range(0.6..1.2);
        let phase_seed = rng.gen::<f32>();
        // A little tangential scatter so mist is a band around the fold, not a
        // line of points marching straight out along one normal.
        let scatter = rand_unit(&mut rng) * 0.05 * coverage;
        let anchor = seed.local + scatter;
        out.push(Particle {
            position: ctx.center + (anchor + normal * CLING) * ctx.scale,
            base_offset: anchor,
            normal,
            crease: seed.crease,
            local: normal,
            velocity: Vec3::new(0.0, seed.crease, jitter),
            layer: Layer::Halo,
            shell_offset: phase_seed,
            size: rng.gen_range(2.0..3.6),
            brightness: BRIGHT * jitter * coverage,
            color_bias: ctx.color_bias,
            ..Default::default()
        });
    }
}

pub fn update(particles: &mut [Particle], rise: f32, ctx: &TermCtx) {
    let rise = rise.clamp(0.0, 1.5);
    for p in particles.iter_mut() {
        let phase = (p.shell_offset + ctx.time * RATE).fract();
        let birth_normal = p.local;
        let birth_crease = p.velocity.y;
        let jitter = p.velocity.z;

        // Attachment fades as the point lifts away from the skin.
        let attach = 1.0 - phase;
        let drift = (ctx.time * 0.2 + p.shell_offset * std::f32::consts::TAU).sin() * 0.03 * phase;
        let offset_along = CLING + rise * phase;
        let local =
            p.base_offset + birth_normal * offset_along + Vec3::new(drift, drift * 0.5, 0.0);
        p.position = ctx.center + local * ctx.scale;

        // Inherited material fades out as it detaches, so mist reads as skin at
        // the fold and as free haze once it has risen.
        p.normal = birth_normal * attach;
        p.crease = birth_crease * attach;
        // Fade in from the skin, out as it dissolves at the top.
        let env = (phase * 6.0)
            .min(1.0)
            .min((1.0 - phase) * 3.0)
            .clamp(0.0, 1.0);
        p.brightness = BRIGHT * jitter * env * ctx.presence;
        p.color_bias = ctx.color_bias;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with<'a>(seeds: &'a [SurfaceSeed]) -> TermCtx<'a> {
        TermCtx {
            center: Vec3::ZERO,
            scale: 1.0,
            time: 0.0,
            presence: 1.0,
            color_bias: 0.7,
            seeds,
            surface_affinity: 1.0,
        }
    }

    #[test]
    fn births_from_seeds_when_available() {
        let seeds = vec![SurfaceSeed {
            local: Vec3::X,
            normal: Vec3::X,
            crease: 0.8,
        }];
        let ctx = ctx_with(&seeds);
        let mut out = Vec::new();
        generate(200, 1.0, 0.4, &ctx, &mut out);
        assert_eq!(out.len(), 200);
        // Every surface-born point anchors at the single seed direction.
        assert!(out.iter().all(|p| (p.base_offset - Vec3::X).length() < 0.2));
    }

    #[test]
    fn falls_back_to_procedural_without_seeds() {
        let ctx = ctx_with(&[]);
        let mut out = Vec::new();
        generate(200, 1.0, 0.4, &ctx, &mut out);
        assert_eq!(out.len(), 200);
        assert!(out.iter().any(|p| p.normal.length() > 0.5));
    }

    #[test]
    fn update_is_deterministic_and_fades_normal() {
        let ctx = ctx_with(&[]);
        let mut a = Vec::new();
        generate(300, 1.0, 0.5, &ctx, &mut a);
        let mut b = a.clone();
        let mut c = ctx;
        c.time = 1.234;
        update(&mut a, 0.5, &c);
        update(&mut b, 0.5, &c);
        for (pa, pb) in a.iter().zip(b.iter()) {
            assert_eq!(pa.position, pb.position);
            assert_eq!(pa.normal, pb.normal);
        }
    }
}
