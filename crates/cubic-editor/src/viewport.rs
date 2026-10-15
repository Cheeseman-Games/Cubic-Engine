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
//!
//! Beyond framing the scene, this module owns the canvas furniture the editor
//! draws over it: an adaptive grid, the scene's bounds, the selection outline,
//! and the sprites themselves in layer order ([`build_scene`]), plus [`pick`],
//! which turns a click back into the entity under it so the viewport can drive
//! the selection the hierarchy and inspector panels already share. A texture
//! dragged out of the asset panel arrives as a [`TextureDrag`].

use cubic_core::assets::AssetServer;
use cubic_core::components::{Sprite, Transform};
use cubic_core::math::{Rect, Vec2};
use cubic_core::render::{DrawList, Renderer, Rgba};
use cubic_core::world::{EntityId, World};
use cubic_render::offscreen::Offscreen2d;
use cubic_render::text::TextError;
use eframe::egui_wgpu::RenderState;

use crate::gizmo::{self, GizmoMode};

/// What a texture row in the asset panel carries while it is dragged onto the
/// viewport.
///
/// The path is project-relative, the same spelling an [`AssetServer`] loads it
/// by; `Any + Send + Sync` is the shape egui's drag-and-drop stores payloads in.
#[derive(Clone, Debug)]
pub struct TextureDrag {
    /// The texture's path, relative to the project root.
    pub path: String,
}

/// Backdrop: what the panel shows before anything else draws, and the load-op
/// color when the scene list carries no `Clear` of its own.
const BACKDROP: Rgba = Rgba::rgb(0.07, 0.09, 0.13);

/// Fine grid lines, one per [`grid_step`], faint enough to read as texture
/// rather than furniture.
const GRID: Rgba = Rgba::rgb(0.13, 0.15, 0.20);

/// The world-origin crosshair, drawn in target pixels so it stays a hairline
/// at any zoom. Brighter than [`GRID`], so the axes read through it.
const AXIS: Rgba = Rgba::rgb(0.34, 0.37, 0.44);

/// The box around everything the scene holds, so an entity placed off in the
/// void still shows where the scene's mass sits.
const BOUNDS: Rgba = Rgba::rgb(0.40, 0.32, 0.55);

/// Selection highlight: the fill of the selected entity's placeholder and the
/// outline drawn around it.
const MARKER_SELECTED: Rgba = Rgba::rgb(0.93, 0.94, 0.98);
const SELECTION: Rgba = Rgba::rgb(1.0, 0.72, 0.25);

/// Entity placeholder: scenes carry only transforms so far, so each entity is
/// drawn as this much world units through its transform — big enough to see
/// where it is, and it scales with the entity like a sprite would. The gizmo's
/// box uses it too, so a handle sits on the marker it belongs to.
pub(crate) const MARKER_SIZE: f32 = 16.0;
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

/// The spacing, in world units, between grid lines at `zoom`.
///
/// A power-of-two multiple of one world unit, chosen so the on-screen spacing
/// stays in a readable band: coarse enough not to crowd as the camera pulls
/// back, fine enough to place a detail as it pushes in. The band keeps the
/// line count proportional to the viewport rather than to the zoom, so a scene
/// framed from far away draws no more lines than one seen up close.
///
/// A zoom that is not a finite positive scale leaves the grid at its base
/// spacing rather than looping on a degenerate value.
pub fn grid_step(zoom: f32) -> f32 {
    const BASE: f32 = 1.0;
    const MIN_PIXELS: f32 = 10.0;
    if !zoom.is_finite() || zoom <= 0.0 {
        return BASE;
    }
    let mut step = BASE;
    while step * zoom < MIN_PIXELS {
        step *= 2.0;
    }
    while step * zoom > MIN_PIXELS * 2.0 && step > BASE / 1024.0 {
        step *= 0.5;
    }
    step
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
    /// The gizmo drag the pointer is holding, if any. Transient like the
    /// camera: it never outlives the gesture and is not saved anywhere.
    drag: Option<gizmo::Drag>,
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
            drag: None,
            list: DrawList::new(),
        }
    }

    pub fn camera(&self) -> &ViewportCamera {
        &self.camera
    }

    pub fn camera_mut(&mut self) -> &mut ViewportCamera {
        &mut self.camera
    }

    /// The gizmo drag the pointer is holding, if any.
    pub fn drag(&self) -> Option<&gizmo::Drag> {
        self.drag.as_ref()
    }

    /// Begin a gizmo drag; the panel carries it until the pointer is released.
    pub fn begin_drag(&mut self, drag: gizmo::Drag) {
        self.drag = Some(drag);
    }

    /// End the gizmo drag (on release, or when the scene or mode changes under
    /// it).
    pub fn clear_drag(&mut self) {
        self.drag = None;
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

    /// Import whatever the asset server has queued into the target's store.
    ///
    /// Called right after [`prepare`](Self::prepare) returned `Some`, so a
    /// texture that was just loaded — a picker's choice, a dropped sprite — is
    /// drawable in the same frame. Nothing is drained when there is no target
    /// yet: the bytes stay queued and the next frame imports them, rather than
    /// being lost on the frame before the target exists.
    ///
    /// Returns one message per asset the decoder refused, for the caller to log.
    pub fn pump_assets(&mut self, server: Option<&mut AssetServer>) -> Vec<String> {
        let (Some(server), Some(target)) = (server, self.target.as_mut()) else {
            return Vec::new();
        };
        server
            .drain_pending()
            .into_iter()
            .filter_map(|asset| target.import(&asset).err().map(|error| error.to_string()))
            .collect()
    }

    /// Draw `world` through the camera into the target, with `selected`
    /// highlighted and, when `mode` names one and the entity is selected, its
    /// transform gizmo drawn on top.
    ///
    /// Called after [`prepare`](Self::prepare) returned `Some` — the panel's
    /// order — which is the invariant behind the panic.
    pub fn draw_scene(
        &mut self,
        world: &World,
        selected: Option<EntityId>,
        mode: Option<GizmoMode>,
    ) -> Result<(), TextError> {
        let target = self
            .target
            .as_mut()
            .expect("draw_scene called before prepare");
        let [width, height] = target.size();
        let view = Vec2::new(width as f32, height as f32);
        build_scene(&mut self.list, world, &self.camera, view, selected);
        if let Some(mode) = mode
            && let Some(id) = selected
            && let Some(transform) = world.get::<Transform>(id)
        {
            gizmo::draw(&mut self.list, transform, mode, &self.camera, view);
        }
        target.render(&self.list, BACKDROP)
    }
}

/// Fill `list` with the scene as the viewport should show it: backdrop, grid,
/// origin axes, the scene's bounds, a placeholder for each transform-bearing
/// entity with no sprite, every sprite in layer order, and an outline around
/// `selected`.
///
/// Everything is mapped through `camera` into the `view` rectangle's pixels,
/// so the list is ready to hand to a renderer sized the same. Boxes that fall
/// entirely outside the view are dropped — a scene can hold thousands of them,
/// and the panel resizes every frame anyway.
pub fn build_scene(
    list: &mut DrawList,
    world: &World,
    camera: &ViewportCamera,
    view: Vec2,
    selected: Option<EntityId>,
) {
    list.reset();
    list.clear(BACKDROP);
    draw_grid(list, camera, view);

    // The origin axes, drawn only when the origin sits in view so a camera
    // panned far away is not crossed by wire it cannot see the ends of.
    let origin = camera.to_screen(Vec2::ZERO, view);
    if (0.0..=view.x).contains(&origin.x) {
        list.fill_rect(origin.x.round() - 0.5, 0.0, 1.0, view.y, AXIS);
    }
    if (0.0..=view.y).contains(&origin.y) {
        list.fill_rect(0.0, origin.y.round() - 0.5, view.x, 1.0, AXIS);
    }

    if let Some(bounds) = scene_bounds(world) {
        let min = camera.to_screen(Vec2::new(bounds.x, bounds.y), view);
        let size = Vec2::new(bounds.w, bounds.h) * camera.zoom;
        stroke_rect(list, min.x, min.y, size.x, size.y, 1.0, BOUNDS);
    }

    // The selected entity's box is remembered rather than outlined as it is
    // drawn, so the outline lands on top of every sprite, not just the ones
    // painted after it.
    let mut selected_bounds: Option<(f32, f32, f32, f32)> = None;

    // Placeholders first: an entity with no sprite has nothing to draw but the
    // box that shows where it is. They sit under every sprite, so a stand-in
    // never hides the art it belongs to.
    for (id, transform) in world.iter::<Transform>() {
        if world.get::<Sprite>(id).is_some() {
            continue;
        }
        let bounds = transform.aabb(Vec2::splat(MARKER_SIZE));
        let min = camera.to_screen(Vec2::new(bounds.x, bounds.y), view);
        let size = Vec2::new(bounds.w, bounds.h) * camera.zoom;
        if off_view(min, size, view) {
            continue;
        }
        if selected == Some(id) {
            list.fill_rect(min.x, min.y, size.x, size.y, MARKER_SELECTED);
            selected_bounds = Some((min.x, min.y, size.x, size.y));
        } else {
            list.fill_rect(min.x, min.y, size.x, size.y, MARKER);
        }
    }

    // Sprites, back to front: sorted by layer, then by id so same-layer sprites
    // have a stable, total order. This is exactly the order the renderer runs
    // the commands in, so the canvas matches the layer list.
    let mut sprites: Vec<(i32, EntityId)> = world
        .iter::<Sprite>()
        .map(|(id, sprite)| (sprite.layer, id))
        .collect();
    sprites.sort_unstable();
    for (_, id) in sprites {
        let (Some(transform), Some(sprite)) = (world.get::<Transform>(id), world.get::<Sprite>(id))
        else {
            continue;
        };
        // The transform's axis-aligned box decides whether the sprite is worth
        // drawing; the quad itself is then placed rotated about the transform's
        // position, so a turned sprite is drawn turned rather than as its box.
        let bounds = transform.aabb(sprite.size);
        let min = camera.to_screen(Vec2::new(bounds.x, bounds.y), view);
        let extent = Vec2::new(bounds.w, bounds.h) * camera.zoom;
        if off_view(min, extent, view) {
            continue;
        }
        let center = camera.to_screen(transform.position, view);
        let size = sprite.size * transform.scale * camera.zoom;
        list.draw_textured(sprite.textured_quad(
            center.x - size.x * 0.5,
            center.y - size.y * 0.5,
            size.x,
            size.y,
            transform.rotation,
        ));
        if selected == Some(id) {
            selected_bounds = Some((min.x, min.y, extent.x, extent.y));
        }
    }

    if let Some((x, y, w, h)) = selected_bounds {
        stroke_rect(list, x, y, w, h, 2.0, SELECTION);
    }
}

/// Whether a screen box lies entirely outside the `view` rectangle, so drawing
/// it is wasted work.
fn off_view(min: Vec2, size: Vec2, view: Vec2) -> bool {
    min.x > view.x || min.y > view.y || min.x + size.x < 0.0 || min.y + size.y < 0.0
}

/// The world-space extent the viewport treats an entity as having: its sprite's
/// size when it has one, else the fixed marker size.
fn entity_size(world: &World, id: EntityId) -> Vec2 {
    world
        .get::<Sprite>(id)
        .map_or(Vec2::splat(MARKER_SIZE), |sprite| sprite.size)
}

/// Draw an adaptive grid of one-pixel lines over the view.
///
/// Vertical and horizontal lines are laid at every multiple of
/// [`grid_step`](grid_step) inside the visible world rectangle. A degenerate
/// camera or a collapsed view draws nothing rather than looping on an
/// unbounded range.
fn draw_grid(list: &mut DrawList, camera: &ViewportCamera, view: Vec2) {
    if !camera.zoom.is_finite() || camera.zoom <= 0.0 || view.x <= 0.0 || view.y <= 0.0 {
        return;
    }
    let step = grid_step(camera.zoom);
    let min = camera.to_world(Vec2::ZERO, view);
    let max = camera.to_world(view, view);

    let first_x = (min.x / step).ceil() * step;
    let mut x = first_x;
    while x <= max.x {
        let sx = camera.to_screen(Vec2::new(x, 0.0), view).x.round();
        list.fill_rect(sx - 0.5, 0.0, 1.0, view.y, GRID);
        x += step;
    }
    let first_y = (min.y / step).ceil() * step;
    let mut y = first_y;
    while y <= max.y {
        let sy = camera.to_screen(Vec2::new(0.0, y), view).y.round();
        list.fill_rect(0.0, sy - 0.5, view.x, 1.0, GRID);
        y += step;
    }
}

/// The smallest box covering every entity's extent, or `None` for an empty
/// scene.
fn scene_bounds(world: &World) -> Option<Rect> {
    let mut bounds: Option<Rect> = None;
    for (id, transform) in world.iter::<Transform>() {
        let aabb = transform.aabb(entity_size(world, id));
        bounds = Some(match bounds {
            Some(bounds) => bounds.union(&aabb),
            None => aabb,
        });
    }
    bounds
}

/// Draw a rectangle's border as four filled bars, the only stroke the command
/// model offers.
fn stroke_rect(list: &mut DrawList, x: f32, y: f32, w: f32, h: f32, thickness: f32, color: Rgba) {
    list.fill_rect(x, y, w, thickness, color);
    list.fill_rect(x, y + h - thickness, w, thickness, color);
    list.fill_rect(x, y, thickness, h, color);
    list.fill_rect(x + w - thickness, y, thickness, h, color);
}

/// The entity whose extent covers `screen` (a target pixel), if any.
///
/// "Topmost" is the paint order [`build_scene`] draws in: a sprite beats a
/// placeholder, a higher layer beats a lower one, and a greater id wins a tie.
/// A click that hits nothing picks nothing, so the caller can clear the
/// selection rather than leave a stale one.
pub fn pick(world: &World, camera: &ViewportCamera, view: Vec2, screen: Vec2) -> Option<EntityId> {
    let point = camera.to_world(screen, view);
    let mut hit: Option<(i32, EntityId)> = None;
    for (id, transform) in world.iter::<Transform>() {
        if transform
            .aabb(entity_size(world, id))
            .contains(point.x, point.y)
        {
            let key = paint_key(world, id);
            hit = Some(match hit {
                Some(best) if best >= key => best,
                _ => key,
            });
        }
    }
    hit.map(|(_, id)| id)
}

/// An entity's place in the viewport's paint order: sprites sort above
/// placeholders, a higher layer sorts later, and the id breaks ties.
fn paint_key(world: &World, id: EntityId) -> (i32, EntityId) {
    match world.get::<Sprite>(id) {
        Some(sprite) => (sprite.layer, id),
        None => (i32::MIN, id),
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

    /// The rectangles drawn in exactly `color`, for asserting on one layer of
    /// the canvas (grid, marker, outline) without counting the others.
    fn rects_of(list: &DrawList, color: Rgba) -> Vec<(f32, f32, f32, f32)> {
        list.commands
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Rect {
                    x,
                    y,
                    w,
                    h,
                    color: drawn,
                } if *drawn == color => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect()
    }

    /// The textures drawn, in command order, as `(handle id, x, y, w, h)`.
    fn textures(list: &DrawList) -> Vec<(u64, f32, f32, f32, f32)> {
        list.commands
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Texture(quad) => {
                    Some((quad.texture.asset().id(), quad.x, quad.y, quad.w, quad.h))
                }
                _ => None,
            })
            .collect()
    }

    /// The texture commands' whole quads, in command order, for asserting on
    /// everything a sprite draw carries.
    fn quads(list: &DrawList) -> Vec<cubic_core::render::TexturedQuad> {
        list.commands
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Texture(quad) => Some(*quad),
                _ => None,
            })
            .collect()
    }

    fn texture(id: u64) -> cubic_core::assets::TextureHandle {
        cubic_core::assets::TextureHandle::new(
            std::num::NonZeroU64::new(id).expect("a test uses non-zero ids"),
        )
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
    fn a_scene_draws_its_backdrop_grid_axes_and_entities() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Transform::from_position(Vec2::new(10.0, 5.0)));

        let camera = ViewportCamera::default();
        let view = Vec2::new(100.0, 100.0);
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, view, None);

        assert!(
            matches!(list.commands.first(), Some(DrawCommand::Clear(_))),
            "the scene starts with the backdrop"
        );
        assert!(!rects_of(&list, GRID).is_empty(), "the grid is drawn");
        // The origin sits at the view's centre, so both axes cross it.
        assert_eq!(
            rects_of(&list, AXIS).len(),
            2,
            "two axes through the origin"
        );

        // The marker is the 16-unit box around (10, 5), in view pixels.
        let markers = rects_of(&list, MARKER);
        assert_eq!(markers.len(), 1, "one marker: {markers:?}");
        assert_close(
            Vec2::new(markers[0].0, markers[0].1),
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
        build_scene(&mut list, &world, &camera, view, None);

        assert!(rects_of(&list, MARKER).is_empty(), "no marker in view");
        // The origin is far off to the left, so the vertical axis is skipped;
        // the horizontal one (defined by a y still on-screen) remains.
        assert_eq!(rects_of(&list, AXIS).len(), 1, "only the horizontal axis");
    }

    #[test]
    fn rebuilding_a_list_replaces_the_previous_frame() {
        let world = World::new();
        let camera = ViewportCamera::default();
        let view = Vec2::new(100.0, 100.0);

        let mut list = DrawList::new();
        list.fill_rect(1.0, 1.0, 1.0, 1.0, Rgba::rgb(1.0, 1.0, 1.0));
        build_scene(&mut list, &world, &camera, view, None);
        let once = rects(&list).len();
        build_scene(&mut list, &world, &camera, view, None);

        assert_eq!(rects(&list).len(), once, "the stale frame was reset away");
    }

    #[test]
    fn the_selected_entity_is_filled_and_outlined() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Transform::from_position(Vec2::new(10.0, 5.0)));

        let camera = ViewportCamera::default();
        let view = Vec2::new(100.0, 100.0);
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, view, Some(entity));

        assert!(
            rects_of(&list, MARKER).is_empty(),
            "the selected marker is not drawn in the plain color"
        );
        assert_eq!(
            rects_of(&list, MARKER_SELECTED).len(),
            1,
            "filled highlight"
        );
        assert_eq!(
            rects_of(&list, SELECTION).len(),
            4,
            "an outline is four bars"
        );
    }

    #[test]
    fn the_scene_bounds_cover_every_entity() {
        let mut world = World::new();
        for position in [Vec2::new(-50.0, 0.0), Vec2::new(50.0, 20.0)] {
            let id = world.spawn();
            world.insert(id, Transform::from_position(position));
        }

        let camera = ViewportCamera::default();
        let view = Vec2::new(400.0, 400.0);
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, view, None);

        let bars = rects_of(&list, BOUNDS);
        assert_eq!(bars.len(), 4, "the bounds are an outline of four bars");
        // The two markers span x from -58 to 58 and y from -8 to 28 in world
        // units; the bounds' top bar reaches both extremes.
        let (left, right, top, bottom) = bars.iter().fold(
            (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
            |(l, r, t, b), &(x, y, w, h)| (l.min(x), r.max(x + w), t.min(y), b.max(y + h)),
        );
        let min = camera.to_screen(Vec2::new(-58.0, -8.0), view);
        let max = camera.to_screen(Vec2::new(58.0, 28.0), view);
        assert_close(Vec2::new(left, top), min, "bounds min");
        assert_close(Vec2::new(right, bottom), max, "bounds max");
    }

    #[test]
    fn grid_step_keeps_on_screen_spacing_in_a_readable_band() {
        for zoom in [0.05, 0.2, 1.0, 4.0, 32.0] {
            let spacing = grid_step(zoom) * zoom;
            assert!(
                (10.0..=20.0).contains(&spacing),
                "zoom {zoom} gives {spacing}px spacing"
            );
        }
    }

    #[test]
    fn a_degenerate_zoom_does_not_loop_or_divide_by_zero() {
        assert_eq!(grid_step(0.0), 1.0);
        assert_eq!(grid_step(f32::NAN), 1.0);

        // A zero-zoom camera would otherwise produce an infinite world range;
        // the grid bows out and the rest of the scene still builds.
        let mut world = World::new();
        let id = world.spawn();
        world.insert(id, Transform::default());
        let camera = ViewportCamera {
            center: Vec2::ZERO,
            zoom: 0.0,
        };
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, Vec2::new(100.0, 100.0), None);
        assert!(rects_of(&list, GRID).is_empty(), "no grid at a dead zoom");
    }

    #[test]
    fn clicking_an_entity_picks_the_topmost_of_those_under_the_pointer() {
        let mut world = World::new();
        let lower = world.spawn();
        world.insert(lower, Transform::from_position(Vec2::ZERO));
        let upper = world.spawn();
        world.insert(upper, Transform::from_position(Vec2::new(2.0, 0.0)));

        let camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 200.0);
        // A point inside both 16-unit boxes: the higher id is drawn last, so it
        // wins the click.
        let screen = camera.to_screen(Vec2::new(2.0, 0.0), view);

        assert_eq!(pick(&world, &camera, view, screen), Some(upper));
    }

    #[test]
    fn clicking_empty_space_picks_nothing() {
        let mut world = World::new();
        let id = world.spawn();
        world.insert(id, Transform::from_position(Vec2::ZERO));

        let camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 200.0);
        let screen = camera.to_screen(Vec2::new(500.0, 500.0), view);

        assert_eq!(pick(&world, &camera, view, screen), None);
    }

    #[test]
    fn sprites_draw_back_to_front_in_layer_order() {
        let mut world = World::new();
        let front = world.spawn();
        world.insert(front, Transform::default());
        world.insert(front, Sprite::new(texture(1)).with_layer(5));
        let back = world.spawn();
        world.insert(back, Transform::default());
        world.insert(back, Sprite::new(texture(2)).with_layer(-1));

        let camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 200.0);
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, view, None);

        let ids: Vec<u64> = textures(&list).into_iter().map(|t| t.0).collect();
        assert_eq!(ids, vec![2, 1], "the lower layer is painted first");
        assert!(
            rects_of(&list, MARKER).is_empty(),
            "a sprite replaces its placeholder"
        );
    }

    #[test]
    fn a_sprite_draws_a_rotated_quad_about_its_transform() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            Transform::new(
                Vec2::new(10.0, 5.0),
                std::f32::consts::FRAC_PI_2,
                Vec2::splat(2.0),
            ),
        );
        world.insert(
            entity,
            Sprite::new(texture(1)).with_size(Vec2::new(8.0, 4.0)),
        );

        let camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 200.0);
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, view, None);

        let quads = quads(&list);
        assert_eq!(quads.len(), 1, "one sprite, one quad");
        let quad = quads[0];
        let center = camera.to_screen(Vec2::new(10.0, 5.0), view);
        // 8x4 at scale 2 and zoom 1 is 16x8, centred on the transform.
        assert_close(
            Vec2::new(quad.x, quad.y),
            Vec2::new(center.x - 8.0, center.y - 4.0),
            "the quad is centred on the transform",
        );
        assert_eq!((quad.w, quad.h), (16.0, 8.0), "scaled to world size");
        assert_eq!(
            quad.rotation,
            std::f32::consts::FRAC_PI_2,
            "the transform's rotation reaches the draw"
        );
    }

    #[test]
    fn a_sprites_region_and_flip_reach_the_draw() {
        use cubic_core::render::{TextureFilter, UvRect};

        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Transform::default());
        world.insert(
            entity,
            Sprite::new(texture(1))
                .with_size(Vec2::splat(10.0))
                .with_region(UvRect::new(0.0, 0.0, 0.5, 0.5))
                .with_flip(false, true)
                .with_filter(TextureFilter::Nearest),
        );

        let camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 200.0);
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, view, None);

        let quads = quads(&list);
        assert_eq!(quads.len(), 1);
        assert_eq!(
            quads[0].uv,
            UvRect::new(0.0, 0.5, 0.5, 0.0),
            "the vertical flip swaps the region's v edges"
        );
        assert_eq!(quads[0].filter, TextureFilter::Nearest);
    }

    #[test]
    fn same_layer_sprites_break_ties_by_entity_id() {
        let mut world = World::new();
        let first = world.spawn();
        world.insert(first, Transform::default());
        world.insert(first, Sprite::new(texture(1)));
        let second = world.spawn();
        world.insert(second, Transform::default());
        world.insert(second, Sprite::new(texture(2)));

        let camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 200.0);
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, view, None);

        let ids: Vec<u64> = textures(&list).into_iter().map(|t| t.0).collect();
        assert_eq!(ids, vec![1, 2], "same layer paints in id order");
    }

    #[test]
    fn a_selected_sprite_is_outlined_over_the_others() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Transform::default());
        world.insert(entity, Sprite::new(texture(1)).with_layer(3));

        let camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 200.0);
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, view, Some(entity));

        assert_eq!(textures(&list).len(), 1, "the sprite is still drawn");
        assert_eq!(
            rects_of(&list, SELECTION).len(),
            4,
            "the outline is four bars"
        );
        assert!(
            rects_of(&list, MARKER).is_empty(),
            "no placeholder behind the sprite"
        );
    }

    #[test]
    fn picking_prefers_the_sprite_on_the_higher_layer() {
        let mut world = World::new();
        let below = world.spawn();
        world.insert(below, Transform::default());
        world.insert(below, Sprite::new(texture(1)).with_layer(0));
        let above = world.spawn();
        world.insert(above, Transform::default());
        // A *lower* id but a higher layer still wins the click.
        world.insert(above, Sprite::new(texture(2)).with_layer(4));

        let camera = ViewportCamera::default();
        let view = Vec2::new(200.0, 200.0);
        let screen = camera.to_screen(Vec2::ZERO, view);

        assert_eq!(pick(&world, &camera, view, screen), Some(above));
    }

    #[test]
    fn picking_uses_a_sprites_size_not_the_marker() {
        let mut world = World::new();
        let big = world.spawn();
        world.insert(big, Transform::default());
        world.insert(big, Sprite::new(texture(1)).with_size(Vec2::splat(100.0)));

        let camera = ViewportCamera::default();
        let view = Vec2::new(400.0, 400.0);
        // Well outside the 16-unit marker, but inside the 100-unit sprite.
        let screen = camera.to_screen(Vec2::new(40.0, 0.0), view);

        assert_eq!(pick(&world, &camera, view, screen), Some(big));
    }

    #[test]
    fn the_scene_bounds_cover_a_sprites_full_extent() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Transform::default());
        world.insert(entity, Sprite::new(texture(1)).with_size(Vec2::splat(50.0)));

        let camera = ViewportCamera::default();
        let view = Vec2::new(400.0, 400.0);
        let mut list = DrawList::new();
        build_scene(&mut list, &world, &camera, view, None);

        let bars = rects_of(&list, BOUNDS);
        assert_eq!(bars.len(), 4, "the bounds are an outline of four bars");
        // A 50x50 sprite spans +/-25, not the marker's +/-8.
        let min = camera.to_screen(Vec2::new(-25.0, -25.0), view);
        let (left, top) = bars
            .iter()
            .fold((f32::MAX, f32::MAX), |(l, t), &(x, y, _, _)| {
                (l.min(x), t.min(y))
            });
        assert_close(Vec2::new(left, top), min, "bounds follow the sprite");
    }
}
