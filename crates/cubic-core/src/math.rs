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

    /// True when the two rectangles overlap (standard AABB test).
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
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
    }
}
