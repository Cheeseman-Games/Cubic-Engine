//! Right pane: property editor for the current selection.
//!
//! Reflection-driven field editing lands later; this pane reports the
//! selection and marks the spot.

use egui::CollapsingHeader;

use crate::state::{EditorState, Selection};

/// Draws the inspector pane.
pub fn inspector(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.heading("Inspector");
    ui.separator();
    match &state.selection {
        Selection::None => {
            ui.label("Nothing selected.");
            ui.label("Select a file in the Project pane to inspect it.");
        }
        Selection::File(path) => {
            ui.label("File");
            ui.label(egui::RichText::new(path.display().to_string()).monospace());
        }
        Selection::Entity(id) => {
            ui.label("Entity");
            ui.label(egui::RichText::new(format!("#{id}")).monospace());
            ui.separator();
            CollapsingHeader::new("Transform")
                .default_open(true)
                .show(ui, |ui| {
                    ui.label("Field editing comes with reflection support.");
                });
        }
    }
    ui.separator();
    if state.dirty {
        ui.label(
            egui::RichText::new("Scene has unsaved changes.")
                .color(egui::Color32::from_rgb(0xe5, 0x9b, 0x3b)),
        );
    }
}
