//! The 2D transform gizmo: the handles the viewport draws over the selected
//! entity, and the drag math that turns pointer motion into a transform edit.
//!
//! The gizmo is split so both halves stay testable without a panel or a GPU:
//! [`draw`] appends the active mode's handles to a [`DrawList`] in target
//! pixels, and [`hit`]/[`drag`] are pure functions of the entity's
//! [`Transform`], the camera and the pointer. The viewport panel only routes
//! pointer events between them and [`crate::edit`].
//!
//! # Conventions
//!
//! Screen and world share the viewport's pixel conventions: y grows downward,
//! a world unit is `camera.zoom` pixels, and with no camera rotation the world
//! axes land on the screen axes. Translation is therefore along the *world*
//! axes — the move arrows are always screen-aligned — while the scale handles
//! sit on the entity's rotated box corners and rotation is the pointer's angle
//! about the entity's origin.
//!
//! Only [`DrawCommand::Rect`](cubic_core::render::DrawCommand::Rect) exists in
//! the command model, so the rotation ring and the box edges are approximated
//! by short quads stepped along the shape ([`line`] and [`ring`]).
//!
//! # Snapping
//!
//! [`Snap`] quantizes the value a drag produces, in the world units the drag
//! works in: grid snapping lands positions on the visible grid and rounds
//! rotation and scale to neat steps, while pixel snapping lands positions on
//! whole target pixels at the current zoom.

use cubic_core::components::Transform;
use cubic_core::math::Vec2;
use cubic_core::render::{DrawList, Renderer, Rgba};
use cubic_core::world::EntityId;

use crate::viewport::{MARKER_SIZE, ViewportCamera, grid_step};

/// Which handle set the viewport shows for the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GizmoMode {
    /// Axis-constrained and free translation.
    #[default]
    Move,
    /// Rotation about the entity's origin.
    Rotate,
    /// Scaling from the entity's box corners.
    Scale,
}

impl GizmoMode {
    /// Every mode, in the order the toolbar shows them.
    pub const ALL: [GizmoMode; 3] = [GizmoMode::Move, GizmoMode::Rotate, GizmoMode::Scale];

    pub fn title(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::Rotate => "Rotate",
            Self::Scale => "Scale",
        }
    }
}

/// How a drag quantizes the value it produces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Snap {
    /// Continuous; the drag follows the pointer exactly.
    #[default]
    Off,
    /// Positions land on the visible grid; rotation and scale round to steps.
    Grid,
    /// Positions land on whole target pixels at the current zoom.
    Pixel,
}

impl Snap {
    /// Every choice, in the order the toolbar shows them.
    pub const ALL: [Snap; 3] = [Snap::Off, Snap::Grid, Snap::Pixel];

    pub fn title(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Grid => "Grid",
            Self::Pixel => "Pixel",
        }
    }

    /// The world-unit step positions snap to, or `None` when snapping is off.
    ///
    /// Grid uses the same [`grid_step`] the canvas draws, so a dragged entity
    /// lands on a line the user can see. Pixel uses the world size of one
    /// target pixel, which is as fine as the viewport can show.
    pub fn position_step(self, zoom: f32) -> Option<f32> {
        match self {
            Self::Off => None,
            Self::Grid => Some(grid_step(zoom)),
            Self::Pixel => (zoom.is_finite() && zoom > 0.0).then_some(1.0 / zoom),
        }
    }

    /// The step rotations snap to, under the coarse grid mode only.
    pub fn rotation_step(self) -> Option<f32> {
        (self == Self::Grid).then_some(std::f32::consts::PI / 12.0)
    }

    /// The step scales snap to, under the coarse grid mode only.
    pub fn scale_step(self) -> Option<f32> {
        (self == Self::Grid).then_some(0.1)
    }
}

/// The toolbar's gizmo choices: which handles show, and how they snap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GizmoSettings {
    pub mode: GizmoMode,
    pub snap: Snap,
}

/// A part of the gizmo a pointer can grab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// Free movement, in both world axes.
    Move,
    /// Movement along the world X axis only.
    MoveX,
    /// Movement along the world Y axis only.
    MoveY,
    /// Rotation about the entity's origin.
    Rotate,
    /// Scaling, grabbed at the box corner with the index into [`CORNERS`].
    Scale(usize),
}

/// A drag in progress: the entity, the handle held, and the transform and world
/// point the gesture began from.
///
/// Both starting values are kept because every drag is computed from where it
/// started, never from the previous frame — a snapped drag that accumulates
/// from the last snapped value would walk away from the pointer.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub entity: EntityId,
    pub handle: Handle,
    pub start: Transform,
    pub start_world: Vec2,
}

/// Handle sizes in target pixels, held constant with zoom so a handle stays the
/// same size to grab however far the camera pulls back.
const HANDLE: f32 = 13.0;
/// Extra pointer slack around a handle, so a near-miss still grabs.
const GRAB: f32 = 7.0;
/// Move arrow length, from the entity's origin.
const AXIS: f32 = 60.0;
/// Move arrow shaft thickness.
const SHAFT: f32 = 3.0;
/// Rotation ring radius.
const RING_RADIUS: f32 = 46.0;
/// How far from the ring's radius a pointer still counts as on it.
const RING_BAND: f32 = 10.0;
/// Edge of the little quads that approximate a line or ring.
const STEP: f32 = 4.0;

/// Move arrow colors: the world X and Y axes, plus the neutral origin.
const X_AXIS: Rgba = Rgba::rgb(0.91, 0.31, 0.37);
const Y_AXIS: Rgba = Rgba::rgb(0.42, 0.80, 0.35);
const ORIGIN: Rgba = Rgba::rgb(0.93, 0.94, 0.98);
/// Rotation ring and the scale corners/edges.
const ROTATE: Rgba = Rgba::rgb(0.36, 0.72, 0.95);
const SCALE: Rgba = Rgba::rgb(0.98, 0.79, 0.35);
const EDGE: Rgba = Rgba::rgb(0.52, 0.56, 0.66);

/// The entity's box corners in its local frame, in perimeter order.
///
/// Local y grows downward like the screen, so `(-1, -1)` is the top-left
/// corner; consecutive entries are joined by one edge.
const CORNERS: [(f32, f32); 4] = [(-1.0, -1.0), (-1.0, 1.0), (1.0, 1.0), (1.0, -1.0)];

/// The entity's gizmo frame in target pixels: where it sits, which way its local
/// axes point on screen, and how big its box is after scale and zoom.
struct Frame {
    center: Vec2,
    /// Unit screen direction of the entity's local +X.
    axis_x: Vec2,
    /// Unit screen direction of the entity's local +Y.
    axis_y: Vec2,
    /// Screen half-extents of the entity's box.
    half: Vec2,
}

impl Frame {
    fn of(transform: &Transform, camera: &ViewportCamera, view: Vec2) -> Self {
        let center = camera.to_screen(transform.position, view);
        let (sin, cos) = transform.rotation.sin_cos();
        let zoom = if camera.zoom.is_finite() {
            camera.zoom.abs()
        } else {
            0.0
        };
        let half = MARKER_SIZE * 0.5 * zoom;
        Self {
            center,
            axis_x: Vec2::new(cos, sin),
            axis_y: Vec2::new(-sin, cos),
            half: Vec2::new(
                half * transform.scale.x.abs(),
                half * transform.scale.y.abs(),
            ),
        }
    }

    /// A box corner on screen, for the local signs `(sx, sy)`.
    fn corner(&self, sx: f32, sy: f32) -> Vec2 {
        self.center + self.axis_x * (sx * self.half.x) + self.axis_y * (sy * self.half.y)
    }
}

/// Append the active mode's handles for `transform` to `list`.
///
/// Drawn in the same target pixels as the rest of the scene, so the caller can
/// paint this after [`crate::viewport::build_scene`] to land the gizmo on top.
pub fn draw(
    list: &mut DrawList,
    transform: &Transform,
    mode: GizmoMode,
    camera: &ViewportCamera,
    view: Vec2,
) {
    let frame = Frame::of(transform, camera, view);
    match mode {
        GizmoMode::Move => draw_move(list, &frame),
        GizmoMode::Rotate => draw_rotate(list, &frame),
        GizmoMode::Scale => draw_scale(list, &frame),
    }
}

/// The handle of the active mode's gizmo under `screen`, if any.
///
/// Only the active mode's handles are tested, so switching mode is also what
/// decides which gestures the viewport accepts.
pub fn hit(
    transform: &Transform,
    mode: GizmoMode,
    camera: &ViewportCamera,
    view: Vec2,
    screen: Vec2,
) -> Option<Handle> {
    let frame = Frame::of(transform, camera, view);
    match mode {
        GizmoMode::Move => hit_move(&frame, screen),
        GizmoMode::Rotate => hit_rotate(&frame, screen),
        GizmoMode::Scale => hit_scale(&frame, screen),
    }
}

/// The transform `handle` produces, given where its drag began and where the
/// pointer is now.
///
/// `start_world` and `world` are world points; `zoom` is the camera's scale, for
/// the position snap. The result is a whole [`Transform`] so a caller can write
/// the one field the handle changes without having to merge partial values.
pub fn drag(
    handle: Handle,
    start: &Transform,
    start_world: Vec2,
    world: Vec2,
    snap: Snap,
    zoom: f32,
) -> Transform {
    let mut next = *start;
    match handle {
        Handle::Move | Handle::MoveX | Handle::MoveY => {
            let delta = world - start_world;
            let delta = match handle {
                Handle::MoveX => Vec2::new(delta.x, 0.0),
                Handle::MoveY => Vec2::new(0.0, delta.y),
                _ => delta,
            };
            next.position = snap_vec(start.position + delta, snap.position_step(zoom));
        }
        Handle::Rotate => {
            let before = start_world - start.position;
            let after = world - start.position;
            if before.length_squared() > f32::EPSILON && after.length_squared() > f32::EPSILON {
                let turned = wrap_pi(after.y.atan2(after.x) - before.y.atan2(before.x));
                next.rotation = wrap_pi(snap_scalar(start.rotation + turned, snap.rotation_step()));
            }
        }
        Handle::Scale(index) => {
            let (sx, sy) = CORNERS[index];
            let local = rotate(world - start.position, -start.rotation);
            let base = MARKER_SIZE * 0.5;
            next.scale = snap_vec(
                Vec2::new(local.x / (sx * base), local.y / (sy * base)),
                snap.scale_step(),
            );
        }
    }
    next
}

/// The move gizmo: a shaft and head per world axis, plus a free-move square at
/// the origin.
fn draw_move(list: &mut DrawList, frame: &Frame) {
    let c = frame.center;
    list.fill_rect(c.x, c.y - SHAFT * 0.5, AXIS, SHAFT, X_AXIS);
    square(list, c + Vec2::X * AXIS, HANDLE, X_AXIS);
    list.fill_rect(c.x - SHAFT * 0.5, c.y, SHAFT, AXIS, Y_AXIS);
    square(list, c + Vec2::Y * AXIS, HANDLE, Y_AXIS);
    square(list, c, HANDLE, ORIGIN);
}

/// The rotate gizmo: a ring at a fixed radius, and a hand toward the entity's
/// local +X so the current angle is legible.
fn draw_rotate(list: &mut DrawList, frame: &Frame) {
    ring(list, frame.center, RING_RADIUS, ROTATE);
    line(
        list,
        frame.center,
        frame.center + frame.axis_x * RING_RADIUS,
        ROTATE,
    );
}

/// The scale gizmo: the entity's box, corners joined, with a grab square at each
/// corner.
fn draw_scale(list: &mut DrawList, frame: &Frame) {
    for i in 0..CORNERS.len() {
        let (ax, ay) = CORNERS[i];
        let (bx, by) = CORNERS[(i + 1) % CORNERS.len()];
        line(list, frame.corner(ax, ay), frame.corner(bx, by), EDGE);
    }
    for (sx, sy) in CORNERS {
        square(list, frame.corner(sx, sy), HANDLE, SCALE);
    }
}

fn hit_move(frame: &Frame, p: Vec2) -> Option<Handle> {
    if square_hit(p, frame.center, HANDLE) {
        return Some(Handle::Move);
    }
    if distance_to_segment(p, frame.center, frame.center + Vec2::X * AXIS) <= GRAB {
        return Some(Handle::MoveX);
    }
    if distance_to_segment(p, frame.center, frame.center + Vec2::Y * AXIS) <= GRAB {
        return Some(Handle::MoveY);
    }
    None
}

fn hit_rotate(frame: &Frame, p: Vec2) -> Option<Handle> {
    let distance = (p - frame.center).length();
    ((distance - RING_RADIUS).abs() <= RING_BAND).then_some(Handle::Rotate)
}

fn hit_scale(frame: &Frame, p: Vec2) -> Option<Handle> {
    let mut best: Option<(f32, Handle)> = None;
    for (index, (sx, sy)) in CORNERS.iter().enumerate() {
        let distance = (p - frame.corner(*sx, *sy)).length();
        if distance <= HANDLE * 0.5 + GRAB && best.is_none_or(|(nearest, _)| distance < nearest) {
            best = Some((distance, Handle::Scale(index)));
        }
    }
    best.map(|(_, handle)| handle)
}

/// True when `p` is within `size`'s square centred on `center`, plus the grab
/// slack.
fn square_hit(p: Vec2, center: Vec2, size: f32) -> bool {
    let d = (p - center).abs();
    d.x <= size * 0.5 + GRAB && d.y <= size * 0.5 + GRAB
}

/// The distance from `p` to the segment `a..b`.
fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let length_squared = ab.length_squared();
    if length_squared <= f32::EPSILON {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / length_squared).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// A square of side `size` centred on `center`.
fn square(list: &mut DrawList, center: Vec2, size: f32, color: Rgba) {
    list.fill_rect(
        center.x - size * 0.5,
        center.y - size * 0.5,
        size,
        size,
        color,
    );
}

/// Step small quads along `from..to`, so a line that is not axis-aligned still
/// reads as one.
fn line(list: &mut DrawList, from: Vec2, to: Vec2, color: Rgba) {
    let delta = to - from;
    let length = delta.length();
    if !length.is_finite() || length < f32::EPSILON {
        return;
    }
    let steps = (length / STEP).ceil().max(1.0) as usize;
    for i in 0..=steps {
        let point = from + delta * (i as f32 / steps as f32);
        square(list, point, STEP, color);
    }
}

/// Step small quads around a circle of `radius` centred on `center`.
fn ring(list: &mut DrawList, center: Vec2, radius: f32, color: Rgba) {
    let steps = ((std::f32::consts::TAU * radius / STEP).ceil() as usize).max(12);
    for i in 0..steps {
        let angle = i as f32 / steps as f32 * std::f32::consts::TAU;
        let point = center + Vec2::new(angle.cos(), angle.sin()) * radius;
        square(list, point, STEP, color);
    }
}

/// Rotate `v` by `angle` radians (positive is counter-clockwise in the world's
/// own right-handed sense, which is clockwise on the y-down screen).
fn rotate(v: Vec2, angle: f32) -> Vec2 {
    let (sin, cos) = angle.sin_cos();
    Vec2::new(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
}

/// Wrap `angle` into `(-pi, pi]`, so a delta measured across the ring's seam
/// takes the short way round.
fn wrap_pi(angle: f32) -> f32 {
    let turn = std::f32::consts::TAU;
    let mut wrapped = angle % turn;
    if wrapped > std::f32::consts::PI {
        wrapped -= turn;
    } else if wrapped <= -std::f32::consts::PI {
        wrapped += turn;
    }
    wrapped
}

/// Round `value` to the nearest multiple of `step`, leaving it alone when there
/// is no usable step.
fn snap_scalar(value: f32, step: Option<f32>) -> f32 {
    match step {
        Some(step) if step.is_finite() && step > 0.0 => (value / step).round() * step,
        _ => value,
    }
}

fn snap_vec(value: Vec2, step: Option<f32>) -> Vec2 {
    Vec2::new(snap_scalar(value.x, step), snap_scalar(value.y, step))
}

#[cfg(test)]
mod tests {
    use cubic_core::render::DrawCommand;

    use super::*;

    const VIEW: Vec2 = Vec2::new(400.0, 400.0);

    fn camera() -> ViewportCamera {
        ViewportCamera {
            center: Vec2::ZERO,
            zoom: 1.0,
        }
    }

    /// The screen position of a world point under [`camera`].
    fn screen(world: Vec2) -> Vec2 {
        camera().to_screen(world, VIEW)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn assert_vec(got: Vec2, want: Vec2) {
        assert!(
            close(got.x, want.x) && close(got.y, want.y),
            "got {got:?}, want {want:?}"
        );
    }

    fn rects_of(list: &DrawList, color: Rgba) -> usize {
        list.commands
            .iter()
            .filter(|command| matches!(command, DrawCommand::Rect { color: c, .. } if *c == color))
            .count()
    }

    #[test]
    fn the_frame_sits_on_the_entity_and_follows_its_rotation() {
        let camera = ViewportCamera {
            center: Vec2::ZERO,
            zoom: 2.0,
        };
        let transform = Transform::new(
            Vec2::new(10.0, 5.0),
            std::f32::consts::FRAC_PI_2,
            Vec2::splat(2.0),
        );
        let frame = Frame::of(&transform, &camera, VIEW);

        assert_vec(frame.center, Vec2::new(220.0, 210.0));
        // A quarter turn maps local +X to screen +Y and local +Y to screen -X.
        assert_vec(frame.axis_x, Vec2::new(0.0, 1.0));
        assert_vec(frame.axis_y, Vec2::new(-1.0, 0.0));
        // 16 units * scale 2 * zoom 2 / 2 = 32px half-extent on each axis.
        assert_vec(frame.half, Vec2::splat(32.0));
    }

    #[test]
    fn the_move_origin_and_axes_are_hittable() {
        let transform = Transform::from_position(Vec2::new(10.0, 5.0));
        let center = screen(transform.position);
        let mode = GizmoMode::Move;

        assert_eq!(
            hit(&transform, mode, &camera(), VIEW, center),
            Some(Handle::Move)
        );
        assert_eq!(
            hit(&transform, mode, &camera(), VIEW, center + Vec2::X * 40.0),
            Some(Handle::MoveX)
        );
        assert_eq!(
            hit(&transform, mode, &camera(), VIEW, center + Vec2::Y * 40.0),
            Some(Handle::MoveY)
        );
        assert_eq!(
            hit(
                &transform,
                mode,
                &camera(),
                VIEW,
                center + Vec2::new(80.0, 80.0)
            ),
            None
        );
    }

    #[test]
    fn the_rotate_ring_is_hit_at_its_radius_only() {
        let transform = Transform::from_position(Vec2::new(10.0, 5.0));
        let center = screen(transform.position);
        let mode = GizmoMode::Rotate;

        assert_eq!(
            hit(
                &transform,
                mode,
                &camera(),
                VIEW,
                center + Vec2::new(RING_RADIUS, 0.0)
            ),
            Some(Handle::Rotate)
        );
        assert_eq!(
            hit(&transform, mode, &camera(), VIEW, center),
            None,
            "the origin is free"
        );
    }

    #[test]
    fn each_scale_corner_is_hittable() {
        let transform = Transform::from_position(Vec2::new(10.0, 5.0));
        let frame = Frame::of(&transform, &camera(), VIEW);
        let mode = GizmoMode::Scale;

        for (index, (sx, sy)) in CORNERS.iter().enumerate() {
            let corner = frame.corner(*sx, *sy);
            // The corner sits on the 16-unit box, i.e. 8px from the center.
            assert_vec(corner, frame.center + Vec2::new(sx * 8.0, sy * 8.0));
            assert_eq!(
                hit(&transform, mode, &camera(), VIEW, corner),
                Some(Handle::Scale(index)),
                "corner {index}"
            );
        }
        assert_eq!(
            hit(
                &transform,
                mode,
                &camera(),
                VIEW,
                screen(transform.position) + Vec2::new(100.0, 100.0)
            ),
            None
        );
    }

    #[test]
    fn move_drags_track_the_pointer_and_constrain_per_handle() {
        let start = Transform::from_position(Vec2::new(1.0, 2.0));
        let from = Vec2::new(1.0, 2.0);
        let to = Vec2::new(5.0, 9.0);

        let free = drag(Handle::Move, &start, from, to, Snap::Off, 1.0);
        assert_vec(free.position, Vec2::new(5.0, 9.0));

        let along_x = drag(Handle::MoveX, &start, from, to, Snap::Off, 1.0);
        assert_vec(along_x.position, Vec2::new(5.0, 2.0));

        let along_y = drag(Handle::MoveY, &start, from, to, Snap::Off, 1.0);
        assert_vec(along_y.position, Vec2::new(1.0, 9.0));
    }

    #[test]
    fn grid_snapping_lands_the_position_on_the_visible_grid() {
        let step = grid_step(1.0);
        let start = Transform::from_position(Vec2::ZERO);
        let to = Vec2::new(step * 1.4, -step * 2.6);

        let snapped = drag(Handle::Move, &start, Vec2::ZERO, to, Snap::Grid, 1.0);
        assert_vec(snapped.position, Vec2::new(step, -step * 3.0));
    }

    #[test]
    fn pixel_snapping_lands_the_position_on_whole_pixels() {
        let zoom = 4.0;
        let start = Transform::from_position(Vec2::ZERO);
        let to = Vec2::new(0.3, 0.9);
        let snapped = drag(Handle::Move, &start, Vec2::ZERO, to, Snap::Pixel, zoom);
        // One pixel is 1/4 world unit, so 0.3 -> 0.25 and 0.9 -> 1.0.
        assert_vec(snapped.position, Vec2::new(0.25, 1.0));
    }

    #[test]
    fn rotate_follows_the_pointer_angle_about_the_origin() {
        let start = Transform::from_position(Vec2::ZERO);
        // Start pointing along +X, drag to +Y: a quarter turn.
        let next = drag(Handle::Rotate, &start, Vec2::X, Vec2::Y, Snap::Off, 1.0);
        assert!(close(next.rotation, std::f32::consts::FRAC_PI_2));

        // A drag clockwise (to -Y) turns the other way.
        let back = drag(Handle::Rotate, &start, Vec2::X, -Vec2::Y, Snap::Off, 1.0);
        assert!(close(back.rotation, -std::f32::consts::FRAC_PI_2));
    }

    #[test]
    fn rotate_snaps_to_fifteen_degrees_under_grid_snap() {
        let start = Transform::from_position(Vec2::ZERO);
        let angle: f32 = 0.2; // nearer 15 degrees than 0 or 30
        let to = Vec2::new(angle.cos(), angle.sin());
        let next = drag(Handle::Rotate, &start, Vec2::X, to, Snap::Grid, 1.0);
        assert!(close(next.rotation, std::f32::consts::PI / 12.0));
    }

    #[test]
    fn a_rotation_grab_keeps_its_offset_instead_of_jumping() {
        let start = Transform::from_rotation(1.0);
        // Grab along +X and drag to +Y: the rotation gains a quarter turn from
        // wherever it started, rather than snapping onto the pointer's angle.
        let next = drag(Handle::Rotate, &start, Vec2::X, Vec2::Y, Snap::Off, 1.0);
        assert!(
            close(next.rotation, 1.0 + std::f32::consts::FRAC_PI_2),
            "got {}",
            next.rotation
        );
    }

    #[test]
    fn scale_moves_the_grabbed_corner_onto_the_pointer() {
        let start = Transform::default();
        // Grab the bottom-right corner and pull it to (16, 16): the half-extent
        // of 8 doubles, so the scale doubles.
        let target = Vec2::new(16.0, 16.0);
        let next = drag(Handle::Scale(2), &start, Vec2::ZERO, target, Snap::Off, 1.0);
        assert_vec(next.scale, Vec2::splat(2.0));

        // The top-left corner pulled outward scales the same way, sign and all.
        let next = drag(
            Handle::Scale(0),
            &start,
            Vec2::ZERO,
            -target,
            Snap::Off,
            1.0,
        );
        assert_vec(next.scale, Vec2::splat(2.0));
    }

    #[test]
    fn scale_reads_the_pointer_in_the_entity_frame() {
        // A quarter turn: local +X is world +Y, so the pointer is measured after
        // rotating back out of the entity's orientation.
        let start = Transform::from_rotation(std::f32::consts::FRAC_PI_2);
        let target = Vec2::new(-16.0, 16.0); // world point that is local (16, 16)
        let next = drag(Handle::Scale(2), &start, Vec2::ZERO, target, Snap::Off, 1.0);
        assert_vec(next.scale, Vec2::splat(2.0));
    }

    #[test]
    fn each_mode_draws_its_own_handles() {
        let transform = Transform::from_position(Vec2::new(10.0, 5.0));
        let mut list = DrawList::new();

        draw(&mut list, &transform, GizmoMode::Move, &camera(), VIEW);
        assert_eq!(rects_of(&list, X_AXIS), 2, "shaft and head on X");
        assert_eq!(rects_of(&list, Y_AXIS), 2, "shaft and head on Y");
        assert_eq!(rects_of(&list, ORIGIN), 1, "the free-move origin");

        list.reset();
        draw(&mut list, &transform, GizmoMode::Scale, &camera(), VIEW);
        assert_eq!(rects_of(&list, SCALE), 4, "a corner handle per corner");
        assert!(!rects_of(&list, EDGE).eq(&0), "the box edges are drawn");

        list.reset();
        draw(&mut list, &transform, GizmoMode::Rotate, &camera(), VIEW);
        assert!(
            rects_of(&list, ROTATE) > 40,
            "the ring is a run of segments plus the hand"
        );
    }

    #[test]
    fn snapping_steps_match_the_grid_and_the_pixel() {
        assert_eq!(Snap::Off.position_step(2.0), None);
        assert_eq!(Snap::Grid.position_step(2.0), Some(grid_step(2.0)));
        assert_eq!(Snap::Pixel.position_step(2.0), Some(0.5));
        // A dead zoom has no pixel to snap to.
        assert_eq!(Snap::Pixel.position_step(0.0), None);
        assert_eq!(
            Snap::Grid.rotation_step(),
            Some(std::f32::consts::PI / 12.0)
        );
        assert_eq!(Snap::Pixel.rotation_step(), None);
    }
}
