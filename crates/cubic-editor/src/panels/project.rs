//! Left pane: the open project root and a placeholder for the file tree.
//!
//! The tree proper (navigate, create/rename, open by extension) lands with the
//! project system; this pane shows the project root and a few conventional
//! folders so selection is already wired.

use egui::CollapsingHeader;

use crate::state::{EditorState, Selection};

/// Draws the project pane.
pub fn project(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.heading("Project");
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match &state.project {
            Some(root) => {
                let name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.display().to_string());
                CollapsingHeader::new(name)
                    .default_open(true)
                    .show(ui, |ui| {
                        for entry in ["assets", "scenes", "src"] {
                            let path = root.join(entry);
                            if ui.selectable_label(false, entry).clicked() {
                                state.selection = Selection::File(path);
                            }
                        }
                        ui.label("Full file tree comes with project support");
                    });
            }
            None => {
                ui.label("No project open.");
                ui.label("File > Open Project once file dialogs arrive.");
            }
        });
}
