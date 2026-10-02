//! Math for gameplay code.
//!
//! Vector / matrix / quaternion types are re-exported from [`glam`] so game
//! crates get one consistent set of types through a single dependency, while
//! [`Rect`] stays engine-owned: it is the 2D AABB the combat and physics code
//! uses and it is the one shape that has to agree across the sim boundary.

pub use glam::{Mat4, Quat, Vec2, Vec3, Vec4};

/// Axis-aligned bounding box helpers used by the physics / combat code.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.w * 0.5
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.h * 0.5
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// True when the two rectangles overlap (standard AABB test).
    ///
    /// Each axis compares *its own* extents: `self` against `other` and
    /// `other` against `self`. Mixing the two is wrong whenever the rectangles
    /// differ in size, which is the normal case for a narrow hitbox tested
    /// against a wide body.
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// The overlapping region, or `None` when the rectangles are disjoint.
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        if !self.intersects(other) {
            return None;
        }
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Some(Rect::new(
            x,
            y,
            self.right().min(other.right()) - x,
            self.bottom().min(other.bottom()) - y,
        ))
    }

    /// The smallest axis-aligned box containing both rectangles.
    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    /// True when `(px, py)` lies inside the box.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_that_share_area_intersect() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert!(a.intersects(&b));
        assert!(b.intersects(&a));
    }

    #[test]
    fn disjoint_rectangles_do_not_intersect() {
        let a = Rect::new(0.0, 0.0, 1.0, 1.0);
        let b = Rect::new(2.0, 0.0, 1.0, 1.0);
        assert!(!a.intersects(&b));
    }

    #[test]
    fn touching_edges_do_not_count_as_overlap() {
        let a = Rect::new(0.0, 0.0, 1.0, 1.0);
        let b = Rect::new(1.0, 0.0, 1.0, 1.0);
        assert!(!a.intersects(&b));
    }

    #[test]
    fn center_is_midpoint() {
        let r = Rect::new(2.0, 4.0, 6.0, 8.0);
        assert_eq!(r.center_x(), 5.0);
        assert_eq!(r.center_y(), 8.0);
        assert_eq!(r.right(), 8.0);
        assert_eq!(r.bottom(), 12.0);
    }

    /// A narrow box fully inside a wide one overlaps regardless of the two
    /// widths. Reading the wrong extent for the inner edge reports this as
    /// disjoint, which is the case a hitbox-vs-body test hits every frame.
    #[test]
    fn narrow_box_inside_wide_box_intersects() {
        let wide = Rect::new(0.0, 0.0, 40.0, 20.0);
        let narrow = Rect::new(10.0, 5.0, 4.0, 6.0);
        assert!(narrow.intersects(&wide));
        assert!(wide.intersects(&narrow));
    }

    /// A narrow box just past the wide one's right edge is disjoint even
    /// though the wide box is wide enough to reach back over it.
    #[test]
    fn narrow_box_past_a_wide_box_does_not_intersect() {
        let wide = Rect::new(0.0, 0.0, 40.0, 20.0);
        let narrow = Rect::new(40.0, 5.0, 4.0, 6.0);
        assert!(!narrow.intersects(&wide));
        assert!(!wide.intersects(&narrow));
    }

    /// Same test on the vertical axis, which the horizontal cases cannot
    /// reach.
    #[test]
    fn narrow_box_above_a_wide_box_does_not_intersect() {
        let wide = Rect::new(0.0, 0.0, 40.0, 20.0);
        let tall = Rect::new(10.0, 20.0, 6.0, 40.0);
        assert!(!tall.intersects(&wide));
        assert!(!wide.intersects(&tall));
    }

    #[test]
    fn intersection_is_the_shared_region() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersection(&b), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
        assert_eq!(a.intersection(&Rect::new(50.0, 0.0, 1.0, 1.0)), None);
    }

    #[test]
    fn union_covers_both_boxes() {
        let a = Rect::new(0.0, 0.0, 2.0, 3.0);
        let b = Rect::new(10.0, 20.0, 1.0, 1.0);
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 11.0, 21.0));
    }

    #[test]
    fn contains_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(0.0, 0.0));
        assert!(r.contains(9.9, 9.9));
        assert!(!r.contains(10.0, 5.0));
        assert!(!r.contains(-0.1, 5.0));
    }

    #[test]
    fn glam_vectors_are_re_exported() {
        let a = Vec2::new(3.0, 4.0);
        assert_eq!(a.length(), 5.0);
        let m = Mat4::from_scale(Vec3::splat(2.0));
        assert_eq!(m.transform_point3(Vec3::ONE), Vec3::splat(2.0));
        let turned = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) * Vec3::X;
        assert!((turned - Vec3::Y).length() < 1e-5);
        assert_eq!(
            Vec4::new(1.0, 2.0, 3.0, 4.0).truncate(),
            Vec3::new(1.0, 2.0, 3.0)
        );
    }
}
