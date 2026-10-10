//! The eframe shell: window creation, persistence, global chrome and shortcuts.

use std::path::PathBuf;

use egui_dock::DockState;

use crate::dock;
use crate::edit;
use crate::panels;
use crate::play;
use crate::project;
use crate::run;
use crate::state::{EditorState, LogLevel, Pane, Pending, Selection, ensure_dock_panes};
use crate::viewport::ViewportHost;
use cubic_core::world::EntityId;

/// Storage key for the persisted dock layout.
const DOCK_LAYOUT_KEY: &str = "cubic_editor.dock_layout";
/// Storage key for the project to reopen on the next launch.
const PROJECT_ROOT_KEY: &str = "cubic_editor.project_root";

/// Root application type hosted by `eframe`.
pub struct EditorApp {
    pub state: EditorState,
    /// The viewport's engine side: offscreen target and camera. Kept out of
    /// `EditorState` because it holds GPU resources, which are per-run rather
    /// than persisted editor state.
    pub viewport: ViewportHost,
}

impl EditorApp {
    /// Builds the app, restoring the persisted dock layout and project.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut state = EditorState::new();
        let storage = cc.storage;
        let dock = storage
            .and_then(|storage| eframe::get_value::<DockState<Pane>>(storage, DOCK_LAYOUT_KEY));
        if let Some(dock) = dock {
            state.dock = dock;
            ensure_dock_panes(&mut state.dock);
            state.log(LogLevel::Info, "Restored the saved dock layout");
        }
        let root = storage
            .and_then(|storage| eframe::get_value::<Option<PathBuf>>(storage, PROJECT_ROOT_KEY))
            .flatten();
        if let Some(root) = root
            && let Err(error) = project::open_project(&mut state, &root)
        {
            state.log(
                LogLevel::Error,
                format!("could not reopen {}: {error}", root.display()),
            );
        }
        Self {
            state,
            viewport: ViewportHost::new(),
        }
    }

    /// Global keyboard shortcuts. Skipped while any widget owns the keyboard.
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let ctrl = egui::Modifiers::COMMAND;
        if consumed(ctx, egui::KeyboardShortcut::new(ctrl, egui::Key::N)) {
            self.state.pending = Some(Pending::NewProject);
        }
        if consumed(ctx, egui::KeyboardShortcut::new(ctrl, egui::Key::O)) {
            self.state.pending = Some(Pending::OpenProject);
        }
        if consumed(ctx, egui::KeyboardShortcut::new(ctrl, egui::Key::S)) {
            project::save_scene(&mut self.state);
        }
        if consumed(
            ctx,
            egui::KeyboardShortcut::new(ctrl | egui::Modifiers::SHIFT, egui::Key::R),
        ) {
            self.state.reset_dock();
            self.state.log(LogLevel::Info, "Dock layout reset");
        }
        if consumed(ctx, egui::KeyboardShortcut::new(ctrl, egui::Key::Q)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Undo and redo: Ctrl+Shift+Z redo is tested before Ctrl+Z undo so a
        // shift held while pressing Z reaches redo, and Ctrl+Y is Win/Linux
        // muscle memory for the same redo.
        if consumed(
            ctx,
            egui::KeyboardShortcut::new(ctrl | egui::Modifiers::SHIFT, egui::Key::Z),
        ) {
            let outcome = edit::redo(&mut self.state);
            self.state.report(outcome);
        }
        if consumed(ctx, egui::KeyboardShortcut::new(ctrl, egui::Key::Z)) {
            let outcome = edit::undo(&mut self.state);
            self.state.report(outcome);
        }
        if consumed(ctx, egui::KeyboardShortcut::new(ctrl, egui::Key::Y)) {
            let outcome = edit::redo(&mut self.state);
            self.state.report(outcome);
        }
        // Edit the selected entity: Delete drops it (and its subtree), Ctrl+D
        // duplicates it. Both stay silent unless an entity is selected, so a
        // stray key does not spam the console with refusals.
        if matches!(self.state.selection, Selection::Entity(_))
            && ctx.input(|i| i.key_pressed(egui::Key::Delete))
        {
            let id = match self.state.selection {
                Selection::Entity(id) => id as EntityId,
                _ => unreachable!("guarded above"),
            };
            let outcome = edit::delete_entity(&mut self.state, id);
            self.state.report(outcome);
        }
        if consumed(ctx, egui::KeyboardShortcut::new(ctrl, egui::Key::D))
            && let Selection::Entity(id) = self.state.selection
        {
            let outcome = edit::duplicate_entity(&mut self.state, id as EntityId);
            self.state.report(outcome);
        }
        // Play transport. Plain function keys, so they do not collide with the
        // Ctrl-held file shortcuts above.
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            let outcome = play::toggle_play(&mut self.state);
            self.state.report(outcome);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F6)) {
            let outcome = play::step_play(&mut self.state);
            self.state.report(outcome);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F7)) {
            let outcome = play::stop_play(&mut self.state);
            self.state.report(outcome);
        }
        // F8 toggles the project run: stop a live one, otherwise start one.
        if ctx.input(|i| i.key_pressed(egui::Key::F8)) {
            let outcome = if self.state.run.is_running() {
                run::stop_run(&mut self.state)
            } else {
                run::run_project(&mut self.state)
            };
            self.state.report(outcome);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape))
            && !matches!(self.state.selection, Selection::None)
            && self.state.tree.confirm_delete.is_none()
            && self.state.new_project.is_none()
        {
            self.state.selection = Selection::None;
        }
    }
}

/// Whether the given shortcut was pressed this frame (and consumed).
fn consumed(ctx: &egui::Context, shortcut: egui::KeyboardShortcut) -> bool {
    ctx.input_mut(|i| i.consume_shortcut(&shortcut))
}

impl eframe::App for EditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Before any panel runs: the viewport needs the device/queue eframe
        // renders with, so its target is built on the same GPU state.
        self.viewport.set_render_state(frame.wgpu_render_state());
        self.handle_shortcuts(ui.ctx());
        // Run the play clock, then ask for another frame while it is running:
        // egui is event-driven, so without this an idle window would stop
        // repainting and the simulation would appear to freeze mid-run.
        let elapsed = ui.ctx().input(|i| i.stable_dt);
        self.state.play.advance(&mut self.state.scene, elapsed);
        if self.state.play.is_playing() {
            ui.ctx().request_repaint();
        }
        // One running project at a time: fold its output into the console and,
        // while it is live, keep the frame clock going the same way Play does.
        for (level, line) in self.state.run.take_output() {
            self.state.log(level, line);
        }
        if self.state.run.is_running() {
            ui.ctx().request_repaint();
        }
        panels::menu_bar(ui, &mut self.state);
        panels::toolbar(ui, &mut self.state);
        panels::status_bar(ui, &mut self.state);
        dock::show_docked(ui, &mut self.state, &mut self.viewport);
        // After the frame's UI: run any dialog a menu or shortcut asked for, so
        // no egui closure is on the stack when the native dialog blocks.
        project::run_pending(&mut self.state);
        project::modal_windows(ui.ctx(), &mut self.state);
        // Finally, fold anything the engine logged during the frame into the
        // console; it is drained once per frame so the pane shows this frame's
        // records in order, after the panel that displays them has drawn.
        crate::logging::drain_into(&mut self.state);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, DOCK_LAYOUT_KEY, &self.state.dock);
        eframe::set_value(
            storage,
            PROJECT_ROOT_KEY,
            &self.state.project_root().map(PathBuf::from),
        );
    }
}
