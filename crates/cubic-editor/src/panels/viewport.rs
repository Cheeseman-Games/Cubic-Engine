//! Centre pane: the scene viewport, and the preview of an opened file.
//!
//! The engine's wgpu renderer renders into this panel (shared-device path) in
//! a follow-up; until then the pane shows the checkerboard with the scene
//! name, or — when a file was opened from the tree — its preview.

use egui::Color32;
use egui::Pos2;
use egui::Rect;
use egui::Sense;
use egui::Vec2;

use crate::preview::{Preview, PreviewKind};
use crate::state::EditorState;

/// Checkerboard cell edge, in points.
const CELL: f32 = 16.0;

/// Draws the viewport pane.
pub fn viewport(ui: &mut egui::Ui, state: &mut EditorState) {
    if state.preview.is_some() {
        preview(ui, state);
        return;
    }
    let (rect, _response) = ui.allocate_exact_size(ui.available_size(), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        paint_checkerboard(&painter, rect);
        draw_overlay(&painter, rect, state);
    }
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
    let scene = state
        .scene
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("<no scene>");
    let (w, h) = (rect.size().x, rect.size().y);
    let color = Color32::from_rgb(0xbb, 0xbb, 0xbb);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("{scene} — viewport {w:.0} x {h:.0}"),
        egui::FontId::proportional(18.0),
        color,
    );
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
