//! The viewport's engine side: the offscreen target, the camera that frames
//! the scene, and the per-frame command list.
//!
//! The editor never owns a GPU — eframe does. Each frame the app hands this
//! module eframe's render state ([`ViewportHost::set_render_state`]), the
//! viewport panel sizes the target to its rect ([`ViewportHost::prepare`]) and
//! asks for the open scene to be drawn into it ([`ViewportHost::draw_scene`]),
//! and paints the result as an egui image. Camera input (pan/zoom) lives with
//! the panel, since it is pointer plumbing; the world/screen math it drives
//! lives here with [`ViewportCamera`].
//!
//! Screen coordinates throughout are the offscreen target's pixels, y
//! downward — the 2D pipeline's own convention — so nothing here re-derives an
//! orientation the renderer already fixed.

use cubic_core::components::Transform;
use cubic_core::math::Vec2;
use cubic_core::render::{DrawList, Renderer, Rgba};
use cubic_core::world::World;
use cubic_render::offscreen::Offscreen2d;
use cubic_render::text::TextError;
use eframe::egui_wgpu::RenderState;

/// Backdrop: what the panel shows before anything else draws, and the load-op
/// color when the scene list carries no `Clear` of its own.
const BACKDROP: Rgba = Rgba::rgb(0.07, 0.09, 0.13);

/// The world-origin crosshair, drawn in target pixels so it stays a hairline
/// at any zoom.
const AXIS: Rgba = Rgba::rgb(0.34, 0.37, 0.44);

/// Entity placeholder: scenes carry only transforms so far, so each entity is
/// drawn as this much world units through its transform — big enough to see
/// where it is, and it scales with the entity like a sprite would.
const MARKER_SIZE: f32 = 16.0;
const MARKER: Rgba = Rgba::rgb(0.36, 0.68, 0.86);

/// Which world point sits at the centre of the viewport, and how many target
/// pixels one world unit covers.
///
/// The three operations this exists for — pan, zoom-at-pointer, and the
/// pointer conversions picking will need — are all expressed against the
/// target's pixel size, passed in per call so the camera stays a plain value
/// that tests can move around without a panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportCamera {
    /// World point shown at the target's centre.
    pub center: Vec2,
    /// Target pixels per world unit.
    pub zoom: f32,
}

impl Default for ViewportCamera {
    fn default() -> Self {
        Self {
            center: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl ViewportCamera {
    /// Zoom stops: far enough out to see a whole level, far enough in to place
    /// a detail, and no further.
    pub const MIN_ZOOM: f32 = 0.05;
    pub const MAX_ZOOM: f32 = 32.0;

    /// A world point's position in target pixels.
    pub fn to_screen(&self, world: Vec2, view: Vec2) -> Vec2 {
        (world - self.center) * self.zoom + view * 0.5
    }

    /// The inverse of [`to_screen`](Self::to_screen): the world point under a
    /// target pixel. This is what pointer events are converted to before any
    /// viewport control acts on them.
    pub fn to_world(&self, screen: Vec2, view: Vec2) -> Vec2 {
        (screen - view * 0.5) / self.zoom + self.center
    }

    /// Follow a drag: `screen_delta` is how far the pointer moved in target
    /// pixels, and the content moves by the same amount.
    pub fn pan(&mut self, screen_delta: Vec2) {
        self.center -= screen_delta / self.zoom;
    }

    /// Scale by `factor` keeping the world point under `anchor` (a target
    /// pixel) fixed, so zooming is always zooming toward the pointer.
    ///
    /// A `factor` that is not a finite positive scale — a runaway scroll
    /// delta, say — leaves the camera alone rather than poisoning it.
    pub fn zoom_at(&mut self, factor: f32, anchor: Vec2, view: Vec2) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        if zoom == self.zoom {
            return;
        }
        // The anchor's world point must survive the scale change:
        // center' = world - (anchor - centre) / zoom', expanded through the
        // world point's definition at the current zoom.
        self.center += (anchor - view * 0.5) * (1.0 / self.zoom - 1.0 / zoom);
        self.zoom = zoom;
    }
}

/// The viewport panel's engine side: eframe's render state, the offscreen
/// target built on it, the camera, and the frame's command list.
pub struct ViewportHost {
    /// eframe's, cloned each frame it is offered. `None` until the app sees
    /// one (and stays that way on a non-wgpu backend, where there is nothing
    /// to render into).
    render: Option<RenderState>,
    target: Option<Offscreen2d>,
    /// The target's id in egui's texture map, kept so a resize can update it
    /// in place instead of allocating a new one.
    texture: Option<egui::TextureId>,
    camera: ViewportCamera,
    /// Reused frame to frame; [`build_scene`] resets it.
    list: DrawList,
}

impl Default for ViewportHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewportHost {
    pub fn new() -> Self {
        Self {
            render: None,
            target: None,
            texture: None,
            camera: ViewportCamera::default(),
            list: DrawList::new(),
        }
    }

    pub fn camera(&self) -> &ViewportCamera {
        &self.camera
    }

    pub fn camera_mut(&mut self) -> &mut ViewportCamera {
        &mut self.camera
    }

    /// Adopt the render state eframe offers this frame, if any. Offered every
    /// frame; only a `Some` changes anything, so a backend hiccup cannot drop
    /// the target that already exists.
    pub fn set_render_state(&mut self, state: Option<&RenderState>) {
        if let Some(state) = state {
            self.render = Some(state.clone());
        }
    }

    /// Make sure a target of `size` (target pixels) exists and is registered
    /// with egui, then return the id the panel paints as an image.
    ///
    /// Returns `None` when there is no render state — a non-wgpu backend, or
    /// the first frame before one is offered — which tells the panel to fall
    /// back to its placeholder. A size change recreates the texture and points
    /// the existing id at it, so the panel's image keeps working across
    /// resizes.
    pub fn prepare(&mut self, size: [u32; 2]) -> Option<egui::TextureId> {
        let render = self.render.as_ref()?;
        match &mut self.target {
            Some(target) if target.size() == size => {}
            Some(target) => {
                target.resize(size[0], size[1]);
                render
                    .renderer
                    .write()
                    .update_egui_texture_from_wgpu_texture(
                        &render.device,
                        target.view(),
                        eframe::egui_wgpu::wgpu::FilterMode::Linear,
                        self.texture
                            .expect("a registered texture alongside the target"),
                    );
            }
            None => {
                let target = Offscreen2d::new(&render.device, &render.queue, size[0], size[1]);
                let id = render.renderer.write().register_native_texture(
                    &render.device,
                    target.view(),
                    eframe::egui_wgpu::wgpu::FilterMode::Linear,
                );
                self.target = Some(target);
                self.texture = Some(id);
            }
        }
        self.texture
    }

    /// Draw `world` through the camera into the target.
    ///
    /// Called after [`prepare`](Self::prepare) returned `Some` — the panel's
    /// order — which is the invariant behind the panic.
    pub fn draw_scene(&mut self, world: &World) -> Result<(), TextError> {
        let target = self
            .target
            .as_mut()
            .expect("draw_scene called before prepare");
        let [width, height] = target.size();
        let view = Vec2::new(width as f32, height as f32);
        build_scene(&mut self.list, world, &self.camera, view);
        target.render(&self.list, BACKDROP)
    }
}

/// Fill `list` with the scene as the viewport should show it: backdrop,
/// origin crosshair, one placeholder per transform-bearing entity.
///
/// Everything is mapped through `camera` into the `view` rectangle's pixels,
/// so the list is ready to hand to a renderer sized the same. Entities whose
/// placeholder falls entirely outside the view are dropped — a scene can hold
/// thousands of them, and the panel resizes every frame anyway.
pub fn build_scene(list: &mut DrawList, world: &World, camera: &ViewportCamera, view: Vec2) {
    list.reset();
    list.clear(BACKDROP);

    let origin = camera.to_screen(Vec2::ZERO, view);
    list.fill_rect(0.0, origin.y - 0.5, view.x, 1.0, AXIS);
    list.fill_rect(origin.x - 0.5, 0.0, 1.0, view.y, AXIS);

    for (_, transform) in world.iter::<Transform>() {
        let bounds = transform.aabb(Vec2::splat(MARKER_SIZE));
        let min = camera.to_screen(Vec2::new(bounds.x, bounds.y), view);
        let size = Vec2::new(bounds.w, bounds.h) * camera.zoom;
        let off_view =
            min.x > view.x || min.y > view.y || min.x + size.x < 0.0 || min.y + size.y < 0.0;
        if off_view {
            continue;
        }
        list.fill_rect(min.x, min.y, size.x, size.y, MARKER);
    }
}

#[cfg(test)]
mod tests {
    use cubic_core::render::DrawCommand;

    use super::*;

    fn assert_close(got: Vec2, want: Vec2, what: &str) {
        assert!(
            (got - want).length() < 1e-4,
            "{what}: got {got:?}, want {want:?}"
        );
    }

    fn rects(list: &DrawList) -> Vec<(f32, f32, f32, f32)> {
        list.commands
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Rect { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn screen_and_world_conversions_round_trip() {
        let camera = ViewportCamera {
            center: Vec2::new(10.0, -5.0),
            zoom: 2.5,
        };
        let view = Vec2::new(800.0, 600.0);
        for world in [Vec2::ZERO, Vec2::new(3.0, 4.0), Vec2::new(-100.0, 50.0)] {
            let screen = camera.to_screen(world, view);
            assert_close(camera.to_world(screen, view), world, "round trip");
        }
    }

    #[test]
    fn pan_moves_the_content_with_the_drag() {
        let mut camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 100.0);
        let origin_before = camera.to_screen(Vec2::ZERO, view);

        camera.pan(Vec2::new(40.0, 0.0));

        let origin_after = camera.to_screen(Vec2::ZERO, view);
        assert_close(
            origin_after - origin_before,
            Vec2::new(40.0, 0.0),
            "origin follows",
        );
        assert_close(
            camera.center,
            Vec2::new(-40.0, 0.0),
            "center leans into the drag",
        );
    }

    #[test]
    fn zoom_at_keeps_the_anchor_world_point_still() {
        let mut camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 100.0);
        let anchor = Vec2::new(30.0, 70.0);
        let before = camera.to_world(anchor, view);

        camera.zoom_at(1.5, anchor, view);

        assert!((camera.zoom - 1.5).abs() < 1e-6, "zoom applied");
        assert_close(camera.to_world(anchor, view), before, "anchor held");
    }

    #[test]
    fn zoom_stops_at_the_camera_limits() {
        let mut camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 100.0);

        camera.zoom_at(1.0e9, Vec2::ZERO, view);
        assert_eq!(camera.zoom, ViewportCamera::MAX_ZOOM);

        camera.zoom_at(1.0e-9, Vec2::ZERO, view);
        assert_eq!(camera.zoom, ViewportCamera::MIN_ZOOM);
    }

    #[test]
    fn a_nonsense_zoom_factor_is_ignored() {
        let mut camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 100.0);

        camera.zoom_at(f32::NAN, Vec2::ZERO, view);
        camera.zoom_at(0.0, Vec2::ZERO, view);
        camera.zoom_at(-2.0, Vec2::ZERO, view);

        assert_eq!(camera.zoom, 1.0);
        assert_eq!(camera.center, Vec2::ZERO);
    }

    #[test]
    fn prepare_without_a_render_state_has_no_texture() {
        let mut host = ViewportHost::new();
        assert!(host.prepare([64, 64]).is_none());
    }

    #[test]
    fn a_scene_draws_its_backdrop_crosshair_and_entities() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Transform::from_position(Vec2::new(10.0, 5.0)));

        let camera = ViewportCamera::default();
        let view = Vec2::new(100.0, 100.0);
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, view);

        assert!(
            matches!(list.commands.first(), Some(DrawCommand::Clear(_))),
            "the scene starts with the backdrop"
        );
        // The crosshair (two hairlines through the centre) plus the marker.
        let drawn = rects(&list);
        assert_eq!(drawn.len(), 3, "crosshair and one marker: {drawn:?}");

        // The marker is the 16-unit box around (10, 5), in view pixels.
        let marker = drawn
            .iter()
            .find(|(_, _, w, _)| (*w - 16.0).abs() < 1e-4)
            .expect("the entity's marker");
        assert_close(
            Vec2::new(marker.0, marker.1),
            Vec2::new(52.0, 47.0),
            "marker top-left",
        );
    }

    #[test]
    fn entities_outside_the_view_are_not_drawn() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Transform::from_position(Vec2::new(10.0, 5.0)));

        let camera = ViewportCamera {
            center: Vec2::new(1000.0, 0.0),
            ..ViewportCamera::default()
        };
        let view = Vec2::new(100.0, 100.0);
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, view);

        assert_eq!(rects(&list).len(), 2, "only the crosshair is in view");
    }

    #[test]
    fn rebuilding_a_list_replaces_the_previous_frame() {
        let world = World::new();
        let camera = ViewportCamera::default();
        let view = Vec2::new(100.0, 100.0);

        let mut list = DrawList::new();
        list.fill_rect(1.0, 1.0, 1.0, 1.0, Rgba::rgb(1.0, 1.0, 1.0));
        build_scene(&mut list, &world, &camera, view);
        build_scene(&mut list, &world, &camera, view);

        // Two frames of crosshair, not three: the stale rect was reset away.
        assert_eq!(rects(&list).len(), 2);
    }
}
