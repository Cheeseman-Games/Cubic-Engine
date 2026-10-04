//! Game-facing prelude.
//!
//! One glob import gives a game the entire engine surface: the ECS, math,
//! input, the render command model and the lifecycle traits from
//! `cubic-core`, plus the desktop runtime that turns them into a running
//! window. A game crate imports this and nothing else from the engine:
//!
//! ```
//! use cubic_render::prelude::*;
//! ```
//!
//! On wasm — or with the `wgpu` feature off — the runtime half drops out and
//! this module is exactly `cubic_core::prelude`, so the same game code
//! compiles for the canvas backend.

pub use cubic_core::prelude::*;

/// The fixed-timestep clock the runtime advances gameplay by. A game rarely
/// constructs one — [`engine_main!`] supplies a 60 Hz default — but the rate is
/// part of the game-facing contract, so it is reachable from the same glob.
pub use crate::tick::{DEFAULT_TICK_HZ, FixedTick, MAX_FRAME_SECONDS};

/// The desktop runtime: the window shell a game is hosted in and the macro
/// that generates its `main`. Absent without the `wgpu` feature, or on wasm.
#[cfg(feature = "wgpu")]
#[cfg(not(target_arch = "wasm32"))]
pub use crate::{
    AppDelegate, AppError, EngineApp, GameDelegate, WindowConfig, engine_main, run_game,
    run_game_with_tick,
};

/// The project manifest — the `game.toml` a project is described by — plus the
/// entry point that runs a game from one. Absent without the `manifest` feature.
#[cfg(feature = "manifest")]
pub use crate::manifest::{ManifestError, ProjectManifest};

#[cfg(all(feature = "manifest", feature = "wgpu", not(target_arch = "wasm32")))]
pub use crate::run_project;

#[cfg(all(test, feature = "wgpu", not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    /// The smallest legal game: no `update`, no `draw`, no fields. It still has
    /// to satisfy every bound `engine_main!` relies on.
    struct Nothing;

    impl Game for Nothing {}

    /// A ~30-line game built only from this prelude — the shape every template
    /// project is written in.
    struct Hello {
        t: f32,
    }

    impl Game for Hello {
        fn update(&mut self, dt: f32, _input: &InputState, _frame: &FrameInput) {
            // Always the fixed step, whatever the display is doing.
            self.t += dt;
        }

        fn draw(&mut self, list: &mut DrawList) {
            list.clear(Rgba::rgb(0.06, 0.08, 0.12));
            list.fill_rect(40.0, 40.0, 100.0 + self.t, 60.0, Rgba::rgb(1.0, 0.5, 0.2));
            list.text(
                &format!("t = {:.1}", self.t),
                40.0,
                130.0,
                24.0,
                Rgba::rgb(1.0, 1.0, 1.0),
            );
        }
    }

    /// The prelude must reach every layer a game touches: ECS, math, input,
    /// the command model and the runtime entry points.
    #[test]
    fn the_prelude_covers_every_layer_a_game_touches() {
        let mut world = World::new();
        let id = world.spawn();
        world.insert(id, Transform::from_position(Vec2::new(1.0, 2.0)));
        assert_eq!(
            world.get::<Transform>(id).unwrap().position,
            Vec2::new(1.0, 2.0)
        );

        let mut input = InputState::new();
        input.key_down(KeyCode::W);
        assert!(input.begin_frame().pressed(KeyCode::W));

        let mut list = DrawList::new();
        list.fill_rect(0.0, 0.0, 1.0, 1.0, Rgba::rgb(1.0, 1.0, 1.0));
        assert_eq!(list.commands.len(), 1);

        let rect = Rect::new(0.0, 0.0, 2.0, 2.0);
        assert!(rect.contains(1.0, 1.0));

        let window = WindowConfig::default();
        assert_eq!(window.clear_color, Rgba::rgb(0.07, 0.09, 0.13));
    }

    /// The tick rate a game is handed is the documented default, and the type a
    /// manifest-driven host overrides it with.
    #[test]
    fn the_prelude_names_the_runtime_tick_rate() {
        assert_eq!(DEFAULT_TICK_HZ, 60.0);
        assert_eq!(FixedTick::default().step(), 1.0 / 60.0);
        assert_eq!(MAX_FRAME_SECONDS, 0.25);
    }

    #[test]
    fn an_empty_game_impl_is_legal() {
        let delegate = GameDelegate::new(Nothing);
        assert_eq!(delegate.clear_color(), Rgba::rgb(0.0, 0.0, 0.0));
    }

    #[test]
    fn a_prelude_only_game_can_be_handed_to_the_shell() {
        let mut delegate = GameDelegate::new(Hello { t: 0.0 });
        delegate.update(0.5, &InputState::new(), &FrameInput::default());
        let mut list = DrawList::new();
        delegate.draw(&mut list);
        assert_eq!(list.commands.len(), 3);
    }
}
