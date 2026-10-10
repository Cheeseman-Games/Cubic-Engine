//! Minimal entity/component + input + render command core behind
//! Cube-Combat.
//!
//! `cubic-core` is GPU-free and backend-agnostic: gameplay emits
//! `DrawCommand`s into a `DrawList` each frame and backends
//! (`cubic-render`'s canvas/WGPU flush) consume it. Simulation is a
//! fixed-tick pipeline of `System`s over a `World`.
//!
//! The `rendering` feature (on by default) provides the render command
//! model; disable it (`--no-default-features`) for a headless,
//! dependency-free sim core. The `manifest` feature adds the `game.toml`
//! project manifest on top of it, the `scene` feature adds the `.rsn`
//! `Scene` model for loading and saving levels, and the `reflect` feature
//! adds component reflection for the editor's inspector.
//!
//! Game crates import [`prelude`] (or `cubic_render::prelude`, which adds the
//! runtime) rather than naming modules.

pub mod assets;
pub mod components;
pub mod hierarchy;
pub mod input;
#[cfg(feature = "manifest")]
pub mod manifest;
pub mod math;
#[cfg(feature = "reflect")]
pub mod reflect;
#[cfg(feature = "rendering")]
pub mod render;
#[cfg(feature = "scene")]
pub mod scene;
pub mod world;

// The reflection derives reach this crate by its own name, exactly as a game
// crate does, so that a `#[derive(Inspectable)]` inside `cubic-core` expands
// the same way one outside it does.
#[cfg(feature = "reflect")]
extern crate self as cubic_core;

pub mod prelude;

#[cfg(feature = "rendering")]
use crate::assets::AssetServer;
pub use crate::components::Transform;
use crate::input::{FrameInput, InputState};
#[cfg(feature = "rendering")]
use crate::render::Rgba;
use crate::world::World;

/// Everything a `System` may touch while running each tick.
pub struct TickContext<'a> {
    pub world: &'a mut World,
    pub input: &'a InputState,
    pub frame: &'a FrameInput,
    pub dt: f32,
}

/// A discrete gameplay step that reads from / writes to the `World`.
///
/// Systems are the extension point of this engine: add a new `System`
/// implementation and register it in `Game::new` to grow the game.
pub trait System {
    fn run(&mut self, ctx: &mut TickContext<'_>);
}

/// The game trait the engine-hosted runtime calls into.
///
/// A game crate implements this once and hands an instance to whatever hosts
/// it — `engine_main!` on the desktop, a platform runner on wasm. It is the
/// whole host-facing surface: constructing the value is the setup step,
/// `update` advances the simulation, `draw` emits the frame's commands. Hosts
/// own the window, the loop and the GPU, so game code never touches platform
/// APIs.
///
/// Every method has a default, so the smallest legal game is just `impl Game
/// for MyGame {}`.
#[cfg(feature = "rendering")]
pub trait Game {
    /// Advance the simulation by `dt` seconds.
    ///
    /// `input` carries held state, `frame` this tick's edges — the same split
    /// `TickContext` gives a `System`, so an update body can drop straight
    /// into one.
    fn update(&mut self, _dt: f32, _input: &InputState, _frame: &FrameInput) {}

    /// Emit this frame's draw commands. The host resets the list before every
    /// call, so implementations only ever push.
    fn draw(&mut self, _list: &mut crate::render::DrawList) {}

    /// This game's asset server, for a host that pumps it.
    ///
    /// `None` by default: a game with no assets says nothing, and the host skips
    /// the work. A game that returns its server hands the runtime two things it
    /// must not do for itself — re-reading changed files, and importing queued
    /// bytes into the backend — both of which need the host's device and its
    /// frame. Everything else stays the game's: it calls
    /// [`drain_events`](AssetServer::drain_events) to hear what happened, and
    /// reads [`version`](AssetServer::version) to notice a reload.
    fn assets_mut(&mut self) -> Option<&mut AssetServer> {
        None
    }

    /// Backbuffer fill for frames whose `draw` pushed no `Clear` of its own.
    ///
    /// Black by default: a game that wants another backdrop clears explicitly.
    fn clear_color(&self) -> Rgba {
        Rgba::rgb(0.0, 0.0, 0.0)
    }
}
