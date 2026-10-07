//! Bottom status strip: project, scene, dirty flag and transient message.

use egui::Panel;

use crate::state::EditorState;

/// Draws the status bar across the window bottom.
pub fn status_bar(ui: &mut egui::Ui, state: &mut EditorState) {
    Panel::bottom("status_bar").show(ui, |ui| {
        ui.horizontal(|ui| {
            let project = match (state.project_name(), state.project_root()) {
                (Some(name), Some(root)) => format!("{name} ({})", root.display()),
                _ => "no project".to_owned(),
            };
            let scene = state
                .scene
                .as_ref()
                .map(|scene| crate::project::shown(state, &scene.path))
                .unwrap_or_else(|| "no scene".to_owned());
            ui.label(format!("project: {project}"));
            ui.separator();
            ui.label(format!("scene: {scene}"));
            ui.separator();
            if state.dirty {
                ui.colored_label(egui::Color32::from_rgb(0xe5, 0x9b, 0x3b), "modified");
            } else {
                ui.colored_label(egui::Color32::from_rgb(0x8a, 0x8a, 0x9a), "saved");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(&state.status);
            });
        });
    });
}
