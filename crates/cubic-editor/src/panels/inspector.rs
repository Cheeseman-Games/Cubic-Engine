//! Right pane: property editor for the current selection.
//!
//! A selection of an entity draws every component the reflection registry
//! knows it holds, and beneath each one a read-only listing of the field
//! descriptors — name and current value per field. Edit widgets land with the
//! inspector session; the reflection surface they drive is what is wired here.

use egui::CollapsingHeader;

use cubic_core::world::EntityId;

use crate::state::{EditorState, Selection};

/// Draws the inspector pane.
pub fn inspector(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.heading("Inspector");
    ui.separator();
    let selection = state.selection.clone();
    match &selection {
        Selection::None => {
            ui.label("Nothing selected.");
            ui.label("Select a file in the Project pane to inspect it.");
        }
        Selection::File(path) => {
            ui.label("File");
            ui.label(egui::RichText::new(path.display().to_string()).monospace());
        }
        Selection::Entity(id) => draw_entity(ui, state, *id as EntityId),
    }
    ui.separator();
    if state.dirty {
        ui.label(
            egui::RichText::new("Scene has unsaved changes.")
                .color(egui::Color32::from_rgb(0xe5, 0x9b, 0x3b)),
        );
    }
}

/// One reflected component of `entity`: its fields, name and current value.
fn draw_entity(ui: &mut egui::Ui, state: &EditorState, entity: EntityId) {
    ui.label("Entity");
    ui.label(egui::RichText::new(format!("#{entity}")).monospace());
    ui.separator();
    let Some(scene) = &state.scene else {
        ui.label("No scene open.");
        return;
    };
    let components = state.components.on_entity(&scene.world, entity);
    if components.is_empty() {
        ui.label("This entity has no reflected components.");
        return;
    }
    for entry in components {
        CollapsingHeader::new(entry.name)
            .default_open(true)
            .show(ui, |ui| {
                let Some(component) = entry.read(&scene.world, entity) else {
                    ui.label("Component is gone.");
                    return;
                };
                for field in entry.fields {
                    ui.horizontal(|ui| {
                        ui.label(field.name());
                        ui.label(egui::RichText::new(field.get(component).to_string()).monospace());
                    });
                }
            });
    }
}
