//! Top menu bar (File / View / Help).

use egui::Button;
use egui::Panel;

use crate::state::{EditorState, LogLevel};

/// Draws the top menu bar into the window.
pub fn menu_bar(ui: &mut egui::Ui, state: &mut EditorState) {
    Panel::top("menu_bar").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.menu_button("File", |ui| {
                if clickable(ui, "New", "Ctrl+N").clicked() {
                    ui.close();
                    state.log(
                        LogLevel::Info,
                        "New project: file dialog comes with project support",
                    );
                }
                if clickable(ui, "Open...", "Ctrl+O").clicked() {
                    ui.close();
                    state.log(
                        LogLevel::Info,
                        "Open project: file dialog comes with project support",
                    );
                }
                if clickable(ui, "Save", "Ctrl+S").clicked() {
                    ui.close();
                    state.mark_saved();
                }
                ui.separator();
                if clickable(ui, "Quit", "Ctrl+Q").clicked() {
                    ui.close();
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.menu_button("View", |ui| {
                if clickable(ui, "Reset layout", "Ctrl+Shift+R").clicked() {
                    ui.close();
                    state.reset_dock();
                    state.log(LogLevel::Info, "Dock layout reset");
                }
            });

            ui.menu_button("Help", |ui| {
                if clickable(ui, "About", "").clicked() {
                    ui.close();
                    state.log(
                        LogLevel::Info,
                        "Cubic Editor — the Cubic-Engine editor shell",
                    );
                }
            });
        });
    });
}

/// A menu-styled item with an optional key-hint on the right.
fn clickable(ui: &mut egui::Ui, label: &str, shortcut: &str) -> egui::Response {
    ui.add(Button::new(label).shortcut_text(shortcut).frame(false))
}
