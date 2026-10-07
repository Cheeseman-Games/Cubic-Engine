//! The eframe shell: window creation, persistence, global chrome and shortcuts.

use std::path::PathBuf;

use egui_dock::DockState;

use crate::dock;
use crate::panels;
use crate::project;
use crate::state::{EditorState, LogLevel, Pane, Pending, Selection};

/// Storage key for the persisted dock layout.
const DOCK_LAYOUT_KEY: &str = "cubic_editor.dock_layout";
/// Storage key for the project to reopen on the next launch.
const PROJECT_ROOT_KEY: &str = "cubic_editor.project_root";

/// Root application type hosted by `eframe`.
pub struct EditorApp {
    pub state: EditorState,
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
        Self { state }
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
            self.state.mark_saved();
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ui.ctx());
        panels::menu_bar(ui, &mut self.state);
        panels::status_bar(ui, &mut self.state);
        dock::show_docked(ui, &mut self.state);
        // After the frame's UI: run any dialog a menu or shortcut asked for, so
        // no egui closure is on the stack when the native dialog blocks.
        project::run_pending(&mut self.state);
        project::modal_windows(ui.ctx(), &mut self.state);
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
