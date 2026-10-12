//! Right pane, under the inspector: the open project's textures and prefabs.
//!
//! Textures are listed with a thumbnail so a project's art is recognisable at a
//! glance, and a click previews the file the way the project tree does. Prefabs
//! are listed as rows that can be dragged onto the viewport, where releasing one
//! stamps the fragment into the open scene (see [`crate::prefab`]). Under both,
//! a row saves the current selection back out as a prefab file.
//!
//! Like every panel it reads while it draws and defers its changes: the
//! thumbnail cache is the only thing touched in place, and a click, a drop, and
//! a save are applied once the scroll area has released its borrow of the UI.

use std::path::{Path, PathBuf};

use cubic_core::assets::AssetKind;
use cubic_core::prefab::Prefab as CorePrefab;

use crate::prefab::{self, PrefabDrag};
use crate::state::{EditorState, Selection};

/// The edge of a texture thumbnail, in points.
const THUMB: f32 = 48.0;

/// Draws the asset pane.
pub fn assets(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.heading("Assets");
    ui.separator();
    if state.project.is_none() {
        ui.weak("No project open.");
        ui.weak("Open a project to browse its assets.");
        return;
    }

    // Cloned so a click can open a file after the scroll area has released its
    // borrow of `ui`.
    let ctx = ui.ctx().clone();
    let mut open: Option<PathBuf> = None;
    let mut save: Option<String> = None;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            textures(ui, state, &mut open);
            ui.separator();
            prefabs(ui, state, &mut open);
            ui.separator();
            save_row(ui, state, &mut save);
        });

    if let Some(path) = open {
        crate::project::open_file(state, &ctx, path);
    }
    if let Some(name) = save {
        let outcome = prefab::save_selection(state, &name);
        if outcome.is_ok() {
            state.prefab_name.clear();
        }
        state.report(outcome);
    }
}

/// The project's textures, a thumbnail beside each path; a click previews one.
fn textures(ui: &mut egui::Ui, state: &mut EditorState, open: &mut Option<PathBuf>) {
    ui.label("Textures");
    let Some(root) = state.project_root().map(Path::to_path_buf) else {
        return;
    };
    let paths: Vec<String> = state
        .catalog
        .matching(AssetKind::Texture)
        .map(|entry| entry.path.clone())
        .collect();
    if paths.is_empty() {
        ui.weak("(no textures in this project)");
        return;
    }
    for path in paths {
        ui.horizontal(|ui| {
            let clicked = match state.thumbnails.get(ui.ctx(), &root, &path) {
                Some(texture) => ui
                    .add(
                        egui::Image::from_texture(texture)
                            .fit_to_exact_size(egui::vec2(THUMB, THUMB))
                            .sense(egui::Sense::click()),
                    )
                    .clicked(),
                None => missing_thumbnail(ui),
            };
            if ui.selectable_label(false, &path).clicked() || clicked {
                *open = Some(root.join(&path));
            }
        });
    }
}

/// A placeholder square for a texture that could not be decoded; a click on it
/// still previews the file, whose own preview explains what is wrong.
fn missing_thumbnail(ui: &mut egui::Ui) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(THUMB, THUMB), egui::Sense::click());
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::ZERO,
        egui::Color32::from_rgb(0x39, 0x1f, 0x1f),
    );
    response.clicked()
}

/// The project's prefabs, a draggable row each; a drop stamps one into the
/// scene, a click previews the file.
fn prefabs(ui: &mut egui::Ui, state: &EditorState, open: &mut Option<PathBuf>) {
    ui.label("Prefabs");
    let Some(root) = state.project_root().map(Path::to_path_buf) else {
        return;
    };
    let paths: Vec<String> = state
        .catalog
        .ending_with(CorePrefab::EXTENSION)
        .map(|entry| entry.path.clone())
        .collect();
    if paths.is_empty() {
        ui.weak("(none — save a selection below)");
        return;
    }
    ui.weak("Drag one onto the viewport to place it.");
    for path in paths {
        let name = prefab::name_of(&path);
        let row = ui.dnd_drag_source(
            egui::Id::new(("prefab", &path)),
            PrefabDrag {
                path: path.clone(),
                name: name.clone(),
            },
            |ui| ui.selectable_label(false, &name),
        );
        if row.inner.clicked() {
            *open = Some(root.join(&path));
        }
        row.response
            .on_hover_text(format!("{path}\nDrag onto the viewport to place it."));
    }
}

/// The save-selection row: a name field and a button that writes a prefab.
fn save_row(ui: &mut egui::Ui, state: &mut EditorState, save: &mut Option<String>) {
    ui.label("Save selection as prefab");
    let selected = matches!(state.selection, Selection::Entity(_));
    ui.horizontal(|ui| {
        ui.add_enabled(
            selected,
            egui::TextEdit::singleline(&mut state.prefab_name)
                .hint_text("name")
                .desired_width(140.0),
        );
        if ui
            .add_enabled(selected, egui::Button::new("Save"))
            .on_hover_text("Writes assets/prefabs/<name>.prefab")
            .clicked()
        {
            *save = Some(state.prefab_name.trim().to_owned());
        }
    });
    if !selected {
        ui.weak("Select an entity to save it.");
    }
}
