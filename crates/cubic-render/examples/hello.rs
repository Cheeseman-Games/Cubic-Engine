//! The whole hello game: one glob import, one `Game` impl, one macro.
//!
//! No `main`, no window code, no renderer, no platform module — `engine_main!`
//! generates all of it from the crate name and `Game`. This file is the size
//! every game project starts at.
//!
//! Run: `cargo run -p cubic-render --example hello`
//!
//! `update` is a fixed step, not a frame gap: it runs 60 times a second at
//! `dt = 1/60` however fast the display is, so `t` below is a real clock and
//! not a frame counter.

use cubic_render::prelude::*;

struct Hello {
    t: f32,
}

impl Game for Hello {
    fn update(&mut self, dt: f32, _input: &InputState, _frame: &FrameInput) {
        self.t += dt;
    }

    fn draw(&mut self, list: &mut DrawList) {
        list.clear(Rgba::rgb(0.06, 0.08, 0.12));

        // A bar that grows and shrinks, so the frame loop is visibly running.
        let w = 160.0 + 80.0 * self.t.sin().abs();
        list.fill_rect(40.0, 40.0, w, 56.0, Rgba::rgb(1.0, 0.55, 0.25));
        list.text("cubic", 40.0, 120.0, 40.0, Rgba::rgb(0.94, 0.95, 1.0));
        list.text(
            &format!("t = {:.1}s — Esc quits", self.t),
            40.0,
            180.0,
            20.0,
            Rgba::rgb(0.7, 0.78, 0.9),
        );
    }
}

engine_main!(Hello { t: 0.0 });
