//! Concrete render backends for `cubic-core`'s command model.
//!
//! The command model itself (`Renderer`, `DrawList`) and the lifecycle traits
//! (`System`, `Game`) live in `cubic-core`; this crate flushes the model to a
//! real target and hosts games:
//!
//! - `app` (feature `wgpu`, desktop) — the winit + wgpu window/surface shell
//!   plus the `engine_main!` entry point a game crate generates its `main`
//!   from.
//! - `tick` — the fixed-timestep accumulator the runtime advances gameplay by.
//!   Plain arithmetic, so it builds for any target and any feature set.
//! - `platform` — per-host input adapters: `platform::native` (winit,
//!   desktop) and `platform::web` (DOM, wasm only).
//! - `render2d` (feature `wgpu`, desktop) — the immediate-mode 2D renderer:
//!   quad batching + color instancing over wgpu.
//! - `text` (feature `wgpu`, desktop) — glyph-atlas text for the same command
//!   model: shaping, atlas caching, and the layout cache behind it.
//! - `canvas` (feature `web`, wasm only) — the legacy 2d-canvas fallback,
//!   kept compilable until the engine fully retires it.
//!
//! A game crate imports [`prelude`] and writes one `Game` impl; it should never
//! name `winit`, `wgpu` or `cubic_core` directly.
//!
//! With the `manifest` feature the [`manifest`] module is re-exported from
//! `cubic_core`: that is the `game.toml` a project is described by, and
//! [`run_project`] is the entry point that turns one into a running game.

pub mod tick;

#[cfg(feature = "manifest")]
pub use cubic_core::manifest;

#[cfg(feature = "wgpu")]
#[cfg(not(target_arch = "wasm32"))]
pub mod app;

pub mod platform;

pub mod prelude;

#[cfg(feature = "wgpu")]
#[cfg(not(target_arch = "wasm32"))]
pub mod render2d;

#[cfg(feature = "wgpu")]
#[cfg(not(target_arch = "wasm32"))]
pub mod text;

#[cfg(all(target_arch = "wasm32", feature = "web"))]
pub mod canvas;

// The runtime entry points sit at the crate root so `engine_main!` can reach
// them through `$crate` without a version-specific path. `engine_main!` itself
// is already at the root: `#[macro_export]` hoists it out of `app`.
pub use tick::{DEFAULT_TICK_HZ, FixedTick, MAX_FRAME_SECONDS};

#[cfg(feature = "wgpu")]
#[cfg(not(target_arch = "wasm32"))]
pub use app::{
    AppDelegate, AppError, EngineApp, GameDelegate, WindowConfig, run_game, run_game_with_tick,
};

#[cfg(feature = "manifest")]
#[cfg(feature = "wgpu")]
#[cfg(not(target_arch = "wasm32"))]
pub use app::run_project;
