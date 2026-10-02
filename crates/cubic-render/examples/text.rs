//! Desktop example: text rendering — a title, a size ramp, a color sweep and a
//! live HUD, all emitted through the same backend-agnostic
//! `DrawCommand::Text` that the wasm canvas backend consumes.
//!
//! Run: `cargo run -p cubic-render --example text`
//!
//! Fonts come from the system font database. Set `CUBIC_FONT` to a font file (or
//! a directory of them) to add to it — see `text::FONT_PATH_ENV` in the engine.

use cubic_core::input::{FrameInput, InputState};
use cubic_core::render::{DrawList, Renderer, Rgba};
use cubic_render::app::{AppDelegate, Application, WindowConfig};

const WIDTH: f32 = 960.0;
const HEIGHT: f32 = 540.0;

/// Sizes rendered as one paragraph each, to show that any size works and that
/// each one gets its own glyphs.
const SIZES: [f32; 6] = [12.0, 14.0, 16.0, 20.0, 24.0, 32.0];
const SWATCHES: usize = 24;

struct Scene {
    t: f32,
    frames: u64,
}

impl AppDelegate for Scene {
    fn update(&mut self, dt: f32, _input: &InputState, _frame: &FrameInput) {
        self.t += dt;
        self.frames += 1;
    }

    fn draw(&mut self, list: &mut DrawList) {
        list.clear(Rgba::rgb(0.06, 0.08, 0.12));

        list.text("cubic text", 40.0, 28.0, 56.0, Rgba::rgb(0.94, 0.95, 1.0));
        list.fill_rect(40.0, 100.0, 240.0, 2.0, Rgba::new(0.4, 0.6, 1.0, 0.8));

        // Draw order is command order: this bar tints the label painted before it
        // instead of hiding behind it, same as the canvas backend.
        list.text(
            "interleaved with rects",
            40.0,
            118.0,
            24.0,
            Rgba::rgb(0.9, 0.9, 0.95),
        );
        list.fill_rect(46.0, 122.0, 150.0, 18.0, Rgba::new(1.0, 0.4, 0.2, 0.35));

        let mut y = 160.0;
        for size in SIZES {
            let text = format!("{size:.0}px  The quick brown fox jumps over 0123456789");
            list.text(&text, 40.0, y, size, Rgba::rgb(0.82, 0.85, 0.9));
            y += size * 1.35;
        }

        // One color per glyph, all at the same size.
        for index in 0..SWATCHES {
            let hue = index as f32 / SWATCHES as f32;
            let (r, g, b) = hsv_to_rgb(hue, 0.7, 1.0);
            list.text(
                "Aa",
                40.0 + index as f32 * 36.0,
                y + 8.0,
                24.0,
                Rgba::rgb(r, g, b),
            );
        }

        // A HUD whose numbers change every frame — the layout cache only re-shapes
        // what actually moved.
        list.fill_rect(
            0.0,
            HEIGHT - 56.0,
            WIDTH,
            56.0,
            Rgba::new(0.0, 0.0, 0.0, 0.55),
        );
        list.text(
            &format!("t {:.1}s", self.t),
            40.0,
            HEIGHT - 42.0,
            20.0,
            Rgba::rgb(0.7, 0.8, 0.95),
        );
        list.text(
            &format!("frames {}", self.frames),
            160.0,
            HEIGHT - 42.0,
            20.0,
            Rgba::rgb(0.7, 0.8, 0.95),
        );
        list.text(
            &format!("fps {:.0}", self.frames as f32 / self.t.max(f32::EPSILON)),
            300.0,
            HEIGHT - 42.0,
            20.0,
            Rgba::rgb(0.7, 0.8, 0.95),
        );
        list.text(
            "HP 100 / 100",
            WIDTH - 220.0,
            HEIGHT - 42.0,
            24.0,
            Rgba::rgb(0.45, 0.95, 0.55),
        );
    }

    fn clear_color(&self) -> Rgba {
        Rgba::rgb(0.06, 0.08, 0.12)
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = h - h.floor();
    let i = (h * 6.0).floor() as u32;
    let f = h * 6.0 - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

fn main() {
    env_logger::init();

    let config = WindowConfig {
        title: "cubic — text".to_string(),
        width: WIDTH.into(),
        height: HEIGHT.into(),
        ..WindowConfig::default()
    };

    Application::new(config)
        .run(Scene { t: 0.0, frames: 0 })
        .expect("application ended early");
}
