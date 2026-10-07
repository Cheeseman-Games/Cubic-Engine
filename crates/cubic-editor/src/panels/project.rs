//! Left pane: the open project as a file tree.
//!
//! One row per entry, expanded folders indented beneath their parent, and the
//! inline editors (rename, new file/folder) in place of the row they act on.
//! Rows select on click and open on double-click; right-click offers what the
//! row can do. The tree is read from disk every frame it is expanded — an
//! editor tree is small, and the alternative is a cache that goes stale.

use egui::Key;
use egui::Sense;
use egui::TextStyle;

use crate::project;
use crate::state::{EditorState, LogLevel, Pending, Selection};
use crate::tree::{self, CreateKind, TreeEntry};

/// Draws the project pane.
pub fn project(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.heading("Project");
    ui.separator();
    let Some(root) = state.project_root().map(std::path::Path::to_path_buf) else {
        no_project(ui, state);
        return;
    };
    let name = state.project_name().unwrap_or_default().to_owned();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            dir_row(ui, state, &root, &name, true);
        });
    ui.weak("Double-click to open, right-click for file actions.");
}

/// Shown instead of the tree when nothing is open.
fn no_project(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.label("No project open.");
    ui.label("A project is a folder with a game.toml in it.");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("New Project…").clicked() {
            state.pending = Some(Pending::NewProject);
        }
        if ui.button("Open Project…").clicked() {
            state.pending = Some(Pending::OpenProject);
        }
    });
    ui.add_space(4.0);
    ui.weak("`cubic-cli new <name>` generates one too.");
}

/// A directory: its own row, then (when expanded) its children.
///
/// `is_root` keeps the project folder itself from being renamed or deleted —
/// the tree edits what is inside it.
fn dir_row(
    ui: &mut egui::Ui,
    state: &mut EditorState,
    dir: &std::path::Path,
    name: &str,
    is_root: bool,
) {
    let path = dir.to_path_buf();
    let expanded = state.tree.is_expanded(&path);
    let renaming = state.tree.is_renaming(&path);

    if renaming {
        rename_row(ui, state, &path);
    } else {
        let selected = matches!(&state.selection, Selection::File(file) if *file == path);
        let mut toggle = false;
        ui.horizontal(|ui| {
            let side = ui.text_style_height(&TextStyle::Body);
            let (rect, chevron) = ui.allocate_exact_size(egui::vec2(side, side), Sense::click());
            egui::containers::collapsing_header::paint_default_icon(
                ui,
                if expanded { 1.0 } else { 0.0 },
                &chevron,
            );
            let label = ui.selectable_label(selected, name);
            if label.clicked() {
                state.selection = Selection::File(path.clone());
                let on_chevron = ui
                    .input(|input| input.pointer.hover_pos())
                    .is_some_and(|pos| pos.x < rect.right());
                if on_chevron {
                    toggle = true;
                }
            }
            if label.double_clicked() {
                toggle = true;
            }
            label.context_menu(|ui| entry_menu(ui, state, &path, true, is_root));
        });
        if toggle {
            state.tree.toggle(&path);
        }
    }

    if expanded {
        let creating_here = state
            .tree
            .create
            .as_ref()
            .is_some_and(|create| create.dir == path);
        let children = tree::entries(dir);
        if children.is_empty() && !creating_here {
            ui.weak("  (empty)");
        }
        ui.indent(&path, |ui| {
            if creating_here {
                create_row(ui, state, &path);
            }
            for entry in children {
                if entry.is_dir {
                    dir_row(ui, state, &entry.path, &entry.name, false);
                } else {
                    file_row(ui, state, &entry);
                }
            }
        });
    }
}

/// A file: select on click, open on double-click.
fn file_row(ui: &mut egui::Ui, state: &mut EditorState, entry: &TreeEntry) {
    if state.tree.is_renaming(&entry.path) {
        rename_row(ui, state, &entry.path);
        return;
    }
    let selected = matches!(&state.selection, Selection::File(file) if *file == entry.path);
    let label = ui.selectable_label(selected, &entry.name);
    if label.clicked() {
        state.selection = Selection::File(entry.path.clone());
    }
    if label.double_clicked() {
        let path = entry.path.clone();
        project::open_file(state, ui.ctx(), path);
    }
    label.context_menu(|ui| entry_menu(ui, state, &entry.path, false, false));
}

/// What a row can do: create inside a folder, open a file, rename, delete.
fn entry_menu(
    ui: &mut egui::Ui,
    state: &mut EditorState,
    path: &std::path::Path,
    is_dir: bool,
    is_root: bool,
) {
    if is_dir {
        if ui.button("New file").clicked() {
            ui.close();
            state.tree.begin_create(path, CreateKind::File);
        }
        if ui.button("New folder").clicked() {
            ui.close();
            state.tree.begin_create(path, CreateKind::Folder);
        }
        ui.separator();
    } else if ui.button("Open").clicked() {
        ui.close();
        let path = path.to_path_buf();
        project::open_file(state, ui.ctx(), path);
    }
    if !is_root && ui.button("Rename").clicked() {
        ui.close();
        state.tree.begin_rename(path);
    }
    if !is_root && ui.button("Delete").clicked() {
        ui.close();
        state.tree.confirm_delete = Some(path.to_path_buf());
    }
}

/// The rename editor, replacing the row it renames.
fn rename_row(ui: &mut egui::Ui, state: &mut EditorState, path: &std::path::Path) {
    let Some(rename) = state.tree.rename.as_mut() else {
        return;
    };
    let mut finished: Option<Option<String>> = None;
    ui.horizontal(|ui| {
        let edit = ui.add(
            egui::TextEdit::singleline(&mut rename.name)
                .id_salt(path)
                .desired_width(f32::INFINITY),
        );
        if rename.focus {
            edit.request_focus();
            rename.focus = false;
        }
        if edit.lost_focus() {
            if ui.input(|input| input.key_pressed(Key::Escape)) {
                finished = Some(None);
            } else {
                finished = Some(Some(rename.name.clone()));
            }
        }
    });

    let Some(name) = finished.flatten() else {
        return;
    };
    state.tree.rename = None;
    match tree::rename(path, &name) {
        Ok(new_path) => {
            state.log(LogLevel::Info, format!("renamed to `{name}`"));
            state.selection = Selection::File(new_path.clone());
            if state.scene.as_ref().is_some_and(|scene| scene.path == path) {
                state.scene.as_mut().expect("just checked it is open").path = new_path.clone();
            }
            if let Some(preview) = state.preview.as_mut()
                && preview.path == path
            {
                preview.path = new_path;
            }
        }
        Err(error) => state.log(LogLevel::Error, error),
    }
}

/// The "new file/folder" editor, shown as the first child of its directory.
fn create_row(ui: &mut egui::Ui, state: &mut EditorState, dir: &std::path::Path) {
    let Some(create) = state.tree.create.as_mut() else {
        return;
    };
    let kind = create.kind;
    let mut finished: Option<Option<String>> = None;
    ui.horizontal(|ui| {
        ui.label(match kind {
            CreateKind::File => "file:",
            CreateKind::Folder => "folder:",
        });
        let edit = ui.add(
            egui::TextEdit::singleline(&mut create.name)
                .hint_text("name")
                .id_salt(("new_entry", dir))
                .desired_width(f32::INFINITY),
        );
        if create.focus {
            edit.request_focus();
            create.focus = false;
        }
        if edit.lost_focus() {
            if ui.input(|input| input.key_pressed(Key::Escape)) {
                finished = Some(None);
            } else {
                finished = Some(Some(create.name.clone()));
            }
        }
    });

    let Some(name) = finished.flatten() else {
        return;
    };
    state.tree.create = None;
    match tree::create(dir, kind, &name) {
        Ok(path) => {
            state.log(LogLevel::Info, format!("created `{name}`"));
            state.selection = Selection::File(path);
        }
        Err(error) => state.log(LogLevel::Error, error),
    }
}
