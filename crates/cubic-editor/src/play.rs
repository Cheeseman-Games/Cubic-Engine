//! Play mode: running the open scene in place, through the engine's runtime.
//!
//! The editor is only an editing surface until a scene can be *played*. This
//! module is that bridge: it owns an [`EngineRuntime`] and drives the open
//! scene's [`World`](cubic_core::world::World) through it, so Play, Pause, Step
//! and Stop act on the same world the viewport draws — no window, no second
//! process, no round-trip through a file.
//!
//! # Editing and playing are the same scene
//!
//! Play mutates the open scene's world in place, which is what makes the run
//! visible immediately in the viewport. Stop must therefore put the edited
//! state back: entering play takes a [`Scene`] snapshot of the world first and
//! restores it on the way out, so a play/stop cycle leaves the scene exactly as
//! it was found. The snapshot is the same `Scene` save contract the `.rsn`
//! files use, so what is restored is what would have been saved.
//!
//! # What runs
//!
//! The step body is an empty system pipeline today — the editor's scenes carry
//! only [`Transform`](cubic_core::components::Transform) and the engine has no
//! gameplay systems yet. [`Play::add_system`] is the seam: the runtime and the
//! transport around it are the bridge this delivers, and a project's own
//! systems drop into the pipeline later without the controls changing.

use cubic_core::input::{FrameInput, InputState};
use cubic_core::render::{DrawList, Rgba};
use cubic_core::scene::{Scene, SceneRegistry};
use cubic_core::world::World;
use cubic_core::{System, TickContext};
use cubic_render::{AppDelegate, EngineRuntime, FixedTick};

use crate::state::{EditorState, OpenScene};

/// Whether the active scene is being simulated, and if so whether it is frozen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Playback {
    /// Editable; simulation is not running.
    #[default]
    Stopped,
    /// Simulating, advanced once per fixed step.
    Playing,
    /// Simulating, frozen between steps — what Step advances.
    Paused,
}

/// Play mode's engine side: the runtime, the edit-state snapshot and the system
/// pipeline the steps run.
pub struct Play {
    playback: Playback,
    /// The fixed-tick clock and input pump driving the steps.
    runtime: EngineRuntime,
    /// The world as it was when play began, restored by [`stop`](Self::stop).
    /// `None` whenever playback is stopped.
    snapshot: Option<Scene>,
    /// The systems each step runs. Empty until gameplay systems are hostable.
    systems: Vec<Box<dyn System>>,
}

impl Default for Play {
    fn default() -> Self {
        Self {
            playback: Playback::Stopped,
            runtime: EngineRuntime::new(InputState::new(), FixedTick::default()),
            snapshot: None,
            systems: Vec::new(),
        }
    }
}

impl Play {
    /// The current transport state.
    pub fn state(&self) -> Playback {
        self.playback
    }

    pub fn is_stopped(&self) -> bool {
        self.playback == Playback::Stopped
    }

    pub fn is_playing(&self) -> bool {
        self.playback == Playback::Playing
    }

    pub fn is_paused(&self) -> bool {
        self.playback == Playback::Paused
    }

    /// Whether play mode is entered at all, paused or running.
    pub fn is_active(&self) -> bool {
        self.playback != Playback::Stopped
    }

    /// Simulation steps run since this run of play began.
    pub fn steps(&self) -> u64 {
        self.runtime.steps()
    }

    /// Simulation time run since this run of play began, in seconds.
    pub fn elapsed(&self) -> f32 {
        self.runtime.elapsed()
    }

    /// Append a system to the step pipeline.
    ///
    /// The editor registers none itself yet; a test — or, later, a project's
    /// gameplay systems — uses this to give Play something to run.
    pub fn add_system(&mut self, system: Box<dyn System>) {
        self.systems.push(system);
    }

    /// Enter play mode: snapshot the edited world on first entry, then run.
    ///
    /// Re-entering from Paused just resumes; a fresh run from Stopped takes a
    /// new snapshot, so Stop always restores the state the latest Play began
    /// with. With no scene open there is nothing to run.
    pub fn play(
        &mut self,
        scene: &mut Option<OpenScene>,
        registry: &SceneRegistry,
    ) -> Result<(), String> {
        if self.is_playing() {
            return Ok(());
        }
        if self.is_stopped() {
            self.snapshot(scene, registry)?;
            self.runtime.reset();
        }
        self.playback = Playback::Playing;
        Ok(())
    }

    /// Freeze the running simulation without leaving play mode.
    pub fn pause(&mut self) {
        if self.is_playing() {
            self.playback = Playback::Paused;
        }
    }

    /// Resume a paused simulation.
    pub fn resume(&mut self) {
        if self.is_paused() {
            self.playback = Playback::Playing;
        }
    }

    /// Advance exactly one step while frozen.
    ///
    /// From Stopped it begins a paused run — snapshotting the world first — so
    /// a stepped scene can still be restored by Stop.
    pub fn step(
        &mut self,
        scene: &mut Option<OpenScene>,
        registry: &SceneRegistry,
    ) -> Result<(), String> {
        if self.is_playing() {
            return Ok(());
        }
        if self.is_stopped() {
            self.snapshot(scene, registry)?;
            self.runtime.reset();
            self.playback = Playback::Paused;
        }
        let Some(open) = scene.as_mut() else {
            return Err("no scene is open".to_owned());
        };
        let mut delegate = SceneDelegate {
            world: &mut open.world,
            systems: &mut self.systems,
        };
        self.runtime.step(&mut delegate);
        Ok(())
    }

    /// Leave play mode and restore the world as it was when Play began.
    pub fn stop(
        &mut self,
        scene: &mut Option<OpenScene>,
        registry: &SceneRegistry,
    ) -> Result<(), String> {
        if let (Some(snapshot), Some(open)) = (self.snapshot.take(), scene.as_mut()) {
            let loaded = snapshot
                .to_world(registry)
                .map_err(|error| error.to_string())?;
            open.world = loaded.world;
        }
        self.runtime.reset();
        self.playback = Playback::Stopped;
        Ok(())
    }

    /// Run `elapsed` seconds of simulation, if playing.
    ///
    /// The per-frame call from the shell: a no-op while stopped or paused, so a
    /// frozen scene costs nothing and its clock stays put.
    pub fn advance(&mut self, scene: &mut Option<OpenScene>, elapsed: f32) {
        if !self.is_playing() {
            return;
        }
        let Some(open) = scene.as_mut() else {
            return;
        };
        let mut delegate = SceneDelegate {
            world: &mut open.world,
            systems: &mut self.systems,
        };
        self.runtime.advance(elapsed, &mut delegate);
    }

    /// Drop the snapshot and return to stopped *without* restoring it.
    ///
    /// For when the scene underneath is about to be replaced or closed: there
    /// is nothing to restore into, and the next Play takes a fresh snapshot.
    pub fn cancel(&mut self) {
        self.snapshot = None;
        self.runtime.reset();
        self.playback = Playback::Stopped;
    }

    fn snapshot(
        &mut self,
        scene: &Option<OpenScene>,
        registry: &SceneRegistry,
    ) -> Result<(), String> {
        let open = scene
            .as_ref()
            .ok_or_else(|| "no scene is open".to_owned())?;
        let snapshot =
            Scene::from_world(&open.world, registry).map_err(|error| error.to_string())?;
        self.snapshot = Some(snapshot);
        Ok(())
    }
}

/// Run a scene's systems against its world, one update per step.
struct SceneDelegate<'a> {
    world: &'a mut World,
    systems: &'a mut [Box<dyn System>],
}

impl AppDelegate for SceneDelegate<'_> {
    fn update(&mut self, dt: f32, input: &InputState, frame: &FrameInput) {
        for system in self.systems.iter_mut() {
            let mut ctx = TickContext {
                world: &mut *self.world,
                input,
                frame,
                dt,
            };
            system.run(&mut ctx);
        }
    }

    // Play draws through the viewport's own offscreen pass; the runtime's
    // draw/clear hooks are unused, but the trait still needs the backdrop.
    fn draw(&mut self, _list: &mut DrawList) {}

    fn clear_color(&self) -> Rgba {
        Rgba::rgb(0.0, 0.0, 0.0)
    }
}

/// The transport's Play/Resume action, shared by the toolbar and the shortcut.
///
/// `Ok` carries the message for the console; `Err` is a refusal (no scene, or a
/// snapshot that would not serialize).
pub fn toggle_play(state: &mut EditorState) -> Result<String, String> {
    if state.play.is_playing() {
        state.play.pause();
        return Ok("paused".to_owned());
    }
    if state.play.is_paused() {
        state.play.resume();
        return Ok("resumed".to_owned());
    }
    state.play.play(&mut state.scene, &state.registry)?;
    Ok("playing".to_owned())
}

/// The transport's Step action.
pub fn step_play(state: &mut EditorState) -> Result<String, String> {
    state.play.step(&mut state.scene, &state.registry)?;
    Ok(format!("stepped to {} ticks", state.play.steps()))
}

/// The transport's Stop action.
pub fn stop_play(state: &mut EditorState) -> Result<String, String> {
    state.play.stop(&mut state.scene, &state.registry)?;
    Ok("stopped — the scene was restored".to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use cubic_core::components::Transform;
    use cubic_core::math::Vec2;

    use super::*;

    const STEP: f32 = 1.0 / 60.0;

    /// A step that nudges every transform one unit right, so play mode has
    /// something observable to run.
    struct Shift;

    impl System for Shift {
        fn run(&mut self, ctx: &mut TickContext<'_>) {
            for id in ctx.world.ids_with::<Transform>() {
                if let Some(transform) = ctx.world.get_mut::<Transform>(id) {
                    transform.position.x += 1.0;
                }
            }
        }
    }

    fn state_with_scene() -> EditorState {
        let mut state = EditorState::new();
        let mut world = World::new();
        let id = world.spawn();
        world.insert(id, Transform::from_position(Vec2::ZERO));
        state.scene = Some(OpenScene {
            path: PathBuf::from("main.rsn"),
            world,
        });
        state.play.add_system(Box::new(Shift));
        state
    }

    fn x(state: &EditorState) -> f32 {
        state
            .scene
            .as_ref()
            .expect("a scene")
            .world
            .get::<Transform>(0)
            .expect("the entity's transform")
            .position
            .x
    }

    #[test]
    fn play_advances_the_scene_and_stop_restores_it() {
        let mut state = state_with_scene();

        state.play.play(&mut state.scene, &state.registry).unwrap();
        assert!(state.play.is_playing());

        state.play.advance(&mut state.scene, STEP);
        assert_eq!(x(&state), 1.0, "one step ran");
        state.play.advance(&mut state.scene, STEP * 3.0);
        assert_eq!(x(&state), 4.0, "a slow frame catches up");
        assert_eq!(state.play.steps(), 4);

        state.play.stop(&mut state.scene, &state.registry).unwrap();
        assert!(state.play.is_stopped());
        assert_eq!(x(&state), 0.0, "stop restored the edited state");
    }

    #[test]
    fn pause_freezes_the_scene_and_resume_continues() {
        let mut state = state_with_scene();
        state.play.play(&mut state.scene, &state.registry).unwrap();
        state.play.advance(&mut state.scene, STEP);
        assert_eq!(x(&state), 1.0);

        state.play.pause();
        assert!(state.play.is_paused());
        state.play.advance(&mut state.scene, STEP * 10.0);
        assert_eq!(x(&state), 1.0, "a paused scene does not advance");

        state.play.resume();
        state.play.advance(&mut state.scene, STEP);
        assert_eq!(x(&state), 2.0, "resuming continues from where it froze");
    }

    #[test]
    fn step_runs_exactly_one_tick_from_stopped_and_can_be_restored() {
        let mut state = state_with_scene();

        state.play.step(&mut state.scene, &state.registry).unwrap();

        assert!(state.play.is_paused(), "stepping enters a paused run");
        assert_eq!(state.play.steps(), 1);
        assert_eq!(x(&state), 1.0);

        state.play.stop(&mut state.scene, &state.registry).unwrap();
        assert_eq!(x(&state), 0.0, "stop restores even a stepped scene");
    }

    #[test]
    fn playing_without_a_scene_is_refused_and_leaves_nothing_running() {
        let mut state = EditorState::new();

        assert!(state.play.play(&mut state.scene, &state.registry).is_err());

        assert!(state.play.is_stopped());
    }

    #[test]
    fn a_fresh_play_snapshots_the_current_edits() {
        let mut state = state_with_scene();
        // Edit the scene, then play and move it, then stop: the edit is what
        // comes back, not the original file contents.
        state
            .scene
            .as_mut()
            .unwrap()
            .world
            .get_mut::<Transform>(0)
            .unwrap()
            .position = Vec2::new(10.0, 0.0);

        state.play.play(&mut state.scene, &state.registry).unwrap();
        state.play.advance(&mut state.scene, STEP);
        assert_eq!(x(&state), 11.0);
        state.play.stop(&mut state.scene, &state.registry).unwrap();

        assert_eq!(x(&state), 10.0, "stop restores the edited value");
    }

    #[test]
    fn cancel_discards_the_snapshot_without_restoring() {
        let mut state = state_with_scene();
        state.play.play(&mut state.scene, &state.registry).unwrap();
        state.play.advance(&mut state.scene, STEP);

        state.play.cancel();

        assert!(state.play.is_stopped());
        assert_eq!(x(&state), 1.0, "cancel leaves the simulated world as it is");
    }
}
