//! The fixed-timestep loop, visible.
//!
//! `update` is never handed the wall-clock gap between frames: the runtime
//! banks real time and calls it once per whole `FixedTick::step`. This game
//! counts its steps and draws the count as a bar of 60 cells — one per tick of
//! the last simulated second — so you can watch 60 Hz hold steady on whatever
//! refresh the display happens to run at.
//!
//! The readout is deliberately frame-rate independent: on a 144 Hz panel most
//! frames run no step at all and the bar redraws unchanged; on a 30 Hz one every
//! frame runs two. The `t` clock advances by exactly one second per 60 cells.
//!
//! Run: `cargo run -p cubic-render --example tick`

use cubic_render::prelude::*;

const WIDTH: u32 = 720;
const HEIGHT: u32 = 360;
const CELLS: usize = 60;

struct Ticker {
    t: f32,
    /// Rolling log of the `dt` each step was given, for the readout below.
    steps: u32,
}

impl Ticker {
    fn new() -> Self {
        Self { t: 0.0, steps: 0 }
    }
}

impl Game for Ticker {
    fn update(&mut self, dt: f32, _input: &InputState, _frame: &FrameInput) {
        // The point of the whole example: this is the same value every time, on
        // every refresh rate, however late the frame was.
        debug_assert_eq!(
            dt,
            1.0 / 60.0,
            "the runtime must never hand out a variable dt"
        );
        self.t += dt;
        self.steps += 1;
    }

    fn draw(&mut self, list: &mut DrawList) {
        list.clear(Rgba::rgb(0.06, 0.08, 0.12));

        // One cell per step of the last simulated second, lit as it fills.
        let second = self.t - self.t.floor();
        let filled = (second * CELLS as f32) as usize;
        let cell = 8.0;
        for i in 0..CELLS {
            let on = i < filled;
            let shade = 0.25 + 0.75 * (i as f32 / CELLS as f32);
            let color = Rgba::new(
                0.2,
                0.4 + 0.5 * shade,
                0.9 * shade,
                if on { 1.0 } else { 0.18 },
            );
            list.fill_rect(40.0 + i as f32 * cell, 80.0, cell - 2.0, 40.0, color);
        }

        list.text(
            &format!("{:.2}s simulated — {CELLS} steps/s", self.t),
            40.0,
            30.0,
            22.0,
            Rgba::rgb(0.94, 0.95, 1.0),
        );
        list.text(
            &format!("total steps: {}", self.steps),
            40.0,
            140.0,
            18.0,
            Rgba::rgb(0.7, 0.78, 0.9),
        );

        // The bar sweeps on wall-clock time, so it visibly desynchronizes from
        // the simulated clock whenever the display is not a 60 Hz one.
        list.fill_rect(40.0, 180.0, 640.0 * second, 6.0, Rgba::rgb(1.0, 0.55, 0.25));
    }

    fn clear_color(&self) -> Rgba {
        Rgba::rgb(0.06, 0.08, 0.12)
    }
}

fn main() {
    // `engine_main!` would cover this whole file's plumbing, but the tick rate is
    // the one runtime setting worth naming out loud, so the runtime is driven
    // directly here.
    run_game_with_tick(
        Ticker::new(),
        WindowConfig {
            title: "cubic — fixed tick".to_string(),
            width: WIDTH.into(),
            height: HEIGHT.into(),
            ..WindowConfig::default()
        },
        FixedTick::hz(60.0, MAX_FRAME_SECONDS),
    )
    .expect("runtime ended early");
}
