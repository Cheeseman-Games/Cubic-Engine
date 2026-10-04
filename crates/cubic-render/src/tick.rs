//! Fixed-timestep accumulator: the deterministic half of the frame loop.
//!
//! Rendering is free-running — a vsync-locked display decides how often a frame
//! arrives — but simulation is not. Advancing gameplay by the wall-clock delta
//! of whatever frame happened to land makes the same playthrough produce
//! different results on a 60 Hz laptop and a 144 Hz monitor, and makes a slow
//! frame teleport everything at once.
//!
//! [`FixedTick`] splits the two concerns. Host code measures real time and hands
//! the elapsed seconds to [`FixedTick::advance`], which answers how many whole
//! simulation steps that time is worth. The runtime then runs the game's update
//! exactly that many times, each with the *same* `dt`. Leftover time is kept, so
//! the step boundaries land where they would have on a display running at any
//! other rate.
//!
//! ```
//! use cubic_render::tick::{FixedTick, MAX_FRAME_SECONDS};
//!
//! let mut tick = FixedTick::hz(60.0, MAX_FRAME_SECONDS);
//!
//! // A 144 Hz display: some frames owe nothing, some owe a step.
//! assert_eq!(tick.advance(1.0 / 144.0), 0);
//! assert_eq!(tick.advance(1.0 / 144.0), 0);
//! assert_eq!(tick.advance(1.0 / 144.0), 1);
//!
//! // A 30 Hz display: two steps per frame, same dt either way.
//! let mut slow = FixedTick::hz(60.0, MAX_FRAME_SECONDS);
//! assert_eq!(slow.advance(1.0 / 30.0), 2);
//! assert_eq!(slow.step(), 1.0 / 60.0);
//! ```
//!
//! The accumulator is plain arithmetic over an `f64` carried between frames, so
//! the same sequence of elapsed times always yields the same tick count — that
//! is the property gameplay tests rely on. It is deliberately free of window,
//! GPU and input types: anything that drives a game (a windowed host, the
//! headless bench, an editor) can own one.

/// Steps per second a [`FixedTick`] defaults to.
///
/// 60 Hz is the rate a fighting game is balanced at and the rate a display
/// refresh usually locks to, so the accumulator hands out one step per frame.
pub const DEFAULT_TICK_HZ: f32 = 60.0;

/// Longest real interval [`FixedTick::advance`] will believe in.
///
/// After a stall — a breakpoint, a level load, the window being dragged — the
/// measured gap is far larger than any step. Crediting all of it would make the
/// runtime run hundreds of catch-up steps on the next frame, each one costing a
/// frame's work, so the game would fall further behind the harder it tried to
/// catch up (a spiral of death). Clamping trades a small amount of simulation
/// time for a loop that always recovers.
pub const MAX_FRAME_SECONDS: f32 = 0.25;

/// The step a [`FixedTick`] falls back to when asked for a non-positive one.
const FALLBACK_STEP: f32 = 1.0 / DEFAULT_TICK_HZ;

/// Converts real elapsed seconds into whole simulation steps.
///
/// See the [module docs](self) for why the split exists. Construct one with
/// [`FixedTick::hz`] (from a tick rate) or [`FixedTick::new`] (from a step in
/// seconds); both replace nonsensical arguments with the engine defaults rather
/// than dividing by zero later.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FixedTick {
    step: f32,
    max_frame: f32,
    accumulator: f64,
}

impl Default for FixedTick {
    /// 60 Hz with the standard [`MAX_FRAME_SECONDS`] clamp.
    fn default() -> Self {
        Self::hz(DEFAULT_TICK_HZ, MAX_FRAME_SECONDS)
    }
}

impl FixedTick {
    /// A clock stepping at `hz` ticks per second, clamping real gaps to
    /// `max_frame` seconds.
    pub fn hz(hz: f32, max_frame: f32) -> Self {
        Self::new(1.0 / positive(hz, DEFAULT_TICK_HZ), max_frame)
    }

    /// A clock stepping every `step` seconds, clamping real gaps to
    /// `max_frame` seconds.
    ///
    /// A non-positive or non-finite argument is replaced with the engine default
    /// instead of being rejected, so a bad rate from a project manifest
    /// downgrades to 60 Hz rather than freezing or stalling the game.
    pub fn new(step: f32, max_frame: f32) -> Self {
        Self {
            step: positive(step, FALLBACK_STEP),
            max_frame: positive(max_frame, MAX_FRAME_SECONDS),
            accumulator: 0.0,
        }
    }

    /// Seconds one step advances the simulation by — the `dt` every update call
    /// receives.
    pub fn step(&self) -> f32 {
        self.step
    }

    /// The steps per second this clock runs at.
    pub fn rate(&self) -> f32 {
        1.0 / self.step
    }

    /// Longest real interval this clock credits, in seconds.
    pub fn max_frame(&self) -> f32 {
        self.max_frame
    }

    /// Bank the elapsed real time and report how many whole steps it is worth.
    ///
    /// `elapsed` is clamped to `0..=max_frame` and added to the leftover from
    /// previous frames; the returned count is then removed from the bank, leaving
    /// the sub-step remainder for next time. Zero means "this frame has not
    /// earned a step yet" — call again on the next frame rather than treating it
    /// as a missed update.
    pub fn advance(&mut self, elapsed: f32) -> u32 {
        // `f32::clamp` passes a NaN straight through, and one NaN would poison the
        // bank for the rest of the process. An infinite gap is not a NaN though —
        // it is just a very large stall, so it clamps like any other.
        let elapsed = if elapsed.is_nan() {
            0.0
        } else {
            elapsed.clamp(0.0, self.max_frame)
        };
        self.accumulator += elapsed as f64;

        // `accumulator` is bounded by `max_frame` plus one step, so this stays a
        // small count; the cast saturates rather than wrapping even if a caller
        // configures an absurd rate.
        let ticks = (self.accumulator / self.step as f64).floor() as u32;
        // Subtract in one go rather than a loop, and clamp at zero: with the two
        // quantities this close, rounding can leave a negative residue that would
        // otherwise swallow part of a later step.
        let spent = ticks as f64 * self.step as f64;
        self.accumulator = (self.accumulator - spent).max(0.0);
        ticks
    }

    /// Time banked toward a step that has not been earned yet, in seconds.
    ///
    /// Always in `0..step`. Exposed for tests and for a frame-time readout in an
    /// editor status bar.
    pub fn leftover(&self) -> f64 {
        self.accumulator
    }

    /// Drop banked time, so the next [`advance`](Self::advance) starts from zero.
    ///
    /// The escape hatch for a host that deliberately abandons simulation time —
    /// skipping ahead after a load, or leaving play mode — without a stale
    /// remainder producing a burst of catch-up steps.
    pub fn reset(&mut self) {
        self.accumulator = 0.0;
    }
}

/// Guard a configuration value, falling back when it is not a usable positive
/// finite number.
fn positive(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 60 Hz clock. Used where the assertion is exact: passing the step itself
    /// as the elapsed time always yields exactly one tick.
    fn clock() -> FixedTick {
        FixedTick::new(1.0 / 60.0, MAX_FRAME_SECONDS)
    }

    /// A 16 Hz clock whose step is a power of two, so arbitrary frame times
    /// written as fractions of a second are *exact* in `f32`. Multi-step
    /// assertions use this to avoid tests that hinge on rounding at a step
    /// boundary.
    fn exact() -> FixedTick {
        FixedTick::new(0.0625, MAX_FRAME_SECONDS)
    }

    /// Drain a clock over a list of real frame times, returning the per-frame
    /// tick counts and the remainder left behind.
    fn run(tick: &mut FixedTick, frames: &[f32]) -> (Vec<u32>, f64) {
        let counts = frames.iter().map(|&dt| tick.advance(dt)).collect();
        (counts, tick.leftover())
    }

    /// Total ticks `frames` is worth to a fresh clock.
    fn total(mut tick: FixedTick, frames: &[f32]) -> u32 {
        frames.iter().map(|&dt| tick.advance(dt)).sum()
    }

    /// Assert a rate to within a thousandth. The step is stored as an `f32`, so
    /// inverting it cannot land on the original rate exactly — the same rounding
    /// that makes 1/60 not quite a divisor of 1.0.
    fn assert_rate(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-3,
            "{actual} is not within a thousandth of {expected}"
        );
    }

    #[test]
    fn the_default_is_sixty_hertz() {
        let tick = FixedTick::default();
        assert_rate(tick.rate(), DEFAULT_TICK_HZ);
        assert_eq!(tick.step(), 1.0 / 60.0);
        assert_eq!(tick.max_frame(), MAX_FRAME_SECONDS);
        assert_eq!(tick.leftover(), 0.0);
    }

    #[test]
    fn an_exactly_one_step_frame_runs_exactly_one_tick() {
        assert_eq!(clock().advance(1.0 / 60.0), 1);
    }

    /// The display outrunning the tick rate must idle, not interpolate: the
    /// sub-step remainder is banked until it becomes a whole step.
    #[test]
    fn a_fast_display_banks_the_remainder_until_it_completes_a_step() {
        // A quarter of a step per frame: four frames, one update.
        let (counts, leftover) = run(&mut exact(), &[0.015625; 4]);
        assert_eq!(counts, vec![0, 0, 0, 1]);
        assert_eq!(leftover, 0.0);
    }

    /// The other half of the same trade: a slow display earns several ticks in one
    /// frame, each with the same `dt`, so the simulation keeps real-time pace.
    #[test]
    fn a_slow_display_earns_several_ticks_in_one_frame() {
        let mut tick = exact();
        assert_eq!(tick.advance(0.25), 4);
        assert_eq!(tick.leftover(), 0.0);
        assert_eq!(tick.step(), 0.0625);
    }

    /// A frame that leaves a remainder must not lose it: the bank is what makes
    /// the average rate exact rather than merely close.
    #[test]
    fn a_partial_step_is_carried_into_the_next_frame() {
        let mut tick = exact();
        assert_eq!(tick.advance(0.03), 0);
        assert!((tick.leftover() - 0.03).abs() < 1e-6, "{}", tick.leftover());

        // The next frame completes the step, and only the excess is left over.
        assert_eq!(tick.advance(0.04), 1);
        assert!(
            (tick.leftover() - 0.0075).abs() < 1e-6,
            "{}",
            tick.leftover()
        );
    }

    /// The determinism contract: a span of simulation time yields the same number
    /// of steps however it was cut into frames.
    #[test]
    fn the_tick_count_does_not_depend_on_how_the_time_was_framed() {
        // A quarter-second of play, delivered in one lump, in eight equal slices,
        // and in six ragged ones.
        let coarse = total(exact(), &[0.25]);
        let fine = total(exact(), &[0.03125; 8]);
        let ragged = total(
            exact(),
            &[0.0078125, 0.0390625, 0.015625, 0.09375, 0.0625, 0.03125],
        );

        assert_eq!(coarse, 4);
        assert_eq!(fine, coarse);
        assert_eq!(ragged, coarse);
    }

    /// Ten seconds of play advances 600 steps at 60 Hz from any refresh rate.
    #[test]
    fn the_tick_count_tracks_elapsed_time_not_the_frame_rate() {
        for frames in [
            vec![1.0 / 60.0; 600],
            vec![1.0 / 144.0; 1440],
            vec![1.0 / 240.0; 2400],
        ] {
            let ticks = total(clock(), &frames);
            // A boundary case can round one way or the other, but never by more
            // than a single step.
            assert!(ticks.abs_diff(600) <= 1, "{ticks} ticks from 10 seconds");
        }
    }

    /// The same frame times in the same order always give the same answer — the
    /// property a recorded-input replay depends on.
    #[test]
    fn replaying_the_same_frame_times_is_bit_identical() {
        let frames: Vec<f32> = (0..200).map(|i| 0.004 + (i % 7) as f32 * 0.0031).collect();
        assert_eq!(run(&mut clock(), &frames), run(&mut clock(), &frames));
    }

    /// A frame that ran long — a stall, a load — is credited only up to the
    /// clamp, so recovery does not become a catch-up stampede.
    #[test]
    fn a_stall_is_clamped_instead_of_banked() {
        let mut tick = clock();
        let mut reference = clock();

        // A 30-second stall is worth exactly what the clamp says it is.
        assert_eq!(tick.advance(30.0), reference.advance(MAX_FRAME_SECONDS));
        assert!(tick.leftover() < 1.0 / 60.0);

        // And the next frame resumes from a sane place, not from a huge debt.
        assert_eq!(tick.advance(1.0 / 60.0), 1);
    }

    /// A negative delta (a clock that went backwards) must not pay back time.
    #[test]
    fn a_negative_interval_banks_nothing() {
        let mut tick = clock();
        assert_eq!(tick.advance(-5.0), 0);
        assert_eq!(tick.leftover(), 0.0);
    }

    /// A NaN would poison the bank forever if it were not screened; an infinite
    /// gap is just a very large stall and clamps like any other.
    #[test]
    fn a_non_finite_interval_does_not_poison_the_bank() {
        let mut tick = clock();

        assert_eq!(tick.advance(f32::NAN), 0);
        assert_eq!(tick.leftover(), 0.0);

        let mut reference = clock();
        assert_eq!(
            tick.advance(f32::INFINITY),
            reference.advance(MAX_FRAME_SECONDS)
        );
    }

    /// The remainder is what makes sub-step time survive, so it must stay in
    /// range no matter the frame times.
    #[test]
    fn the_remainder_always_stays_below_one_step() {
        let mut tick = clock();
        for i in 0..500 {
            tick.advance(0.0007 * (i + 1) as f32);
            assert!(
                (0.0..tick.step() as f64).contains(&tick.leftover()),
                "leftover {} escaped the step at frame {i}",
                tick.leftover()
            );
        }
    }

    /// An hour of frames must not drift: 216_000 steps is where an `f32` bank
    /// would start losing steps.
    #[test]
    fn an_hour_of_frames_does_not_drift() {
        let mut tick = clock();
        for _ in 0..3600 * 60 {
            assert_eq!(tick.advance(1.0 / 60.0), 1);
        }
        assert_eq!(tick.leftover(), 0.0);
    }

    /// A step with no exact binary representation still lands exactly on itself,
    /// which is where rounding would otherwise accumulate.
    #[test]
    fn a_step_that_is_not_a_binary_fraction_stays_exact_against_itself() {
        let mut tick = FixedTick::new(0.1, MAX_FRAME_SECONDS);
        for _ in 0..1000 {
            assert_eq!(tick.advance(0.1), 1);
        }
        assert_eq!(tick.leftover(), 0.0);
    }

    #[test]
    fn reset_drops_banked_time() {
        let mut tick = clock();
        tick.advance(1.5 / 60.0);
        assert!(tick.leftover() > 0.0);
        tick.reset();
        assert_eq!(tick.leftover(), 0.0);
        assert_eq!(tick.advance(0.5 / 60.0), 0);
    }

    /// A manifest that says "0 Hz" must not divide by zero in the frame loop.
    #[test]
    fn nonsensical_arguments_fall_back_to_the_defaults() {
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let mut tick = FixedTick::hz(bad, bad);
            assert_eq!(tick.step(), 1.0 / 60.0, "hz({bad}) was not sanitized");
            assert_eq!(tick.max_frame(), MAX_FRAME_SECONDS);
            assert_eq!(tick.advance(1.0 / 60.0), 1);
        }

        let tick = FixedTick::new(-2.0, 0.0);
        assert_eq!(tick.step(), 1.0 / 60.0);
        assert_eq!(tick.max_frame(), MAX_FRAME_SECONDS);
    }

    #[test]
    fn an_arbitrary_rate_round_trips() {
        let mut tick = FixedTick::hz(120.0, MAX_FRAME_SECONDS);
        assert_rate(tick.rate(), 120.0);
        assert_eq!(tick.advance(1.0 / 120.0), 1);
    }
}
