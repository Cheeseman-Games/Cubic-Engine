//! Centre pane: the scene viewport, and the preview of an opened file.
//!
//! With a scene open, the pane paints the engine's offscreen target (built and
//! fed by [`crate::viewport::ViewportHost`]) as an image and steers its camera
//! from the pointer: middle-drag pans, wheel or pinch zooms toward the cursor.
//! With no scene — or no GPU render state to draw into — it shows the
//! checkerboard placeholder instead.

use egui::Color32;
use egui::Pos2;
use egui::Rect;
use egui::Sense;
use egui::Vec2;

use cubic_core::math;

use crate::preview::{Preview, PreviewKind};
use crate::state::{EditorState, LogLevel};
use crate::viewport::ViewportHost;

/// Checkerboard cell edge, in points.
const CELL: f32 = 16.0;

/// How much a scroll's worth of wheel input zooms: the same curve egui gives
/// ctrl+wheel, so a notch feels identical whether it arrives plain or modified.
const ZOOM_SPEED: f32 = 1.0 / 200.0;

/// Draws the viewport pane.
pub fn viewport(ui: &mut egui::Ui, state: &mut EditorState, host: &mut ViewportHost) {
    if state.preview.is_some() {
        preview(ui, state);
        return;
    }
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::drag());
    if !ui.is_rect_visible(rect) {
        return;
    }

    let Some(scene) = state.scene.as_ref() else {
        placeholder(ui, rect, state);
        return;
    };

    // The target tracks the panel at the display's pixel density, so the
    // image is sharp and the camera's pixel space is the target's own.
    let ppp = ui.ctx().pixels_per_point();
    let size = [
        (rect.width() * ppp).round().max(1.0) as u32,
        (rect.height() * ppp).round().max(1.0) as u32,
    ];
    camera_input(ui, host, &response, rect, size, ppp);

    let Some(texture) = host.prepare(size) else {
        placeholder(ui, rect, state);
        return;
    };

    let drawn = host.draw_scene(&scene.world);
    let painter = ui.painter_at(rect);
    painter.image(
        texture,
        rect,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
    overlay(&painter, rect, state, host.camera().zoom);

    if let Err(error) = drawn {
        state.log(LogLevel::Error, format!("viewport: {error}"));
    }
}

/// Route the viewport rect's pointer events into camera moves. Input is read
/// before the frame is drawn, so a pan or zoom lands in the same frame it
/// happened in.
fn camera_input(
    ui: &egui::Ui,
    host: &mut ViewportHost,
    response: &egui::Response,
    rect: Rect,
    size: [u32; 2],
    ppp: f32,
) {
    if response.dragged_by(egui::PointerButton::Middle) {
        let delta = response.drag_delta();
        host.camera_mut()
            .pan(math::Vec2::new(delta.x * ppp, delta.y * ppp));
    }
    if !response.hovered() {
        return;
    }
    let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta(), i.zoom_delta()));
    if scroll.y == 0.0 && pinch == 1.0 {
        return;
    }
    let Some(anchor) = response.hover_pos() else {
        return;
    };
    let factor = (scroll.y * ZOOM_SPEED).exp() * pinch;
    let view = math::Vec2::new(size[0] as f32, size[1] as f32);
    // egui's pointer is in points over the rect; the camera's screen space is
    // the target's pixels, so the density scale closes the gap.
    let anchor = anchor - rect.min;
    host.camera_mut().zoom_at(
        factor,
        math::Vec2::new(anchor.x * ppp, anchor.y * ppp),
        view,
    );
}

/// The no-scene, no-GPU state: the checkerboard with whatever is known about
/// the centre pane written over it.
fn placeholder(ui: &egui::Ui, rect: Rect, state: &EditorState) {
    let painter = ui.painter_at(rect);
    paint_checkerboard(&painter, rect);
    draw_overlay(&painter, rect, state);
}

/// A subtle 2x2 checkerboard so the viewport is visible against the chrome.
fn paint_checkerboard(painter: &egui::Painter, rect: Rect) {
    let light = Color32::from_rgb(0x25, 0x25, 0x33);
    let dark = Color32::from_rgb(0x1c, 0x1c, 0x28);
    let mut index = 0u32;
    let mut y = rect.top();
    while y < rect.bottom() {
        let mut x = rect.left();
        while x < rect.right() {
            let cell = Rect::from_min_size(Pos2::new(x, y), Vec2::splat(CELL));
            painter.rect_filled(
                cell,
                egui::CornerRadius::ZERO,
                if index.is_multiple_of(2) { light } else { dark },
            );
            index += 1;
            x += CELL;
        }
        index += 1;
        y += CELL;
    }
}

/// Draws the scene name and viewport size in the middle of the canvas.
fn draw_overlay(painter: &egui::Painter, rect: Rect, state: &EditorState) {
    let (w, h) = (rect.size().x, rect.size().y);
    let color = Color32::from_rgb(0xbb, 0xbb, 0xbb);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("{} — viewport {w:.0} x {h:.0}", scene_name(state)),
        egui::FontId::proportional(18.0),
        color,
    );
}

/// Scene name and zoom over the rendered frame, in the corner so the centre
/// of the canvas stays clear.
fn overlay(painter: &egui::Painter, rect: Rect, state: &EditorState, zoom: f32) {
    painter.text(
        rect.left_top() + egui::vec2(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        format!("{} — {:.0}%", scene_name(state), zoom * 100.0),
        egui::FontId::monospace(12.0),
        Color32::from_rgb(0xbb, 0xbb, 0xbb),
    );
}

/// The open scene's file name, for either overlay.
fn scene_name(state: &EditorState) -> &str {
    state
        .scene
        .as_ref()
        .and_then(|scene| scene.path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("<no scene>")
}

/// The opened file's preview: a header with its name, then the contents.
fn preview(ui: &mut egui::Ui, state: &mut EditorState) {
    let mut close = false;
    ui.horizontal(|ui| {
        let preview = state.preview.as_ref().expect("checked by the caller");
        let title = match &preview.kind {
            PreviewKind::Image { size, .. } => {
                format!("{} — {}×{}", shown_name(state, preview), size[0], size[1])
            }
            _ => shown_name(state, preview),
        };
        ui.label(title);
        if preview.truncated() {
            ui.colored_label(
                Color32::from_rgb(0xe5, 0x9b, 0x3b),
                "showing the first part only",
            );
        }
        if ui.button("✕").clicked() {
            close = true;
        }
    });
    ui.separator();
    let available = ui.available_size();

    let preview = state.preview.as_ref().expect("checked by the caller");
    match &preview.kind {
        PreviewKind::Image { texture, size } => paint_image(ui, texture, *size, available),
        PreviewKind::Text { text, .. } => show_text(ui, text),
        PreviewKind::Error(message) => {
            ui.colored_label(Color32::from_rgb(0xef, 0x43, 0x43), message.clone());
        }
    }

    if close {
        state.preview = None;
    }
}

/// The preview's file, as it is named in the header: project-relative.
fn shown_name(state: &EditorState, preview: &Preview) -> String {
    crate::project::shown(state, &preview.path)
}

/// The image, centred and scaled to fit the pane without cropping.
fn paint_image(ui: &mut egui::Ui, texture: &egui::TextureHandle, size: [usize; 2], area: Vec2) {
    let (rect, _response) = ui.allocate_exact_size(area, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        egui::CornerRadius::ZERO,
        Color32::from_rgb(0x14, 0x14, 0x1e),
    );

    let (width, height) = (size[0] as f32, size[1] as f32);
    let scale = (rect.width() / width).min(rect.height() / height);
    let scaled = Vec2::new(width * scale, height * scale);
    let image = Rect::from_center_size(rect.center(), scaled);
    painter.image(
        texture.id(),
        image,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
}

/// The text, scrolled and virtualized to a row at a time.
fn show_text(ui: &mut egui::Ui, text: &str) {
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_height, text.lines().count(), |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for line in text.lines().skip(range.start).take(range.len()) {
                ui.monospace(line);
            }
        });
}
