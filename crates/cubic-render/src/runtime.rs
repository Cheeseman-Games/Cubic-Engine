//! Windowless engine runtime: the simulation half of a host.
//!
//! [`EngineApp`](crate::EngineApp) owns a window, a surface and a GPU device;
//! none of that is needed to *advance* a game. [`EngineRuntime`] is exactly the
//! part that is: a fixed-tick clock and an input pump, feeding a delegate one
//! update per simulation step. A host that already owns a frame loop — the
//! editor running the active scene in place, a test, a headless bench — drives
//! this directly and never touches `winit` or `wgpu`.
//!
//! The clock is the same [`FixedTick`] the windowed loop uses, so the stepping
//! rule (and therefore the determinism) is identical whichever host runs it.
//! The two hosts differ only in where the input comes from, which is what
//! [`InputSource`] abstracts: `NativeInput` folds window events, a bare
//! `InputState` is what an in-process host that has no window supplies.

use cubic_core::input::{FrameInput, InputState};

use crate::app::AppDelegate;
use crate::platform::native::NativeInput;
use crate::tick::FixedTick;

/// Where an [`EngineRuntime`] gets held state and per-step edges.
///
/// The one thing a host must supply that is not already clock arithmetic. The
/// windowed shell pumps `winit` events through `NativeInput`; a host with no
/// window (the editor, a test) hands over a plain `InputState`.
pub trait InputSource {
    /// Accumulated input, for held-state queries.
    fn held(&self) -> &InputState;

    /// Drain this step's edges. Called once per simulation step, never once
    /// per presented frame, so a press that arrived between two steps is one
    /// press.
    fn begin_step(&mut self) -> FrameInput;
}

impl InputSource for InputState {
    fn held(&self) -> &InputState {
        self
    }

    fn begin_step(&mut self) -> FrameInput {
        self.begin_frame()
    }
}

impl InputSource for NativeInput {
    fn held(&self) -> &InputState {
        self.state()
    }

    fn begin_step(&mut self) -> FrameInput {
        self.begin_frame()
    }
}

/// A fixed-tick clock and an input pump, with no window or GPU.
///
/// [`advance`](Self::advance) banks real elapsed seconds and runs the delegate
/// once per whole step; [`step`](Self::step) runs exactly one, for a host that
/// wants to single-step. The default input source is a bare `InputState`, so an
/// in-process host writes `EngineRuntime::default()` and a windowed one writes
/// `EngineRuntime::new(NativeInput::new(), tick)`.
pub struct EngineRuntime<I: InputSource = InputState> {
    tick: FixedTick,
    input: I,
    /// This step's drained edges, parked outside the delegate so both the held
    /// state and the edges can be borrowed across the `update` call.
    frame: FrameInput,
    /// Simulation steps run since the last [`reset`](Self::reset).
    steps: u64,
}

impl<I: InputSource + Default> Default for EngineRuntime<I> {
    /// A runtime with a default input source on the default tick.
    fn default() -> Self {
        Self::new(I::default(), FixedTick::default())
    }
}

impl<I: InputSource> EngineRuntime<I> {
    /// A runtime with `input` on `tick`'s schedule.
    pub fn new(input: I, tick: FixedTick) -> Self {
        Self {
            tick,
            input,
            frame: FrameInput::default(),
            steps: 0,
        }
    }

    /// The input source, for held-state reads.
    pub fn input(&self) -> &I {
        &self.input
    }

    /// The input source, for folding events in.
    pub fn input_mut(&mut self) -> &mut I {
        &mut self.input
    }

    /// The clock driving simulation.
    pub fn tick(&self) -> &FixedTick {
        &self.tick
    }

    /// The clock driving simulation.
    pub fn tick_mut(&mut self) -> &mut FixedTick {
        &mut self.tick
    }

    /// Simulation steps run since the last [`reset`](Self::reset).
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// Simulation time elapsed since the last [`reset`](Self::reset), in
    /// seconds — the step count times the fixed step.
    pub fn elapsed(&self) -> f32 {
        self.steps as f32 * self.tick.step()
    }

    /// Bank `elapsed` seconds of real time and call `delegate.update` once per
    /// whole step, returning how many ran.
    ///
    /// `dt` is always [`FixedTick::step`], never the wall-clock gap, so the
    /// same elapsed sequence produces the same number of identical steps on any
    /// host.
    pub fn advance<H: AppDelegate>(&mut self, elapsed: f32, delegate: &mut H) -> u32 {
        let mut ran = 0;
        for _ in 0..self.tick.advance(elapsed) {
            self.run_step(delegate);
            ran += 1;
        }
        ran
    }

    /// Run exactly one step, ignoring the clock.
    ///
    /// The single-step path of a host that freezes play between ticks; the
    /// accumulator is left untouched, so resuming continues where pausing did.
    pub fn step<H: AppDelegate>(&mut self, delegate: &mut H) {
        self.run_step(delegate);
    }

    fn run_step<H: AppDelegate>(&mut self, delegate: &mut H) {
        let step = self.tick.step();
        // Drain per step, not per frame: a press that arrived between two steps
        // of the same frame is one press, so only the first step sees it.
        self.frame = self.input.begin_step();
        delegate.update(step, self.input.held(), &self.frame);
        self.steps += 1;
    }

    /// Drop banked time and reset the step count.
    ///
    /// Leaving play mode, or loading a new scene, is an intended break in
    /// simulation time; a stale remainder would otherwise produce a burst of
    /// catch-up steps on the next frame.
    pub fn reset(&mut self) {
        self.tick.reset();
        self.steps = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tick::MAX_FRAME_SECONDS;
    use cubic_core::render::{DrawList, Rgba};

    /// A delegate that records the `dt` of every update it is handed.
    #[derive(Default)]
    struct Recorder {
        dts: Vec<f32>,
    }

    impl AppDelegate for Recorder {
        fn update(&mut self, dt: f32, _input: &InputState, _frame: &FrameInput) {
            self.dts.push(dt);
        }

        fn draw(&mut self, _list: &mut DrawList) {}

        fn clear_color(&self) -> Rgba {
            Rgba::rgb(0.0, 0.0, 0.0)
        }
    }

    #[test]
    fn the_default_source_is_a_bare_input_state() {
        let runtime = EngineRuntime::<InputState>::default();
        assert_eq!(runtime.steps(), 0);
        assert_eq!(runtime.elapsed(), 0.0);
    }

    #[test]
    fn advance_runs_one_update_per_whole_step_with_the_fixed_dt() {
        // A 1/16 s step, so three of them are exactly representable.
        let mut runtime =
            EngineRuntime::new(InputState::new(), FixedTick::new(0.0625, MAX_FRAME_SECONDS));
        let mut recorder = Recorder::default();

        let ran = runtime.advance(0.1875, &mut recorder);

        assert_eq!(ran, 3);
        assert_eq!(recorder.dts, vec![0.0625, 0.0625, 0.0625]);
        assert_eq!(runtime.steps(), 3);
        assert!((runtime.elapsed() - 0.1875).abs() < 1e-5);
    }

    #[test]
    fn a_frame_too_short_for_a_step_runs_nothing() {
        let mut runtime = EngineRuntime::<InputState>::default();
        let mut recorder = Recorder::default();

        assert_eq!(runtime.advance(1.0 / 240.0, &mut recorder), 0);
        assert!(recorder.dts.is_empty());
    }

    #[test]
    fn step_runs_exactly_one_update_and_ignores_the_clock() {
        let mut runtime = EngineRuntime::<InputState>::default();
        let mut recorder = Recorder::default();

        runtime.step(&mut recorder);
        runtime.step(&mut recorder);

        assert_eq!(recorder.dts, vec![1.0 / 60.0, 1.0 / 60.0]);
        assert_eq!(runtime.steps(), 2);
        assert_eq!(runtime.tick().leftover(), 0.0);
    }

    #[test]
    fn reset_drops_banked_time_and_the_step_count() {
        let mut runtime = EngineRuntime::<InputState>::default();
        let mut recorder = Recorder::default();
        for _ in 0..3 {
            runtime.advance(1.0 / 60.0, &mut recorder);
        }
        assert_eq!(runtime.steps(), 3);

        runtime.reset();

        assert_eq!(runtime.steps(), 0);
        assert_eq!(runtime.tick().leftover(), 0.0);
    }
}
