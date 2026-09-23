use cubic_core::render::{DrawCommand, DrawList, Rgba};
use web_sys::CanvasRenderingContext2d;

/// 2d-canvas backend. Flushes a full `DrawList` once per frame, so the game
/// code never crosses the wasm->JS boundary mid-draw. This is the fallback
/// path; the wgpu backend replaces it when available.
pub struct CanvasRenderer {
    ctx: CanvasRenderingContext2d,
    width: f32,
    height: f32,
}

impl CanvasRenderer {
    pub fn new(ctx: CanvasRenderingContext2d, width: f32, height: f32) -> Self {
        Self { ctx, width, height }
    }

    fn css_color(color: Rgba) -> String {
        format!(
            "rgba({},{},{},{:.3})",
            (color.r * 255.0) as u32,
            (color.g * 255.0) as u32,
            (color.b * 255.0) as u32,
            color.a
        )
    }

    pub fn render(&mut self, list: &DrawList) {
        for command in &list.commands {
            match command {
                DrawCommand::Clear(color) => {
                    self.ctx.set_fill_style_str(&Self::css_color(*color));
                    self.ctx
                        .fill_rect(0.0, 0.0, self.width as f64, self.height as f64);
                }
                DrawCommand::Rect { x, y, w, h, color } => {
                    self.ctx.set_fill_style_str(&Self::css_color(*color));
                    self.ctx
                        .fill_rect((*x).into(), (*y).into(), (*w).into(), (*h).into());
                }
                DrawCommand::Text {
                    text,
                    x,
                    y,
                    size,
                    color,
                } => {
                    self.ctx.set_font(&format!("{}px monospace", *size as u32));
                    self.ctx.set_text_baseline("top");
                    self.ctx.set_fill_style_str(&Self::css_color(*color));
                    let _ = self.ctx.fill_text(text, (*x).into(), (*y).into());
                }
            }
        }
    }
}
