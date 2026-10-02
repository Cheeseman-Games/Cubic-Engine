//! Concrete render backends for `cubic-core`'s command model.
//!
//! The command model itself (`Renderer`, `DrawList`) lives in `cubic-core`;
//! this crate flushes it to a real target:
//!
//! - `app` (feature `wgpu`, desktop) — the winit + wgpu window/surface
//!   shell.
//! - `platform` — per-host input adapters: `platform::native` (winit,
//!   desktop) and `platform::web` (DOM, wasm only).
//! - `render2d` (feature `wgpu`, desktop) — the immediate-mode 2D renderer:
//!   quad batching + color instancing over wgpu.
//! - `text` (feature `wgpu`, desktop) — glyph-atlas text for the same command
//!   model: shaping, atlas caching, and the layout cache behind it.
//! - `canvas` (feature `web`, wasm only) — the legacy 2d-canvas fallback,
//!   kept compilable until the engine fully retires it.

#[cfg(feature = "wgpu")]
#[cfg(not(target_arch = "wasm32"))]
pub mod app;

pub mod platform;

#[cfg(feature = "wgpu")]
#[cfg(not(target_arch = "wasm32"))]
pub mod render2d;

#[cfg(feature = "wgpu")]
#[cfg(not(target_arch = "wasm32"))]
pub mod text;

#[cfg(all(target_arch = "wasm32", feature = "web"))]
pub mod canvas;
