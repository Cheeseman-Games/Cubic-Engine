//! The hierarchy panel: the open scene's entities as an editable tree.
//!
//! Rows are computed once per frame from the `World` ([`crate::hierarchy`]),
//! then drawn with the tree chrome: a chevron when there are children, an
//! indent per level, selection on click, and a right-click menu with
//! everything one entity can do. No row touches the world directly — each
//! menu item records a request (the private [`Action`]) that is applied
//! through the edit queue after the scroll area, so the panel never interleaves
//! UI drawing with scene mutation.

use egui::TextStyle;

use crate::edit;
use crate::hierarchy::{self, Row};
use crate::state::{EditorState, Selection};
use cubic_core::world::EntityId;

/// Something a row asked for this frame, applied once the drawing is done.
#[derive(Clone, Copy)]
enum Action {
    AddEntity(Option<EntityId>),
    Duplicate(EntityId),
    Delete(EntityId),
    AddComponent(EntityId, &'static str),
    RemoveComponent(EntityId, &'static str),
    Reparent(EntityId, Option<EntityId>),
}

/// Draws the hierarchy pane.
pub fn hierarchy(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.heading("Hierarchy");
    ui.separator();
    let Some(open) = state.scene.as_ref() else {
        ui.weak("No scene open.");
        ui.weak("Open a .rsn file from the Project panel to edit entities.");
        return;
    };
    let rows = hierarchy::layout(&open.world, &state.components, &state.hierarchy);

    let mut action: Option<Action> = None;
    let mut toggled: Option<EntityId> = None;
    let mut select: Option<EntityId> = None;
    let selected = selected_entity(state);

    ui.horizontal(|ui| {
        if ui.button("+ Entity").clicked() {
            action = Some(Action::AddEntity(None));
        }
        let can_duplicate = selected.is_some_and(|id| rows.iter().any(|row| row.id == id));
        if ui
            .add_enabled(can_duplicate, egui::Button::new("Duplicate"))
            .clicked()
            && let Some(id) = selected
        {
            action = Some(Action::Duplicate(id));
        }
        let can_delete = can_duplicate;
        if ui
            .add_enabled(can_delete, egui::Button::new("Delete"))
            .on_hover_text(if can_delete {
                "Removes the entity and its children."
            } else {
                ""
            })
            .clicked()
            && let Some(id) = selected
        {
            action = Some(Action::Delete(id));
        }
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if rows.is_empty() {
                ui.weak("(nothing in the scene)");
            }
            for row in &rows {
                row_ui(
                    ui,
                    state,
                    row,
                    selected,
                    &mut action,
                    &mut toggled,
                    &mut select,
                );
            }
        });
    ui.weak("Right-click an entity for actions.");

    if let Some(toggled) = toggled {
        state.hierarchy.toggle(toggled);
    }
    if let Some(selected) = select {
        state.selection = Selection::Entity(selected as u64);
    }
    if let Some(action) = action {
        let outcome = run_action(state, action);
        state.report(outcome);
    }
}

/// The selected entity, if the selection is one.
fn selected_entity(state: &EditorState) -> Option<EntityId> {
    match state.selection {
        Selection::Entity(id) => Some(id as EntityId),
        _ => None,
    }
}

/// One entity row: chevron, indent, id, and the actions it offers.
fn row_ui(
    ui: &mut egui::Ui,
    state: &EditorState,
    row: &Row,
    selected: Option<EntityId>,
    action: &mut Option<Action>,
    toggled: &mut Option<EntityId>,
    select: &mut Option<EntityId>,
) {
    ui.horizontal(|ui| {
        if row.depth > 0 {
            ui.add_space(row.depth as f32 * 12.0);
        }
        let side = ui.text_style_height(&TextStyle::Body);
        let (_, chevron) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
        let expanded = state.hierarchy.is_expanded(row.id);
        egui::containers::collapsing_header::paint_default_icon(
            ui,
            if expanded { 1.0 } else { 0.0 },
            &chevron,
        );
        if row.children > 0 && chevron.clicked() {
            *toggled = Some(row.id);
        }

        let is_selected = selected == Some(row.id);
        let label = ui.selectable_label(is_selected, format!("#{}", row.id));
        if label.clicked() {
            *select = Some(row.id);
        }
        label.context_menu(|ui| row_menu(ui, state, row, action));
    });
}

/// What one entity can do: add under it, duplicate, delete, reparent, and
/// add or remove components.
fn row_menu(ui: &mut egui::Ui, state: &EditorState, row: &Row, action: &mut Option<Action>) {
    if ui.button("Add child entity").clicked() {
        ui.close();
        *action = Some(Action::AddEntity(Some(row.id)));
    }
    if ui.button("Duplicate").clicked() {
        ui.close();
        *action = Some(Action::Duplicate(row.id));
    }
    if ui.button("Delete").clicked() {
        ui.close();
        *action = Some(Action::Delete(row.id));
    }

    if let Some(selected) = selected_entity(state) {
        ui.separator();
        if selected != row.id && ui.button("Parent to selected").clicked() {
            ui.close();
            *action = Some(Action::Reparent(row.id, Some(selected)));
        }
        if row.parent.is_some() && ui.button("Unparent").clicked() {
            ui.close();
            *action = Some(Action::Reparent(row.id, None));
        }
    }

    if !row.addable.is_empty() {
        ui.separator();
        for name in &row.addable {
            if ui.button(format!("Add {name}")).clicked() {
                ui.close();
                *action = Some(Action::AddComponent(row.id, name));
            }
        }
    }
    if !row.components.is_empty() {
        ui.separator();
        for name in &row.components {
            if ui.button(format!("Remove {name}")).clicked() {
                ui.close();
                *action = Some(Action::RemoveComponent(row.id, name));
            }
        }
    }
}

/// Runs a collected action through the edit queue, returning its outcome.
fn run_action(state: &mut EditorState, action: Action) -> Result<String, String> {
    match action {
        Action::AddEntity(parent) => edit::add_entity(state, parent),
        Action::Duplicate(id) => edit::duplicate_entity(state, id),
        Action::Delete(id) => edit::delete_entity(state, id),
        Action::AddComponent(entity, name) => edit::add_component(state, entity, name),
        Action::RemoveComponent(entity, name) => edit::remove_component(state, entity, name),
        Action::Reparent(entity, parent) => edit::reparent(state, entity, parent),
    }
}
