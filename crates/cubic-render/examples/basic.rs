//! Minimal desktop example: opens a winit window, clears the backbuffer to a
//! slowly-cycling color via wgpu, and presents at the display refresh rate
//! (vsync, typically 60 fps). The shell logs the measured presentation rate
//! once per second — on a 60 Hz display that reads ~60 fps.
//!
//! Run: `cargo run -p cubic-render --example basic`

use cubic_core::render::Rgba;
use cubic_render::app::{AppDelegate, Application, WindowConfig};

/// A modest demo state: it animates the clear color so the presentation loop
/// is visibly running (and `dt` is provably being fed).
struct Cycle {
    t: f32,
}

impl AppDelegate for Cycle {
    fn update(&mut self, dt: f32) {
        self.t += dt;
    }

    fn clear_color(&self) -> Rgba {
        // One full sweep of the hue circle every ~10 seconds.
        hsv_to_rgb(self.t / 10.0, 0.85, 0.85)
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgba {
    let h = h - h.floor();
    let i = (h * 6.0).floor() as u32;
    let f = h * 6.0 - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    Rgba::rgb(r, g, b)
}

fn main() {
    env_logger::init();

    let config = WindowConfig {
        title: "cubic — basic".to_string(),
        ..WindowConfig::default()
    };

    Application::new(config)
        .run(Cycle { t: 0.0 })
        .expect("application ended early");
}
