//! Common engine components.
//!
//! Only components that are backend- and gameplay-agnostic live here. A
//! component that needs a GPU resource or a game's balance numbers belongs in
//! the game crate, not in the engine.

#[cfg(feature = "rendering")]
use crate::assets::TextureHandle;
use crate::math::{Mat4, Vec2, Vec3};
#[cfg(feature = "rendering")]
use crate::render::{Rgba, TextureFilter, TexturedQuad, UvRect};

/// 2D transform: position, rotation (radians, counter-clockwise), and scale.
///
/// Components compose parent-before-child, so `to_mat4` is `T * parent` and a
/// child offset is expressed in the parent's rotated/scaled space.
#[cfg_attr(
    feature = "reflect",
    derive(crate::reflect::Component, crate::reflect::Inspectable)
)]
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

/// The size a sprite takes when nothing says otherwise: a small, visible quad
/// that a drop or a prefab can grow.
pub const SPRITE_DEFAULT_SIZE: Vec2 = Vec2::new(32.0, 32.0);

/// A textured quad drawn at an entity's [`Transform`].
///
/// The handle names the image; nothing decodes it here, so a sprite is the
/// GPU-free half of what a backend draws. `tint` multiplies the sampled color
/// (white leaves the image as it is), `size` is the quad's extent in world units
/// before the transform's own scale, and `layer` orders the sprite against the
/// others: a higher layer draws on top, and two sprites on one layer draw in
/// entity id order, so the paint order is total and stable.
///
/// A sprite draws a region of its image — the whole of it by default — so one
/// sheet can feed many sprites: `region` slices it, `flip_x`/`flip_y` mirror
/// that slice, and `filter` picks how the slice is sampled. A sprite has no
/// meaning without a texture, so it is deliberately not addable from the
/// inspector's menu: it arrives with a texture, from a drop or a scene.
#[cfg(feature = "rendering")]
#[cfg_attr(
    feature = "reflect",
    derive(crate::reflect::Component, crate::reflect::Inspectable)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    /// The image to sample.
    pub texture: TextureHandle,
    /// The region of the image to sample: a whole sheet or one cell of it.
    ///
    /// Not reflected: slicing a sheet is a sheet-editing job, not something a
    /// property grid should do a number at a time.
    #[cfg_attr(feature = "reflect", inspect(skip))]
    #[cfg_attr(feature = "serde", serde(default))]
    pub region: UvRect,
    /// Multiplied into the sampled color; white leaves the image as it is.
    pub tint: Rgba,
    /// Draw order: a higher layer paints after a lower one.
    pub layer: i32,
    /// The quad's extent in world units, before the transform's scale.
    pub size: Vec2,
    /// Mirror the region across its vertical axis.
    #[cfg_attr(feature = "serde", serde(default))]
    pub flip_x: bool,
    /// Mirror the region across its horizontal axis.
    #[cfg_attr(feature = "serde", serde(default))]
    pub flip_y: bool,
    /// How the region is sampled: smooth when scaled, or hard texel edges.
    #[cfg_attr(feature = "reflect", inspect(select = ["nearest", "linear"]))]
    #[cfg_attr(feature = "serde", serde(default))]
    pub filter: TextureFilter,
}

#[cfg(feature = "rendering")]
impl Sprite {
    /// A sprite of `texture`, untinted, on layer 0, at
    /// [`SPRITE_DEFAULT_SIZE`], drawing the whole image with the default filter.
    pub const fn new(texture: TextureHandle) -> Self {
        Self {
            texture,
            region: UvRect::WHOLE,
            tint: Rgba::new(1.0, 1.0, 1.0, 1.0),
            layer: 0,
            size: SPRITE_DEFAULT_SIZE,
            flip_x: false,
            flip_y: false,
            filter: TextureFilter::Linear,
        }
    }

    /// The same sprite sized to `size` world units.
    pub const fn with_size(mut self, size: Vec2) -> Self {
        self.size = size;
        self
    }

    /// The same sprite drawn on `layer`.
    pub const fn with_layer(mut self, layer: i32) -> Self {
        self.layer = layer;
        self
    }

    /// The same sprite tinted by `tint`.
    pub const fn with_tint(mut self, tint: Rgba) -> Self {
        self.tint = tint;
        self
    }

    /// The same sprite drawing only `region` of its image.
    pub const fn with_region(mut self, region: UvRect) -> Self {
        self.region = region;
        self
    }

    /// The same sprite mirrored horizontally and/or vertically.
    pub const fn with_flip(mut self, flip_x: bool, flip_y: bool) -> Self {
        self.flip_x = flip_x;
        self.flip_y = flip_y;
        self
    }

    /// The same sprite sampled with `filter`.
    pub const fn with_filter(mut self, filter: TextureFilter) -> Self {
        self.filter = filter;
        self
    }

    /// The region to sample, with the sprite's flips applied.
    ///
    /// A flip is a mirrored region rather than extra instance state, so this is
    /// the one place the two come together and the shader only ever sees a
    /// [`UvRect`].
    pub fn uv(&self) -> UvRect {
        self.region.flipped(self.flip_x, self.flip_y)
    }

    /// This sprite as the quad a backend draws: the image and region, the
    /// tint, filter, and a destination rectangle `x, y, w, h` in the caller's
    /// space with a `rotation` about its centre.
    pub fn textured_quad(&self, x: f32, y: f32, w: f32, h: f32, rotation: f32) -> TexturedQuad {
        TexturedQuad {
            texture: self.texture,
            x,
            y,
            w,
            h,
            rotation,
            uv: self.uv(),
            filter: self.filter,
            color: self.tint,
        }
    }
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

#[cfg(all(test, feature = "rendering"))]
mod sprite_tests {
    use std::num::NonZeroU64;

    use super::*;

    fn texture(id: u64) -> TextureHandle {
        TextureHandle::new(NonZeroU64::new(id).expect("a test uses non-zero ids"))
    }

    #[test]
    fn a_new_sprite_is_untinted_at_the_default_size() {
        let sprite = Sprite::new(texture(1));
        assert_eq!(sprite.texture, texture(1));
        assert_eq!(sprite.tint, Rgba::new(1.0, 1.0, 1.0, 1.0), "white tint");
        assert_eq!(sprite.layer, 0);
        assert_eq!(sprite.size, SPRITE_DEFAULT_SIZE);
        assert_eq!(sprite.region, UvRect::WHOLE, "the whole image by default");
        assert!(!sprite.flip_x && !sprite.flip_y, "unflipped by default");
        assert_eq!(sprite.filter, TextureFilter::Linear);
    }

    #[test]
    fn builders_set_one_field_at_a_time() {
        let sprite = Sprite::new(texture(2))
            .with_size(Vec2::new(8.0, 4.0))
            .with_layer(3)
            .with_tint(Rgba::rgb(0.5, 0.25, 0.0))
            .with_region(UvRect::new(0.5, 0.0, 1.0, 0.5))
            .with_flip(true, false)
            .with_filter(TextureFilter::Nearest);
        assert_eq!(
            sprite,
            Sprite {
                texture: texture(2),
                region: UvRect::new(0.5, 0.0, 1.0, 0.5),
                tint: Rgba::rgb(0.5, 0.25, 0.0),
                layer: 3,
                size: Vec2::new(8.0, 4.0),
                flip_x: true,
                flip_y: false,
                filter: TextureFilter::Nearest,
            }
        );
    }

    #[test]
    fn a_sprites_uv_is_its_region_with_its_flips_applied() {
        let region = UvRect::new(0.25, 0.1, 0.75, 0.6);
        let plain = Sprite::new(texture(1)).with_region(region);
        assert_eq!(plain.uv(), region, "no flip leaves the region alone");
        assert_eq!(
            plain.with_flip(true, false).uv(),
            region.flipped(true, false),
            "a horizontal flip mirrors the region"
        );
        assert_eq!(
            plain.with_flip(false, true).uv(),
            region.flipped(false, true),
            "a vertical flip mirrors the region"
        );
        assert_eq!(
            plain.with_flip(true, true).uv(),
            region.flipped(true, true),
            "both flips mirror both ways"
        );
    }

    #[test]
    fn a_sprites_quad_carries_its_region_flip_filter_and_tint() {
        let sprite = Sprite::new(texture(4))
            .with_region(UvRect::new(0.0, 0.0, 0.5, 0.5))
            .with_flip(true, false)
            .with_filter(TextureFilter::Nearest)
            .with_tint(Rgba::rgb(0.1, 0.2, 0.3));

        let quad = sprite.textured_quad(1.0, 2.0, 3.0, 4.0, 0.5);

        assert_eq!(quad.texture, texture(4));
        assert_eq!((quad.x, quad.y, quad.w, quad.h), (1.0, 2.0, 3.0, 4.0));
        assert_eq!(quad.rotation, 0.5);
        assert_eq!(quad.uv, sprite.uv(), "the quad draws the flipped region");
        assert_eq!(quad.filter, TextureFilter::Nearest);
        assert_eq!(quad.color, sprite.tint);
    }

    #[test]
    fn a_sprite_sits_at_its_transforms_aabb() {
        let transform = Transform::from_position(Vec2::new(10.0, 5.0));
        let sprite = Sprite::new(texture(1)).with_size(Vec2::new(8.0, 4.0));
        // The quad is centred on the transform, so the box covers +/- half the
        // sprite's size around the position.
        assert_eq!(
            transform.aabb(sprite.size),
            crate::math::Rect::new(6.0, 3.0, 8.0, 4.0)
        );
    }
}
