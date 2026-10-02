//! Platform input adapters.
//!
//! One adapter per host, each translating that host's raw events into
//! `cubic_core`'s `InputState` so gameplay and the editor see one input model:
//!
//! - [`native`] (feature `wgpu`, desktop) — `winit` `WindowEvent`s.
//! - [`web`] (feature `web`, wasm only) — DOM listeners on the canvas window.
//!
//! Each reports the cursor in the coordinate space its own backend draws in, so
//! the value lines up with the draw calls without a conversion step:
//!
//! - [`native`] reports physical pixels, matching the `wgpu` surface.
//! - [`web`] reports CSS pixels, matching the canvas 2D context.
//!
//! A game that needs to reason about both hosts must therefore treat cursor
//! position as backend-relative rather than absolute. Both adapters normalize
//! the wheel to lines so a zoom control behaves the same on either host.

#[cfg(all(feature = "wgpu", not(target_arch = "wasm32")))]
pub mod native;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub mod web;

/// Wheel motion is normalized to lines. A trackpad reports pixels and a wheel
/// mouse reports lines, so pixel deltas are divided by this to land on the same
/// scale a wheel notch would produce.
pub const PIXELS_PER_SCROLL_LINE: f32 = 50.0;
