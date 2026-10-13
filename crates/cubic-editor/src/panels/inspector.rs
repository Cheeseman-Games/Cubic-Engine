//! Right pane: the property editor for the current selection.
//!
//! Selecting an entity draws every component the reflection registry knows it
//! holds, and under each one a row per field. Each row is the widget its
//! [`FieldKind`] implies — a drag-scrub for a number, a swatch for a color, a
//! combo box for a choice, a picker for an asset — and every edit is handed to
//! [`edit::set_field`], so it lands in the world the moment it is made and can
//! be undone like any other change.
//!
//! # Edits are collected, then applied
//!
//! The panel reads the world to draw, so it cannot hand out a `&mut World`
//! while an egui closure still borrows it. Instead each widget returns an
//! [`Edit`] if it changed something, the rows push those into a list, and the
//! list is applied after the last row is drawn — the same shape the hierarchy
//! panel uses for its actions.

use egui::CollapsingHeader;

use cubic_core::assets::{AssetHandle, AssetKind};
use cubic_core::reflect::{Field, FieldKind, FieldValue};
use cubic_core::render::Rgba;
use cubic_core::world::EntityId;

use crate::edit;
use crate::state::{EditorState, LogLevel, Selection};

/// The component that carries a sprite's draw layer.
const SPRITE: &str = "Sprite";

/// The field on [`SPRITE`] that orders the draws.
const LAYER: &str = "layer";

/// One change a widget reported, applied after the frame's rows are drawn.
enum Edit {
    /// A field took a new value.
    Set {
        entity: EntityId,
        component: &'static str,
        field: &'static str,
        value: FieldValue,
    },
    /// An asset field picked a file to load.
    LoadAsset {
        entity: EntityId,
        component: &'static str,
        field: &'static str,
        path: String,
    },
}

/// Draws the inspector pane.
pub fn inspector(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.heading("Inspector");
    ui.separator();
    let selection = state.selection.clone();
    let edits = match &selection {
        Selection::None => {
            ui.label("Nothing selected.");
            ui.label("Select an entity in the Hierarchy to edit it.");
            Vec::new()
        }
        Selection::File(path) => {
            ui.label("File");
            ui.label(egui::RichText::new(path.display().to_string()).monospace());
            Vec::new()
        }
        Selection::Entity(id) => draw_entity(ui, state, *id as EntityId),
    };
    for edit in edits {
        apply(state, edit);
    }
    ui.separator();
    if state.dirty {
        ui.label(
            egui::RichText::new("Scene has unsaved changes.")
                .color(egui::Color32::from_rgb(0xe5, 0x9b, 0x3b)),
        );
    }
}

/// Draws one entity's reflected components, collecting what changed.
fn draw_entity(ui: &mut egui::Ui, state: &EditorState, entity: EntityId) -> Vec<Edit> {
    ui.label("Entity");
    ui.label(egui::RichText::new(format!("#{entity}")).monospace());
    ui.separator();
    let Some(scene) = &state.scene else {
        ui.label("No scene open.");
        return Vec::new();
    };
    let components = state.components.on_entity(&scene.world, entity);
    if components.is_empty() {
        ui.label("This entity has no reflected components.");
        return Vec::new();
    }
    let mut edits = Vec::new();
    for entry in components {
        CollapsingHeader::new(entry.name)
            .default_open(true)
            .show(ui, |ui| {
                let Some(component) = entry.read(&scene.world, entity) else {
                    ui.label("Component is gone.");
                    return;
                };
                if entry.fields.is_empty() {
                    ui.weak("No editable fields.");
                    return;
                }
                for field in entry.fields {
                    let value = field.get(component);
                    ui.horizontal(|ui| {
                        ui.label(field.name());
                        if let Some(edit) =
                            field_widget(ui, state, entity, entry.name, field, value)
                        {
                            edits.push(edit);
                        }
                    });
                }
                if entry.name == SPRITE
                    && let Some(field) = entry.field(LAYER)
                    && let FieldValue::I32(layer) = field.get(component)
                {
                    layer_order_row(ui, entity, layer, &mut edits);
                }
            });
    }
    edits
}

/// A pair of buttons that nudge a sprite's draw layer, one undoable step each.
///
/// The viewport sorts sprites by layer, so a click moves this sprite exactly one
/// band towards the front or the back; the `layer` field's own drag-scrub is
/// still there for a bigger jump.
fn layer_order_row(ui: &mut egui::Ui, entity: EntityId, layer: i32, edits: &mut Vec<Edit>) {
    ui.horizontal(|ui| {
        ui.label("draw order");
        if ui
            .button("▲")
            .on_hover_text("Draw one layer nearer the front")
            .clicked()
        {
            edits.push(Edit::Set {
                entity,
                component: SPRITE,
                field: LAYER,
                value: FieldValue::I32(layer.saturating_add(1)),
            });
        }
        if ui
            .button("▼")
            .on_hover_text("Draw one layer nearer the back")
            .clicked()
        {
            edits.push(Edit::Set {
                entity,
                component: SPRITE,
                field: LAYER,
                value: FieldValue::I32(layer.saturating_sub(1)),
            });
        }
    });
}

/// The widget for one field, returning an [`Edit`] only when it changed.
fn field_widget(
    ui: &mut egui::Ui,
    state: &EditorState,
    entity: EntityId,
    component: &'static str,
    field: &Field,
    value: FieldValue,
) -> Option<Edit> {
    let name = field.name();
    match (field.kind(), value) {
        (FieldKind::F32, FieldValue::F32(mut v)) => {
            let changed = ui.add(egui::DragValue::new(&mut v).speed(0.01)).changed();
            changed.then_some(Edit::Set {
                entity,
                component,
                field: name,
                value: FieldValue::F32(v),
            })
        }
        (FieldKind::I32, FieldValue::I32(mut v)) => {
            let changed = ui.add(egui::DragValue::new(&mut v).speed(1.0)).changed();
            changed.then_some(Edit::Set {
                entity,
                component,
                field: name,
                value: FieldValue::I32(v),
            })
        }
        (FieldKind::Bool, FieldValue::Bool(mut v)) => {
            let changed = ui.checkbox(&mut v, "").changed();
            changed.then_some(Edit::Set {
                entity,
                component,
                field: name,
                value: FieldValue::Bool(v),
            })
        }
        (FieldKind::Vec2, FieldValue::Vec2(mut v)) => {
            let x = ui.add(egui::DragValue::new(&mut v.x).speed(0.01).prefix("x "));
            let y = ui.add(egui::DragValue::new(&mut v.y).speed(0.01).prefix("y "));
            (x.changed() || y.changed()).then_some(Edit::Set {
                entity,
                component,
                field: name,
                value: FieldValue::Vec2(v),
            })
        }
        (FieldKind::Color, FieldValue::Color(color)) => {
            let mut rgba = [color.r, color.g, color.b, color.a];
            let changed = ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed();
            changed.then_some(Edit::Set {
                entity,
                component,
                field: name,
                value: FieldValue::Color(Rgba::new(rgba[0], rgba[1], rgba[2], rgba[3])),
            })
        }
        (FieldKind::Select(options), FieldValue::Select(current)) => {
            let mut selected = current;
            egui::ComboBox::from_id_salt((entity, component, name))
                .selected_text(options.get(current).copied().unwrap_or("?"))
                .show_ui(ui, |ui| {
                    for (index, label) in options.iter().enumerate() {
                        ui.selectable_value(&mut selected, index, *label);
                    }
                });
            (selected != current).then_some(Edit::Set {
                entity,
                component,
                field: name,
                value: FieldValue::Select(selected),
            })
        }
        (FieldKind::String, FieldValue::String(current)) => {
            text_field(ui, entity, component, name, &current)
        }
        (FieldKind::Asset { kind }, FieldValue::Asset(handle)) => {
            asset_picker(ui, state, entity, component, field, kind, handle)
        }
        // A descriptor always carries the value shape its kind promises, so this
        // is unreachable while the reflection layer is honest; drawing a note
        // beats panicking in the middle of a frame if it ever is not.
        (_, value) => {
            ui.weak(format!("cannot show a {} value", value.name()));
            None
        }
    }
}

/// A text box whose buffer is kept in egui's temporary storage.
///
/// Re-seeding the buffer from the component every frame would fight the user:
/// the world lags a keystroke behind the box, so the cursor would jump. While
/// the box has focus the buffer is the truth and the field follows it; once it
/// loses focus the buffer is dropped, so an undo or a fresh scene load shows
/// through again.
fn text_field(
    ui: &mut egui::Ui,
    entity: EntityId,
    component: &'static str,
    field: &'static str,
    current: &str,
) -> Option<Edit> {
    let ctx = ui.ctx().clone();
    let id = ui.make_persistent_id((entity, component, field));
    if !ctx.memory(|memory| memory.has_focus(id)) {
        ctx.data_mut(|data| data.remove::<String>(id));
    }
    let mut buffer = ctx
        .data(|data| data.get_temp::<String>(id))
        .unwrap_or_else(|| current.to_owned());
    let changed = ui
        .add(
            egui::TextEdit::singleline(&mut buffer)
                .id(id)
                .desired_width(f32::INFINITY),
        )
        .changed();
    ctx.data_mut(|data| data.insert_temp(id, buffer.clone()));
    changed.then_some(Edit::Set {
        entity,
        component,
        field,
        value: FieldValue::String(buffer),
    })
}

/// A combo box over the assets a field of `kind` can point at.
///
/// The current value is shown by its path when the open project's server knows
/// the handle, and by the handle's id otherwise — a handle a game registered at
/// runtime names nothing on disk the editor can read.
fn asset_picker(
    ui: &mut egui::Ui,
    state: &EditorState,
    entity: EntityId,
    component: &'static str,
    field: &Field,
    kind: AssetKind,
    handle: AssetHandle,
) -> Option<Edit> {
    let name = field.name();
    let current = state
        .assets
        .as_ref()
        .and_then(|server| server.path(handle))
        .map(str::to_owned);
    let label = current.clone().unwrap_or_else(|| handle.to_string());
    let mut chosen = None;
    egui::ComboBox::from_id_salt((entity, component, name))
        .selected_text(label)
        .show_ui(ui, |ui| {
            let mut any = false;
            for candidate in state.catalog.matching(kind) {
                any = true;
                let selected = current.as_deref() == Some(candidate.path.as_str());
                if ui.selectable_label(selected, &candidate.path).clicked() {
                    chosen = Some(candidate.path.clone());
                }
            }
            if !any {
                ui.weak(format!("no {kind} assets in this project"));
            }
        });
    chosen.map(|path| Edit::LoadAsset {
        entity,
        component,
        field: name,
        path,
    })
}

/// Applies one collected edit, reporting any refusal to the console.
fn apply(state: &mut EditorState, edit: Edit) {
    let outcome = match edit {
        Edit::Set {
            entity,
            component,
            field,
            value,
        } => edit::set_field(state, entity, component, field, value),
        Edit::LoadAsset {
            entity,
            component,
            field,
            path,
        } => load_asset(state, entity, component, field, &path),
    };
    // A successful field edit is silent: a drag would otherwise fill the console
    // with one line per frame.
    if let Err(error) = outcome {
        state.log(LogLevel::Error, error);
    }
}

/// Loads `path` through the open project's asset server and writes the handle.
fn load_asset(
    state: &mut EditorState,
    entity: EntityId,
    component: &'static str,
    field: &'static str,
    path: &str,
) -> Result<String, String> {
    let handle = match state.assets.as_mut() {
        None => return Err("no project is open — there are no assets to pick".to_owned()),
        Some(server) => server.load(path).map_err(|error| error.to_string())?,
    };
    edit::set_field(state, entity, component, field, FieldValue::Asset(handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_an_asset_without_a_server_is_refused() {
        let mut state = EditorState::default();
        let error = load_asset(&mut state, 0, "Sprite", "texture", "assets/hero.png").unwrap_err();
        assert!(error.contains("no project"), "got: {error}");
    }
}
