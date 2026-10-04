//! Desktop input example: clears to a color driven by the keyboard and mouse,
//! and logs a once-per-second snapshot of the live input state.
//!
//! `update` receives two views of input, and the difference matters:
//!
//! - `InputState` is the *held* state — "is W down right now", "where is the
//!   cursor", "what are the stick axes". Read this for continuous motion.
//! - `FrameInput` is the *edge* state — what changed during the frame that was
//!   just simulated. Its edge sets are drained by the shell before the next
//!   update, so a press is observed exactly once no matter how many frames run.
//!
//! Reading `pressed` from `InputState` instead would double-count on a long
//! frame and miss a press entirely on a fast one.
//!
//! Run: `cargo run -p cubic-render --example input`

use std::collections::HashSet;

use cubic_core::input::{FrameInput, InputState, KeyCode, MouseButton};
use cubic_core::render::Rgba;
use cubic_render::app::{AppDelegate, EngineApp, WindowConfig};

/// What the log line reports on, so the snapshot stays readable.
const REPORTED: &[KeyCode] = &[
    KeyCode::W,
    KeyCode::A,
    KeyCode::S,
    KeyCode::D,
    KeyCode::Space,
    KeyCode::Shift,
    KeyCode::Escape,
];

struct InputDemo {
    /// Seconds since start, for the log cadence.
    t: f32,
    /// Next time to emit a snapshot.
    next_report: f32,
    /// Keys held right now, for the clear color.
    held: HashSet<KeyCode>,
    /// Whether the left mouse button is held.
    mouse_held: bool,
    /// Cursor position, kept to drive the color.
    mouse_x: f32,
    mouse_y: f32,
    /// Accumulated wheel, so scrolling visibly changes the color.
    scroll: f32,
}

impl InputDemo {
    fn new() -> Self {
        Self {
            t: 0.0,
            next_report: 0.0,
            held: HashSet::new(),
            mouse_held: false,
            mouse_x: 0.0,
            mouse_y: 0.0,
            scroll: 0.0,
        }
    }

    /// A one-line human-readable snapshot of everything worth seeing.
    fn report(&self, input: &InputState, frame: &FrameInput) {
        let keys: Vec<&str> = REPORTED
            .iter()
            .filter(|k| input.is_down(**k))
            .map(|k| k.name())
            .collect();

        let pressed: Vec<&str> = frame
            .pressed
            .iter()
            .filter(|k| REPORTED.contains(k))
            .map(|k| k.name())
            .collect();

        let released: Vec<&str> = frame
            .released
            .iter()
            .filter(|k| REPORTED.contains(k))
            .map(|k| k.name())
            .collect();

        let buttons: Vec<&str> = MouseButton::all()
            .iter()
            .filter(|b| input.is_mouse_down(**b))
            .map(|b| b.name())
            .collect();

        let gamepad = input.gamepad();

        log::info!(
            "held=[{}] pressed=[{}] released=[{}] mouse=[{},{}] mdelta=[{},{}] \
             scroll=[{},{}] buttons=[{}] pad_left=[{:.2},{:.2}] pad_triggers=[{:.2},{:.2}]",
            keys.join(","),
            pressed.join(","),
            released.join(","),
            input.mouse_x(),
            input.mouse_y(),
            frame.mouse_delta().x,
            frame.mouse_delta().y,
            frame.mouse_scroll_delta().x,
            frame.mouse_scroll_delta().y,
            buttons.join(","),
            gamepad.left.x,
            gamepad.left.y,
            gamepad.left_trigger,
            gamepad.right_trigger,
        );
    }
}

impl AppDelegate for InputDemo {
    fn update(&mut self, dt: f32, input: &InputState, frame: &FrameInput) {
        self.t += dt;

        // Held state, refreshed each frame. `dt` scales motion so the feel is
        // frame-rate independent.
        self.held = REPORTED
            .iter()
            .copied()
            .filter(|k| input.is_down(*k))
            .collect();
        self.mouse_held = input.is_mouse_down(MouseButton::Left);
        self.mouse_x = input.mouse_x();
        self.mouse_y = input.mouse_y();

        // Frame deltas accumulate into a position-like quantity.
        self.scroll += frame.mouse_scroll_delta().y;

        // Escape is a frame edge, so it fires once per press.
        if frame.pressed(KeyCode::Escape) {
            log::info!("escape pressed — quit by closing the window, or press again");
        }

        if self.t >= self.next_report {
            self.next_report = self.t + 1.0;
            self.report(input, frame);
        }
    }

    fn clear_color(&self) -> Rgba {
        // Held keys brighten, the cursor position tints, scrolling shifts hue,
        // and holding the left button boosts overall brightness. Every channel
        // is a direct read of live input, so the window is a visible readout.
        let lit = self.held.len() as f32 / REPORTED.len() as f32;
        let x = (self.mouse_x / 1920.0).clamp(0.0, 1.0);
        let y = (self.mouse_y / 1080.0).clamp(0.0, 1.0);
        let hue = (self.scroll * 0.05 + x * 0.15).rem_euclid(1.0);

        let value = (0.2 + 0.5 * lit + 0.3 * y).min(1.0);
        let boost = if self.mouse_held { 1.35 } else { 1.0 };
        let (r, g, b) = hsv_to_rgb(hue, 0.7, value * boost);
        Rgba::rgb(r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))
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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = WindowConfig {
        title: "cubic — input".to_string(),
        ..WindowConfig::default()
    };

    EngineApp::new(config)
        .run(InputDemo::new())
        .expect("runtime ended early");
}
