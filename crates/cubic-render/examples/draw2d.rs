//! Desktop example: drives the immediate-mode 2D renderer — a full grid of
//! instanced quad rectangles plus a bouncing highlight, animated at the
//! display refresh rate.
//!
//! Output goes through the backend-agnostic `DrawList` (`cubic_core`), so this
//! exact draw code would run identically on the wasm canvas backend.
//!
//! Run: `cargo run -p cubic-render --example draw2d`

use cubic_core::render::{DrawList, Renderer, Rgba};
use cubic_render::app::{AppDelegate, Application, WindowConfig};

const WIDTH: f32 = 960.0;
const HEIGHT: f32 = 540.0;

struct Scene {
    t: f32,
}

impl AppDelegate for Scene {
    fn update(&mut self, dt: f32) {
        self.t += dt;
    }

    fn draw(&mut self, list: &mut DrawList) {
        list.clear(Rgba::rgb(0.06, 0.08, 0.12));

        // A hue-spiralling grid of quads — one instanced draw call total.
        const CELLS: u32 = 10;
        let cell = WIDTH / CELLS as f32;
        for gx in 0..CELLS {
            for gy in 0..CELLS {
                let x = gx as f32 * cell;
                let y = gy as f32 * cell;
                let hue = (gx + gy) as f32 / (2 * CELLS) as f32;
                let (r, g, b) = hsv_to_rgb(hue, 0.65, 0.9);
                list.fill_rect(x + 2.0, y + 2.0, cell - 4.0, cell - 4.0, Rgba::rgb(r, g, b));
            }
        }

        // A bouncing highlight so the animation is visibly running.
        let bx = (self.t * 140.0) % (WIDTH - 48.0);
        let by = 40.0 + (self.t * 2.4).sin() * 40.0;
        list.fill_rect(bx, by, 24.0, 24.0, Rgba::new(1.0, 0.9, 0.55, 0.9));
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
        title: "cubic — draw2d".to_string(),
        width: WIDTH.into(),
        height: HEIGHT.into(),
        ..WindowConfig::default()
    };

    Application::new(config)
        .run(Scene { t: 0.0 })
        .expect("application ended early");
}
