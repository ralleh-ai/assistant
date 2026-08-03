//! Scene placement — anchor + offset + scale → `EntityParams::center` / `scale`.

use glam::Vec3;

use crate::render::camera::REST_EYE;

/// Minimum scale multiplier for adaptive scenes (`PRESENCE_ADAPTIVE_SCENES` §6).
pub const SCENE_MIN_SCALE: f32 = 0.25;

/// Maximum offset from an anchor in world units (clamped per resolved extent).
pub const MAX_PLACEMENT_OFFSET: f32 = 2.5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Anchor {
    #[default]
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    /// Offset from the cloud entity's current center.
    CloudRelative,
}

#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub anchor: Anchor,
    pub offset: glam::Vec2,
    pub scale: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            anchor: Anchor::Center,
            offset: glam::Vec2::ZERO,
            scale: 1.0,
        }
    }
}

/// Window / droplet extent passed into placement resolution each frame.
#[derive(Clone, Copy, Debug)]
pub struct ViewportExtent {
    pub width_px: u32,
    pub height_px: u32,
    pub aspect: f32,
}

impl ViewportExtent {
    pub fn from_pixels(width: u32, height: u32) -> Self {
        let aspect = if height > 0 {
            width as f32 / height as f32
        } else {
            1.0
        };
        Self {
            width_px: width,
            height_px: height,
            aspect,
        }
    }
}

/// Visible half-extents at the entity plane (z ≈ 0), matching the default camera.
fn visible_half_extents(aspect: f32) -> (f32, f32) {
    let fovy = 45.0_f32.to_radians();
    let dist = REST_EYE.z.max(0.1);
    let half_h = dist * (fovy / 2.0).tan();
    let half_w = half_h * aspect.max(0.0001);
    (half_w, half_h)
}

impl Placement {
    /// Convenience for callers (debug panel / hotkeys) that only vary the
    /// anchor and scale; offset defaults to zero.
    pub fn anchored(anchor: Anchor, scale: f32) -> Self {
        Self {
            anchor,
            offset: glam::Vec2::ZERO,
            scale,
        }
    }

    pub fn clamped(self) -> Self {
        let scale = self.scale.clamp(SCENE_MIN_SCALE, 1.0);
        let offset = glam::Vec2::new(
            self.offset
                .x
                .clamp(-MAX_PLACEMENT_OFFSET, MAX_PLACEMENT_OFFSET),
            self.offset
                .y
                .clamp(-MAX_PLACEMENT_OFFSET, MAX_PLACEMENT_OFFSET),
        );
        Self {
            anchor: self.anchor,
            offset,
            scale,
        }
    }

    /// Resolve anchor + offset into a world-space center for `EntityParams`.
    pub fn resolve_center(&self, extent: &ViewportExtent, cloud_center: Vec3) -> Vec3 {
        let p = self.clamped();
        let (half_w, half_h) = visible_half_extents(extent.aspect);
        let margin = 0.82;
        let anchor_xy = match p.anchor {
            Anchor::Center => glam::Vec2::ZERO,
            Anchor::TopLeft => glam::Vec2::new(-half_w * margin, half_h * margin),
            Anchor::TopRight => glam::Vec2::new(half_w * margin, half_h * margin),
            Anchor::BottomLeft => glam::Vec2::new(-half_w * margin, -half_h * margin),
            Anchor::BottomRight => glam::Vec2::new(half_w * margin, -half_h * margin),
            Anchor::CloudRelative => glam::Vec2::new(cloud_center.x, cloud_center.y),
        };
        let xy = anchor_xy + p.offset;
        Vec3::new(xy.x, xy.y, cloud_center.z)
    }

    pub fn resolved_scale(self, base_scale: f32) -> f32 {
        base_scale * self.clamped().scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_resolution_moves_with_aspect() {
        let placement = Placement {
            anchor: Anchor::TopRight,
            offset: glam::Vec2::ZERO,
            scale: 1.0,
        };
        let narrow = ViewportExtent::from_pixels(400, 800);
        let wide = ViewportExtent::from_pixels(1600, 800);
        let c_narrow = placement.resolve_center(&narrow, Vec3::ZERO);
        let c_wide = placement.resolve_center(&wide, Vec3::ZERO);
        assert!(
            c_wide.x > c_narrow.x,
            "wider aspect should push top-right further on X"
        );
        assert_eq!(c_narrow.y, c_wide.y);
    }

    #[test]
    fn cloud_relative_follows_cloud_center() {
        let placement = Placement {
            anchor: Anchor::CloudRelative,
            offset: glam::Vec2::new(0.2, -0.1),
            scale: 0.5,
        };
        let extent = ViewportExtent::from_pixels(800, 600);
        let cloud = Vec3::new(0.5, -0.3, 0.0);
        let c = placement.resolve_center(&extent, cloud);
        assert!((c.x - 0.7).abs() < 1e-5);
        assert!((c.y - -0.4).abs() < 1e-5);
    }
}
