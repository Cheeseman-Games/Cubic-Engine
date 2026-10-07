//! Common engine components.
//!
//! Only components that are backend- and gameplay-agnostic live here. A
//! component that needs a GPU resource or a game's balance numbers belongs in
//! the game crate, not in the engine.

use crate::math::{Mat4, Vec2, Vec3};

/// 2D transform: position, rotation (radians, counter-clockwise), and scale.
///
/// Components compose parent-before-child, so `to_mat4` is `T * parent` and a
/// child offset is expressed in the parent's rotated/scaled space.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: Vec2,
    pub rotation: f32,
    pub scale: Vec2,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec2::ZERO,
            rotation: 0.0,
            scale: Vec2::ONE,
        }
    }
}

impl Transform {
    pub fn new(position: Vec2, rotation: f32, scale: Vec2) -> Self {
        Self {
            position,
            rotation,
            scale,
        }
    }

    pub fn from_position(position: Vec2) -> Self {
        Self {
            position,
            ..Default::default()
        }
    }

    pub fn from_rotation(rotation: f32) -> Self {
        Self {
            rotation,
            ..Default::default()
        }
    }

    pub fn with_position(mut self, position: Vec2) -> Self {
        self.position = position;
        self
    }

    pub fn with_rotation(mut self, rotation: f32) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_scale(mut self, scale: Vec2) -> Self {
        self.scale = scale;
        self
    }

    pub fn translation(&self) -> Vec3 {
        Vec3::new(self.position.x, self.position.y, 0.0)
    }

    /// The equivalent 4x4 matrix, laid out for the 2D renderer's
    /// z-is-zero convention: scale, then rotate, then translate.
    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::new(self.scale.x, self.scale.y, 1.0),
            crate::math::Quat::from_rotation_z(self.rotation),
            self.translation(),
        )
    }

    /// The world matrix for a child nested under `parent`.
    ///
    /// Matrix products apply right-to-left, so the parent matrix goes on the
    /// left: the child is placed in the parent's space, then the parent is
    /// moved into the world.
    pub fn to_mat4_in(&self, parent: &Transform) -> Mat4 {
        parent.to_mat4() * self.to_mat4()
    }

    /// Transform a point by this transform alone.
    pub fn apply(&self, point: Vec2) -> Vec2 {
        let scaled = Vec2::new(point.x * self.scale.x, point.y * self.scale.y);
        let (sin, cos) = self.rotation.sin_cos();
        Vec2::new(
            scaled.x * cos - scaled.y * sin,
            scaled.x * sin + scaled.y * cos,
        ) + self.position
    }

    /// The axis-aligned box covering this transform applied to a unit quad
    /// centered on the origin. Useful for broad-phase culling, where an
    /// exact rotated box is more work than the frame budget allows.
    pub fn aabb(&self, size: Vec2) -> crate::math::Rect {
        let (sin, cos) = self.rotation.sin_cos();
        let half = Vec2::new(size.x * 0.5, size.y * 0.5) * self.scale;
        let extent = Vec2::new(
            half.x.abs() * cos.abs() + half.y.abs() * sin.abs(),
            half.x.abs() * sin.abs() + half.y.abs() * cos.abs(),
        );
        crate::math::Rect::new(
            self.position.x - extent.x,
            self.position.y - extent.y,
            extent.x * 2.0,
            extent.y * 2.0,
        )
    }
}

/// Converts a point to homogeneous clip space, for callers that already hold a
/// `Mat4` and only need the 3-component result.
pub fn transform_point(matrix: &Mat4, point: Vec3) -> Vec3 {
    matrix.transform_point3(point)
}

/// Converts a direction (no translation) to clip space.
pub fn transform_direction(matrix: &Mat4, direction: Vec3) -> Vec3 {
    matrix.transform_vector3(direction)
}

/// Projects a clip-space position to normalized device coordinates (`-1..=1`).
pub fn project_ndc(matrix: &Mat4, point: Vec3) -> Vec3 {
    transform_point(matrix, point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Rect;

    const EPS: f32 = 1e-5;

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < EPS
    }

    #[test]
    fn default_transform_is_identity_like() {
        let t = Transform::default();
        assert_eq!(t.position, Vec2::ZERO);
        assert_eq!(t.rotation, 0.0);
        assert_eq!(t.scale, Vec2::ONE);
    }

    #[test]
    fn transform_from_position_sets_identity_scale_and_rotation() {
        let t = Transform::from_position(Vec2::new(10.0, 5.0));
        assert_eq!(t.position.x, 10.0);
        assert_eq!(t.position.y, 5.0);
        assert_eq!(t.rotation, 0.0);
        assert_eq!(t.scale, Vec2::ONE);
    }

    #[test]
    fn to_mat4_places_the_origin_at_the_position() {
        let t = Transform::from_position(Vec2::new(10.0, 5.0));
        let moved = t.to_mat4().transform_point3(Vec3::ZERO);
        assert!(close(moved.truncate(), Vec2::new(10.0, 5.0)));
    }

    #[test]
    fn to_mat4_rotates_before_translating() {
        let t = Transform::new(Vec2::new(10.0, 0.0), std::f32::consts::FRAC_PI_2, Vec2::ONE);
        let turned = t.to_mat4().transform_point3(Vec3::X);
        // A quarter turn about the origin sends +X to +Y, then translation
        // moves it to (10, 1).
        assert!(close(turned.truncate(), Vec2::new(10.0, 1.0)));
    }

    #[test]
    fn to_mat4_scales_before_rotating_and_translating() {
        let t = Transform::new(Vec2::new(1.0, 2.0), 0.0, Vec2::new(3.0, 4.0));
        let moved = t.to_mat4().transform_point3(Vec3::new(1.0, 1.0, 0.0));
        assert!(close(moved.truncate(), Vec2::new(4.0, 6.0)));
    }

    #[test]
    fn default_to_mat4_is_the_identity() {
        assert_eq!(Transform::default().to_mat4(), Mat4::IDENTITY);
    }

    #[test]
    fn apply_matches_to_mat4() {
        for transform in [
            Transform::from_position(Vec2::new(3.0, -4.0)),
            Transform::new(Vec2::ZERO, std::f32::consts::FRAC_PI_2, Vec2::ONE),
            Transform::new(Vec2::new(1.0, 2.0), 0.7, Vec2::new(3.0, 0.5)),
        ] {
            for point in [Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::new(-2.0, 5.0)] {
                let by_hand = transform.apply(point);
                let by_matrix = transform
                    .to_mat4()
                    .transform_point3(point.extend(0.0))
                    .truncate();
                assert!(
                    close(by_hand, by_matrix),
                    "apply/matrix disagree for {transform:?} at {point:?}"
                );
            }
        }
    }

    #[test]
    fn child_offset_is_measured_in_parent_space() {
        let parent = Transform::new(Vec2::new(10.0, 0.0), std::f32::consts::FRAC_PI_2, Vec2::ONE);
        let child = Transform::from_position(Vec2::new(2.0, 0.0));
        // +X in the parent's space is +Y in world space, plus the parent's
        // own offset.
        let world = child.to_mat4_in(&parent).transform_point3(Vec3::ZERO);
        assert!(close(world.truncate(), Vec2::new(10.0, 2.0)));
    }

    #[test]
    fn parent_scale_multiplies_child_offset() {
        let parent = Transform::new(Vec2::new(5.0, 5.0), 0.0, Vec2::splat(3.0));
        let child = Transform::from_position(Vec2::new(1.0, 1.0));
        let world = child.to_mat4_in(&parent).transform_point3(Vec3::ZERO);
        assert!(close(world.truncate(), Vec2::new(8.0, 8.0)));
    }

    #[test]
    fn aabb_of_an_unrotated_unit_quad_is_its_size() {
        let t = Transform::from_position(Vec2::new(10.0, 20.0));
        let bounds = t.aabb(Vec2::new(4.0, 6.0));
        assert_eq!(bounds, Rect::new(8.0, 17.0, 4.0, 6.0));
    }

    #[test]
    fn aabb_grows_to_cover_the_rotated_corners() {
        let t = Transform::from_rotation(std::f32::consts::FRAC_PI_4);
        let bounds = t.aabb(Vec2::new(2.0, 2.0));
        // A 2x2 square turned 45 degrees spans 2*sqrt(2) on both axes.
        let side = 2.0f32 * std::f32::consts::SQRT_2;
        assert!((bounds.w - side).abs() < EPS, "width {}", bounds.w);
        assert!((bounds.h - side).abs() < EPS, "height {}", bounds.h);
    }

    #[test]
    fn every_transformed_corner_lies_inside_the_aabb() {
        let t = Transform::new(Vec2::new(3.0, 4.0), 0.6, Vec2::new(2.0, 0.5));
        let size = Vec2::new(8.0, 2.0);
        let bounds = t.aabb(size);
        for corner in [
            Vec2::new(-4.0, -1.0),
            Vec2::new(4.0, -1.0),
            Vec2::new(-4.0, 1.0),
            Vec2::new(4.0, 1.0),
        ] {
            let world = t.apply(corner);
            // Corners land exactly on the boundary, and `Rect::contains` is
            // half-open, so compare against the box grown by a rounding
            // tolerance rather than testing containment outright.
            let slack = Rect::new(
                bounds.x - EPS,
                bounds.y - EPS,
                bounds.w + EPS * 2.0,
                bounds.h + EPS * 2.0,
            );
            assert!(
                slack.contains(world.x, world.y),
                "{world:?} escaped {bounds:?}"
            );
        }
    }

    #[test]
    fn builders_set_one_field_at_a_time() {
        let t = Transform::default()
            .with_position(Vec2::new(1.0, 2.0))
            .with_rotation(0.5)
            .with_scale(Vec2::splat(2.0));
        assert_eq!(
            t,
            Transform::new(Vec2::new(1.0, 2.0), 0.5, Vec2::splat(2.0))
        );
        assert_eq!(Transform::from_rotation(0.25).rotation, 0.25);
    }

    #[test]
    fn transform_point_ignores_translation_for_directions() {
        let m = Transform::new(Vec2::new(100.0, 100.0), 0.0, Vec2::splat(2.0)).to_mat4();
        assert_eq!(transform_point(&m, Vec3::ONE), Vec3::new(102.0, 102.0, 1.0));
        // A direction is scaled but not moved: x and y double while z, which
        // the transform never scales, stays at 1.
        assert_eq!(transform_direction(&m, Vec3::ONE), Vec3::new(2.0, 2.0, 1.0));
    }

    #[test]
    fn project_ndc_leaves_points_already_in_clip_space_alone() {
        assert_eq!(
            project_ndc(&Mat4::IDENTITY, Vec3::new(0.5, -0.5, 0.0)),
            Vec3::new(0.5, -0.5, 0.0)
        );
    }
}
