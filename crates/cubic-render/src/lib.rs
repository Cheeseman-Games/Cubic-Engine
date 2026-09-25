//! Concrete render backends for `cubic-core`'s command model.
//!
//! The command model itself (`Renderer`, `DrawList`) lives in `cubic-core`;
//! this crate flushes it to a real target:
//!
//! - `app` (feature `wgpu`, desktop) — the winit + wgpu window/surface
//!   shell.
//! - `render2d` (feature `wgpu`, desktop) — the immediate-mode 2D renderer:
//!   quad batching + color instancing over wgpu.
//! - `canvas` (feature `web`, wasm only) — the legacy 2d-canvas fallback,
//!   kept compilable until the engine fully retires it.

#[cfg(feature = "wgpu")]
#[cfg(not(target_arch = "wasm32"))]
pub mod app;

#[cfg(feature = "wgpu")]
#[cfg(not(target_arch = "wasm32"))]
pub mod render2d;

#[cfg(all(target_arch = "wasm32", feature = "web"))]
pub mod canvas;
