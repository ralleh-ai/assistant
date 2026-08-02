//! Noise & motion engine — `docs/PRESENCE_VISUAL_ENTITY.md` §6.
//!
//! Simplex noise for base drift/breathing, curl noise for the divergence-
//! free swirl used by higher-intensity states. Curl noise is derived from
//! a 3-component vector potential (three independently-seeded Simplex
//! fields), not from a single scalar field — a single field can't produce
//! a divergence-free curl directly.

use glam::Vec3;
use noise::{NoiseFn, Simplex};

pub struct NoiseField {
    potential_x: Simplex,
    potential_y: Simplex,
    potential_z: Simplex,
}

impl NoiseField {
    pub fn new(seed: u32) -> Self {
        Self {
            potential_x: Simplex::new(seed),
            potential_y: Simplex::new(seed.wrapping_add(1_013)),
            potential_z: Simplex::new(seed.wrapping_add(7_919)),
        }
    }

    /// Plain scalar Simplex sample in [-1, 1], with `w` as a time-like 4th
    /// dimension so the field itself drifts smoothly over time without
    /// needing to offset `p` (which would also change spatial frequency).
    pub fn sample(&self, p: Vec3, w: f32) -> f32 {
        self.potential_x
            .get([p.x as f64, p.y as f64, p.z as f64, w as f64]) as f32
    }

    fn potential(&self, p: Vec3, w: f32) -> Vec3 {
        let pd = [p.x as f64, p.y as f64, p.z as f64, w as f64];
        Vec3::new(
            self.potential_x.get(pd) as f32,
            self.potential_y.get(pd) as f32,
            self.potential_z.get(pd) as f32,
        )
    }

    /// A smooth 3-D drift vector in roughly [-1, 1] per axis — the three
    /// independently-seeded potential fields read as a vector rather than
    /// curled. Three evaluations instead of `curl`'s eighteen, which is why
    /// low-swirl states (`idle`, per §6's "almost no curl") use this for
    /// their wander instead of paying for a curl they scale to nothing.
    ///
    /// Note this is deliberately *not* divergence-free: for low-amplitude
    /// wander against a spring anchor that doesn't matter, and the spring
    /// is what bounds the motion.
    ///
    /// Currently unused. `SurfaceBehavior` deliberately has no wander term —
    /// a surface is already moving under its points — but §6 lists this as part
    /// of the motion vocabulary and the states still to come will want it.
    #[allow(dead_code)]
    pub fn drift(&self, p: Vec3, w: f32) -> Vec3 {
        self.potential(p, w)
    }

    /// Curl of the vector potential at `p`, evaluated via central finite
    /// differences. Produces natural, source/sink-free vortices — the
    /// technique called out in `docs/PRESENCE_VISUAL_ENTITY.md` §6.
    ///
    /// Eighteen evaluations per call, which is why it is gated behind a swirl
    /// threshold rather than always paid for. Currently unused: §6's usage table
    /// gives idle "almost no curl", and `thinking` — the state whose "highest
    /// internal complexity" this exists for — is a later phase.
    #[allow(dead_code)]
    pub fn curl(&self, p: Vec3, w: f32, eps: f32) -> Vec3 {
        let dx = Vec3::new(eps, 0.0, 0.0);
        let dy = Vec3::new(0.0, eps, 0.0);
        let dz = Vec3::new(0.0, 0.0, eps);

        let py1 = self.potential(p + dy, w);
        let py0 = self.potential(p - dy, w);
        let pz1 = self.potential(p + dz, w);
        let pz0 = self.potential(p - dz, w);
        let px1 = self.potential(p + dx, w);
        let px0 = self.potential(p - dx, w);

        let inv_2eps = 0.5 / eps;
        let curl_x = (py1.z - py0.z) * inv_2eps - (pz1.y - pz0.y) * inv_2eps;
        let curl_y = (pz1.x - pz0.x) * inv_2eps - (px1.z - px0.z) * inv_2eps;
        let curl_z = (px1.y - px0.y) * inv_2eps - (py1.x - py0.x) * inv_2eps;
        Vec3::new(curl_x, curl_y, curl_z)
    }

    /// Ridged multifractal in `[0, 1]`: `(1 - |simplex|)²` summed over
    /// octaves and normalized. Where plain FBM is smooth everywhere, this has
    /// sharp creases along the zero-crossings of the underlying field, which
    /// is exactly the fold structure a displaced surface needs — see
    /// `crate::sim::shapes::PresenceShell` for why the same value doubles as the
    /// crease brightness rather than being computed separately.
    ///
    /// One evaluation per octave, so the cost is the octave count. Squaring
    /// each term is what sharpens the ridge; without it the creases are as
    /// soft as ordinary FBM and the surface has no visible structure.
    pub fn ridged(&self, p: Vec3, w: f32, octaves: u32) -> f32 {
        let mut amplitude = 1.0;
        let mut frequency = 1.0;
        let mut sum = 0.0;
        let mut normalization = 0.0;
        for _ in 0..octaves.max(1) {
            let ridge = 1.0 - self.sample(p * frequency, w).abs();
            sum += ridge * ridge * amplitude;
            normalization += amplitude;
            amplitude *= 0.5;
            frequency *= 2.0;
        }
        (sum / normalization.max(1e-6)).clamp(0.0, 1.0)
    }

    /// Fractal Brownian Motion — a few octaves of the scalar field summed
    /// at increasing frequency/decreasing amplitude, for extra multi-scale
    /// detail under higher intensity (§6's "FBM" driver). Not yet used by
    /// Idle/Loading (Phase 1 scope); reserved for `thinking`'s "highest
    /// internal complexity" signature in a later phase.
    #[allow(dead_code)]
    pub fn fbm(&self, p: Vec3, w: f32, octaves: u32) -> f32 {
        let mut amplitude = 0.5;
        let mut frequency = 1.0;
        let mut sum = 0.0;
        for _ in 0..octaves {
            sum += self.sample(p * frequency, w) * amplitude;
            amplitude *= 0.5;
            frequency *= 2.0;
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_is_deterministic_and_bounded() {
        let field = NoiseField::new(42);
        let p = Vec3::new(1.2, -0.4, 3.1);
        let a = field.sample(p, 0.5);
        let b = field.sample(p, 0.5);
        assert_eq!(a, b, "same input must give same output");
        assert!(
            (-1.5..=1.5).contains(&a),
            "simplex sample out of expected range: {a}"
        );
    }

    #[test]
    fn curl_is_deterministic_and_finite() {
        let field = NoiseField::new(7);
        let p = Vec3::new(0.3, 0.6, -0.2);
        let a = field.curl(p, 1.0, 0.25);
        let b = field.curl(p, 1.0, 0.25);
        assert_eq!(a, b);
        assert!(a.is_finite(), "curl produced a non-finite vector: {a:?}");
    }

    #[test]
    fn ridged_is_bounded_and_peaks_at_zero_crossings() {
        let field = NoiseField::new(11);
        let mut saw_high = false;
        for i in 0..400 {
            let t = i as f32 * 0.05;
            let v = field.ridged(Vec3::new(t, t * 0.7, -t * 0.3), 0.0, 3);
            assert!(
                (0.0..=1.0).contains(&v),
                "ridged out of [0,1] at t={t}: {v}"
            );
            saw_high |= v > 0.85;
        }
        // A ridged field with no near-1.0 values has no creases to draw, which
        // would silently flatten the surface rather than fail loudly.
        assert!(saw_high, "ridged noise never approached a ridge");
    }

    #[test]
    fn different_seeds_diverge() {
        let a = NoiseField::new(1);
        let b = NoiseField::new(2);
        let p = Vec3::new(0.1, 0.2, 0.3);
        assert_ne!(a.sample(p, 0.0), b.sample(p, 0.0));
    }
}
