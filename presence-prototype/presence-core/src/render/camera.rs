use glam::{Mat4, Vec3};

use crate::palette::PresencePalette;

/// Rest position of the camera. The animated eye orbits within a few
/// centimetres of this — enough to give the volume parallax without ever
/// reading as a camera move.
const REST_EYE: Vec3 = Vec3::new(0.0, 0.3, 5.2);
const ORBIT_RADIUS: f32 = 0.16;
const ORBIT_SPEED: f32 = 0.055;
/// Breathing on the field of view rather than on the eye distance: it reads
/// as the volume inhaling instead of the camera dollying.
const FOV_BREATH_DEGREES: f32 = 0.5;
const FOV_BREATH_SPEED: f32 = 0.13;

pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub world_up: Vec3,
    pub fovy_radians: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
    base_fovy_radians: f32,
    time: f32,
}

impl Camera {
    pub fn new(aspect: f32) -> Self {
        let fovy = 45.0_f32.to_radians();
        Self {
            eye: REST_EYE,
            target: Vec3::ZERO,
            world_up: Vec3::Y,
            fovy_radians: fovy,
            aspect,
            near: 0.1,
            far: 100.0,
            base_fovy_radians: fovy,
            time: 0.0,
        }
    }

    pub fn set_aspect(&mut self, aspect: f32) {
        self.aspect = aspect.max(0.0001);
    }

    /// Very slow Lissajous orbit plus a fovy breath. Both are deliberately
    /// below the threshold where a viewer would call it "camera movement" —
    /// the point is only that a static point cloud stops looking like a
    /// still image, per §2.1's "scanned volume" framing.
    pub fn animate(&mut self, dt: f32) {
        self.time += dt;
        let a = self.time * ORBIT_SPEED;
        self.eye = REST_EYE
            + Vec3::new(
                (a * std::f32::consts::TAU).sin() * ORBIT_RADIUS,
                (a * std::f32::consts::TAU * 0.61).sin() * ORBIT_RADIUS * 0.45,
                0.0,
            );
        let breath = (self.time * FOV_BREATH_SPEED * std::f32::consts::TAU).sin();
        self.fovy_radians = self.base_fovy_radians + breath * FOV_BREATH_DEGREES.to_radians();
    }

    fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let forward = (self.target - self.eye).normalize_or_zero();
        let right = forward.cross(self.world_up).normalize_or_zero();
        let up = right.cross(forward);
        (forward, right, up)
    }

    pub fn uniform(
        &self,
        viewport_height: f32,
        material: &PointMaterial,
        palette: &PresencePalette,
    ) -> CameraUniform {
        let (forward, right, up) = self.basis();
        let view = Mat4::look_at_rh(self.eye, self.target, self.world_up);
        let proj = Mat4::perspective_rh(self.fovy_radians, self.aspect, self.near, self.far);
        let view_proj = proj * view;

        // Three colour axes, kept separate on purpose — §3.1's palette table
        // and §3.2's form vocabulary disagree about what "teal" means unless
        // they are:
        //
        //   state    (`color_bias`, from `EntityParams::cool`) — calm sits at
        //            the near-neutral stop with a faint hue undertone; heavy
        //            compute shifts cooler.
        //   density  (per-fragment energy) — pushes toward the hot stop at the
        //            densest points, in *any* state.
        //   structure (`crease`) — fold filaments pull toward `accent`, which
        //            is what draws surface structure rather than mass.
        //
        // Conflating state with density makes idle render in the thinking
        // state's colour, which both misreads the spec and leaves the cool
        // shift with nowhere to go once the remaining states arrive.
        let calm = palette.calm_tint(material.calm_undertone);

        // Pixels a one-world-unit object covers at one unit of distance. The
        // shader divides by distance to get apparent size, which is what makes
        // the sub-pixel clamp expressible in pixels rather than guesswork.
        let pixels_per_world_unit = viewport_height / (2.0 * (self.fovy_radians * 0.5).tan());

        // Haze is anchored to the camera's distance from its target so it keeps
        // framing the volume identically as the camera drifts.
        let focus_distance = (self.target - self.eye).length();

        CameraUniform {
            view_proj: view_proj.to_cols_array_2d(),
            right: [right.x, right.y, right.z, 0.0],
            up: [up.x, up.y, up.z, 0.0],
            eye: [self.eye.x, self.eye.y, self.eye.z, pixels_per_world_unit],
            forward: [forward.x, forward.y, forward.z, 0.0],
            tint_calm: [calm[0], calm[1], calm[2], 1.0],
            tint_cool: [palette.cool[0], palette.cool[1], palette.cool[2], 1.0],
            tint_hot: [palette.hot[0], palette.hot[1], palette.hot[2], 1.0],
            tint_accent: [palette.accent[0], palette.accent[1], palette.accent[2], 1.0],
            depth_params: [
                focus_distance - material.haze_near_offset,
                focus_distance + material.haze_far_offset,
                material.haze_strength,
                material.min_radius_pixels,
            ],
            material: [
                material.point_scale,
                material.tint_energy_scale,
                material.grazing_boost,
                material.crease_boost,
            ],
        }
    }
}

/// Point material tunables — `docs/PRESENCE_VISUAL_ENTITY.md` §3.1's material
/// rules ("soft falloff, size attenuation, optional very light glow") and §9's
/// requirement that these be configurable rather than shader literals.
#[derive(Clone, Copy, Debug)]
pub struct PointMaterial {
    /// World-space half-size of a point of `size == 1.0`.
    pub point_scale: f32,
    /// How far in front of the focus distance the depth haze begins.
    pub haze_near_offset: f32,
    /// How far behind the focus distance the haze reaches full strength.
    pub haze_far_offset: f32,
    /// Maximum fraction of brightness the haze may remove.
    pub haze_strength: f32,
    /// Floor on apparent point radius, in pixels, to stop sub-pixel shimmer.
    pub min_radius_pixels: f32,
    /// Energy at which a point reaches the hot end of the tint ramp. Lower
    /// values make the cloud read lighter and less saturated overall.
    pub tint_energy_scale: f32,
    /// How far the calm stop is pulled toward the palette's signature hue —
    /// §3.1's "faint undertone". At 0 the entity is neutral white and reads as
    /// unbranded; at 1 idle is fully saturated and becomes indistinguishable
    /// from the compute states.
    pub calm_undertone: f32,
    /// Extra brightness at grazing angles, where the surface turns away from
    /// the camera. This is what gives a point *surface* a readable silhouette;
    /// without it a scanned skin has no edge and reads as a flat spray.
    pub grazing_boost: f32,
    /// Extra brightness along fold creases. Draws the structure lines that
    /// make the surface legible as a folded thing rather than a blob.
    pub crease_boost: f32,
}

impl Default for PointMaterial {
    fn default() -> Self {
        Self {
            // Small and crisp. The references read as individual scan returns,
            // which needs points near the pixel floor; at the old 0.032 they
            // were broad discs that merged into fog long before the surface
            // could show any structure.
            point_scale: 0.011,
            haze_near_offset: 1.1,
            haze_far_offset: 2.4,
            haze_strength: 0.72,
            min_radius_pixels: 0.65,
            // Set so a lone point sits low on the ramp and keeps its hue, and
            // only creases and accumulation reach the neutral end. The previous
            // 0.3 was calibrated against the volumetric brightness budget; left
            // there, every point on the surface saturated the ramp immediately
            // and the entity rendered grey regardless of palette.
            tint_energy_scale: 1.1,
            calm_undertone: 0.62,
            // A closed shell already concentrates screen-space density at its
            // limb by foreshortening, so this compounds with an existing several-
            // fold advantage. Pushed higher the rim becomes the only thing
            // visible and the entity reads as an empty bubble.
            grazing_boost: 0.9,
            crease_boost: 3.0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    pub view_proj: [[f32; 4]; 4],
    pub right: [f32; 4],
    pub up: [f32; 4],
    pub eye: [f32; 4],
    pub forward: [f32; 4],
    pub tint_calm: [f32; 4],
    pub tint_cool: [f32; 4],
    pub tint_hot: [f32; 4],
    pub tint_accent: [f32; 4],
    pub depth_params: [f32; 4],
    pub material: [f32; 4],
}
