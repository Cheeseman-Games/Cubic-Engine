//! Centre pane: the scene viewport.
//!
//! In this session it is a static checkerboard with an overlay; the engine's
//! wgpu renderer renders into this panel (shared-device path) in a follow-up.

use egui::Color32;
use egui::Pos2;
use egui::Rect;
use egui::Sense;
use egui::Vec2;

use crate::state::EditorState;

/// Checkerboard cell edge, in points.
const CELL: f32 = 16.0;

/// Draws the viewport pane.
pub fn viewport(ui: &mut egui::Ui, state: &mut EditorState) {
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
