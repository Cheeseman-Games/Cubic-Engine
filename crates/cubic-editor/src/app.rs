//! The eframe shell: window creation, persistence, global chrome and shortcuts.

use egui_dock::DockState;

use crate::dock;
use crate::panels;
use crate::state::{EditorState, LogLevel, Pane, Selection};

/// Storage key for the persisted dock layout.
const DOCK_LAYOUT_KEY: &str = "cubic_editor.dock_layout";

/// Root application type hosted by `eframe`.
pub struct EditorApp {
    pub state: EditorState,
}

impl EditorApp {
    /// Builds the app, restoring the persisted dock layout if one exists.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut state = EditorState::new();
        let dock = cc
            .storage
            .and_then(|storage| eframe::get_value::<DockState<Pane>>(storage, DOCK_LAYOUT_KEY));
        if let Some(dock) = dock {
            state.dock = dock;
            state.log(LogLevel::Info, "Restored the saved dock layout");
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
            self.state.log(
                LogLevel::Info,
                "New project: file dialog comes with project support",
            );
        }
        if consumed(ctx, egui::KeyboardShortcut::new(ctrl, egui::Key::O)) {
            self.state.log(
                LogLevel::Info,
                "Open project: file dialog comes with project support",
            );
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
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, DOCK_LAYOUT_KEY, &self.state.dock);
    }
}
