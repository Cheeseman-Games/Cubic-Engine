//! Top menu bar (File / View / Help).

use egui::Button;
use egui::Panel;

use crate::project;
use crate::run;
use crate::state::{EditorState, LogLevel, Pending};

/// Draws the top menu bar into the window.
pub fn menu_bar(ui: &mut egui::Ui, state: &mut EditorState) {
    Panel::top("menu_bar").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.menu_button("File", |ui| {
                if clickable(ui, "New Project…", "Ctrl+N").clicked() {
                    ui.close();
                    state.pending = Some(Pending::NewProject);
                }
                if clickable(ui, "Open Project…", "Ctrl+O").clicked() {
                    ui.close();
                    state.pending = Some(Pending::OpenProject);
                }
                if state.project.is_some() && clickable(ui, "Close Project", "").clicked() {
                    ui.close();
                    project::close_project(state);
                }
                ui.separator();
                if clickable(ui, "Save", "Ctrl+S").clicked() {
                    ui.close();
                    project::save_scene(state);
                }
                ui.separator();
                let has_project = state.project.is_some();
                let run_item = Button::new("Run Project").shortcut_text("F8").frame(false);
                if ui
                    .add_enabled(has_project && !state.run.is_running(), run_item)
                    .clicked()
                {
                    ui.close();
                    let outcome = run::run_project(state);
                    state.report(outcome);
                }
                let stop_item = Button::new("Stop Running").shortcut_text("F8").frame(false);
                if ui.add_enabled(state.run.is_running(), stop_item).clicked() {
                    ui.close();
                    let outcome = run::stop_run(state);
                    state.report(outcome);
                }
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
