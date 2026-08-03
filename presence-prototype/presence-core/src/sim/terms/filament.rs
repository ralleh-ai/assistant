//! Crease-filament primitive — bright `Core` points born from high-crease shell
//! spots.
//!
//! Where the shell folds sharply, filaments condense: sparse, bright points
//! that keep a high `crease` (so they pull toward the accent hue exactly like
//! the shell's fold filaments) and lift slightly off the skin along the normal
//! before reabsorbing. This is pure structure/crease language — the opposite of
//! diffuse mist — and it makes ambient content trace the organism's own folds.
//!
//! Per-particle packing mirrors `mist`:
//! - `base_offset` = birth anchor (shell-local)
//! - `local`       = birth normal (lift direction)
//! - `velocity.y`  = birth crease, `velocity.z` = length variance
//! - `shell_offset`= life-phase seed

use glam::Vec3;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use super::TermCtx;
use crate::scene::surface_seed::SurfaceSeed;
use crate::sim::types::{Layer, Particle};

const SEED: u64 = 0xF11A_5EED;

const BRIGHT: f32 = 0.30;
/// Life cycles per second. Faster than mist — filaments flick up and reabsorb.
const RATE: f32 = 0.5;
/// Only seeds at or above this crease qualify as filament roots.
const CREASE_MIN: f32 = 0.3;

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

/// A high-crease birth sample. Prefers the shell's own creased spots; if the
/// snapshot has none (or seeds are unavailable / affinity leaves a procedural
/// remainder), synthesizes a creased spot on a virtual unit shell.
fn birth(rng: &mut SmallRng, roots: &[SurfaceSeed], use_surface: bool) -> SurfaceSeed {
    if use_surface && !roots.is_empty() {
        roots[rng.gen_range(0..roots.len())]
    } else {
        let dir = rand_unit(rng);
        SurfaceSeed {
            local: dir,
            normal: dir,
            crease: rng.gen_range(0.5..1.0),
        }
    }
}

pub fn generate(count: usize, _density: f32, lift: f32, ctx: &TermCtx, out: &mut Vec<Particle>) {
    let mut rng = SmallRng::seed_from_u64(SEED);
    let _ = lift;
    let surface_n = ((count as f32) * ctx.surface_affinity.clamp(0.0, 1.0)) as usize;

    // High-crease roots from the snapshot, if any.
    let roots: Vec<SurfaceSeed> = ctx
        .seeds
        .iter()
        .copied()
        .filter(|s| s.crease >= CREASE_MIN)
        .collect();

    for i in 0..count {
        let seed = birth(&mut rng, &roots, i < surface_n);
        let normal = seed.normal.normalize_or_zero();
        let phase_seed = rng.gen::<f32>();
        let len_var = rng.gen_range(0.6..1.0);
        let crease = seed.crease.max(0.5);
        out.push(Particle {
            position: ctx.center + seed.local * ctx.scale,
            base_offset: seed.local,
            normal,
            crease,
            local: normal,
            velocity: Vec3::new(0.0, crease, len_var),
            layer: Layer::Core,
            shell_offset: phase_seed,
            size: rng.gen_range(0.5..0.85),
            brightness: BRIGHT,
            color_bias: ctx.color_bias,
        });
    }
}

pub fn update(particles: &mut [Particle], lift: f32, ctx: &TermCtx) {
    let lift = lift.clamp(0.0, 1.0);
    for p in particles.iter_mut() {
        let phase = (p.shell_offset + ctx.time * RATE).fract();
        let birth_normal = p.local;
        let birth_crease = p.velocity.y;
        let len_var = p.velocity.z;

        // Rise out along the normal and fall back over the life (a single arc).
        let arc = (phase * std::f32::consts::PI).sin();
        let offset = lift * 0.35 * arc * len_var;
        let local = p.base_offset + birth_normal * offset;
        p.position = ctx.center + local * ctx.scale;

        // Filaments stay attached (grazing helps) but their crease and light
        // pulse with the arc, so they read as flaring up rather than sliding.
        p.normal = birth_normal;
        p.crease = birth_crease * (0.4 + 0.6 * arc);
        p.brightness = BRIGHT * (0.25 + 0.75 * arc) * ctx.presence;
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
    fn prefers_high_crease_roots() {
        let seeds = vec![
            SurfaceSeed {
                local: Vec3::X,
                normal: Vec3::X,
                crease: 0.05,
            },
            SurfaceSeed {
                local: Vec3::Y,
                normal: Vec3::Y,
                crease: 0.9,
            },
        ];
        let ctx = ctx_with(&seeds);
        let mut out = Vec::new();
        generate(200, 1.0, 0.3, &ctx, &mut out);
        // All surface-born filaments root at the high-crease seed (Y), not the
        // low-crease one (X).
        assert!(out
            .iter()
            .all(|p| (p.base_offset - Vec3::Y).length() < 1e-3));
    }

    #[test]
    fn falls_back_when_no_creased_roots() {
        let seeds = vec![SurfaceSeed {
            local: Vec3::X,
            normal: Vec3::X,
            crease: 0.0,
        }];
        let ctx = ctx_with(&seeds);
        let mut out = Vec::new();
        generate(150, 1.0, 0.3, &ctx, &mut out);
        assert_eq!(out.len(), 150);
        assert!(out.iter().all(|p| p.crease >= 0.5));
    }

    #[test]
    fn update_is_deterministic() {
        let ctx = ctx_with(&[]);
        let mut a = Vec::new();
        generate(200, 1.0, 0.4, &ctx, &mut a);
        let mut b = a.clone();
        let mut c = ctx;
        c.time = 0.77;
        update(&mut a, 0.4, &c);
        update(&mut b, 0.4, &c);
        for (pa, pb) in a.iter().zip(b.iter()) {
            assert_eq!(pa.position, pb.position);
            assert_eq!(pa.crease, pb.crease);
        }
    }
}
