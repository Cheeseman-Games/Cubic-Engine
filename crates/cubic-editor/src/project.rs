//! The project system: what is open, how it opens, and how a new one is made.
//!
//! A project is a directory with `game.toml` in it — the same rule
//! `cubic-cli` discovers by, reusing `cubic-cli`'s own parser and generator so
//! the editor and the command line can never disagree about what a project is.
//!
//! This module also owns the two file dialogs and the modal windows behind
//! them (the new-project name form and the delete confirmation), because those
//! need the whole editor, not one panel.

use std::path::{Path, PathBuf};

use cubic_cli::{EngineSource, Project, scaffold};
use cubic_core::scene::Scene;

use crate::preview::{self, PreviewKind};
use crate::state::{EditorState, LogLevel, NewProjectForm, OpenScene, Pending, Selection};
use crate::tree::{self, TreeState};

/// How a file opened from the tree is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenKind {
    /// A `.rsn` scene: becomes the open scene rather than a preview.
    Scene,
    /// A decodable image, previewed in the viewport.
    Image,
    /// A text file, previewed read-only in the viewport.
    Text,
}

/// What opening `path` would do, decided by extension; `None` means the editor
/// has no way to show it and it is selected rather than opened.
pub fn classify(path: &Path) -> Option<OpenKind> {
    let Some(extension) = path.extension() else {
        // No dot at all: README, LICENSE, .gitignore — the plain-text company
        // a project directory keeps.
        return Some(OpenKind::Text);
    };
    match extension.to_str()?.to_ascii_lowercase().as_str() {
        "rsn" => Some(OpenKind::Scene),
        "png" | "jpg" | "jpeg" => Some(OpenKind::Image),
        "rs" | "toml" | "txt" | "md" | "json" | "ron" | "yml" | "yaml" | "wgsl" | "glsl"
        | "cfg" | "ini" | "lock" => Some(OpenKind::Text),
        _ => None,
    }
}

/// Opens the project at `path` — a directory or its `game.toml` — and makes it
/// the editor's project, clearing whatever was open before it.
pub fn open_project(state: &mut EditorState, path: &Path) -> Result<(), String> {
    let project = Project::open(path).map_err(|error| error.to_string())?;
    let name = project.manifest.game.name.clone();
    let root = project.root.clone();

    state.project = Some(project);
    state.scene = None;
    state.play.cancel();
    state.preview = None;
    state.selection = Selection::None;
    state.dirty = false;
    state.edits.clear();
    state.hierarchy.clear();
    state.tree = TreeState::default();
    state.tree.expand(&root);
    state.log(
        LogLevel::Info,
        format!("opened project `{name}` at {}", root.display()),
    );
    Ok(())
}

/// Closes the open project, leaving an empty editor.
pub fn close_project(state: &mut EditorState) {
    if state.project.take().is_none() {
        return;
    }
    state.scene = None;
    state.play.cancel();
    state.preview = None;
    state.selection = Selection::None;
    state.dirty = false;
    state.edits.clear();
    state.hierarchy.clear();
    state.tree = TreeState::default();
    state.log(LogLevel::Info, "project closed");
}

/// Opens a file from the tree: `.rsn` becomes the scene, everything else with
/// a known extension previews in the viewport, the rest is only selected.
pub fn open_file(state: &mut EditorState, ctx: &egui::Context, path: PathBuf) {
    state.selection = Selection::File(path.clone());
    match classify(&path) {
        Some(OpenKind::Scene) => open_scene(state, path),
        Some(kind) => {
            let preview = preview::load(&path, kind, ctx);
            if let PreviewKind::Error(message) = &preview.kind {
                state.log(LogLevel::Error, message.clone());
            }
            state.preview = Some(preview);
        }
        None => state.log(
            LogLevel::Warn,
            format!(
                "no preview for {} — it is selected instead",
                shown(state, &path)
            ),
        ),
    }
}

/// Reads a `.rsn` file into the open scene.
///
/// A file that will not parse keeps whatever scene was open before it; a file
/// that parses replaces the scene even if some components are unknown, with
/// the skipped ones logged as warnings rather than failing the whole load.
fn open_scene(state: &mut EditorState, path: PathBuf) {
    state.preview = None;
    // Any run of the previous scene is over: its snapshot describes a world
    // that is about to be replaced.
    state.play.cancel();
    let loaded = match Scene::load(&path).and_then(|scene| scene.to_world(&state.registry)) {
        Ok(loaded) => loaded,
        Err(error) => {
            state.log(
                LogLevel::Error,
                format!("could not open scene {}: {error}", shown(state, &path)),
            );
            return;
        }
    };
    for warning in loaded.warnings {
        state.log(LogLevel::Warn, warning);
    }
    state.scene = Some(OpenScene {
        path: path.clone(),
        world: loaded.world,
    });
    state.dirty = false;
    state.edits.clear();
    state.hierarchy.clear();
    state.log(
        LogLevel::Info,
        format!("opened scene {}", shown(state, &path)),
    );
}

/// Writes the open scene back to its `.rsn` file.
///
/// `File > Save` and Ctrl+S both land here; it is the only thing that ever
/// clears the dirty flag, so a scene is never marked saved it was not.
pub fn save_scene(state: &mut EditorState) {
    let Some(open) = state.scene.as_ref() else {
        state.log(LogLevel::Warn, "nothing to save — no scene is open");
        return;
    };
    let outcome = Scene::from_world(&open.world, &state.registry)
        .and_then(|scene| scene.save(&open.path))
        .map_err(|error| format!("could not save {}: {error}", open.path.display()));
    match outcome {
        Ok(()) => state.mark_saved(),
        Err(message) => state.log(LogLevel::Error, message),
    }
}

/// Runs the dialog a menu item or shortcut asked for.
///
/// Deferring the blocking native dialog until after the frame's UI means the
/// menu that opened it has closed and no egui closure is on the stack.
pub fn run_pending(state: &mut EditorState) {
    match state.pending.take() {
        None => {}
        Some(Pending::OpenProject) => {
            if let Some(path) = pick_folder("Open project", state)
                && let Err(error) = open_project(state, &path)
            {
                state.log(LogLevel::Error, error);
            }
        }
        Some(Pending::NewProject) => {
            if let Some(parent) = pick_folder("Create the project in", state) {
                state.new_project = Some(NewProjectForm {
                    parent,
                    name: String::new(),
                    focus: true,
                    error: None,
                });
            }
        }
    }
}

/// A native folder picker, starting inside the open project when there is one.
pub fn pick_folder(title: &str, state: &EditorState) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(title.to_owned());
    if let Some(root) = state.project_root() {
        dialog = dialog.set_directory(root);
    }
    dialog.pick_folder()
}

/// The modal windows: the new-project form and the delete confirmation.
///
/// Both are taken out of the state for the frame (a window closure needs the
/// whole state mutably) and put back unless they were dismissed.
pub fn modal_windows(ctx: &egui::Context, state: &mut EditorState) {
    new_project_window(ctx, state);
    delete_window(ctx, state);
}

/// The name form shown after `File > New Project…` picks a folder.
fn new_project_window(ctx: &egui::Context, state: &mut EditorState) {
    let Some(mut form) = state.new_project.take() else {
        return;
    };
    let mut open = true;
    let mut submit = false;
    let mut cancel = false;
    egui::Window::new("New project")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(format!("Creating in {}", form.parent.display()));
            let edit = ui.add(
                egui::TextEdit::singleline(&mut form.name)
                    .hint_text("my_game")
                    .id_salt("new_project_name")
                    .desired_width(300.0),
            );
            if form.focus {
                edit.request_focus();
                form.focus = false;
            }
            if let Some(error) = &form.error {
                ui.colored_label(egui::Color32::from_rgb(0xef, 0x43, 0x43), error);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Create").clicked() {
                    submit = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
            if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
        });

    if !open || cancel {
        return;
    }
    if !submit {
        state.new_project = Some(form);
        return;
    }
    if let Err(error) = create_project(state, &form) {
        form.error = Some(error);
        state.new_project = Some(form);
    }
}

/// Generates a project from the form's folder + name and opens it.
///
/// The generation is `cubic-cli new`'s: same templates, same engine-dependency
/// detection, so a project made here is one the CLI can also run.
fn create_project(state: &mut EditorState, form: &NewProjectForm) -> Result<(), String> {
    let name = scaffold::crate_name(form.name.trim())?;
    let root = form.parent.join(&name);
    let engine = EngineSource::resolve(None).map_err(|error| error.to_string())?;
    let generated = scaffold::generate(&root, &name, &engine, false).map_err(|error| {
        // `generate` refuses a non-empty directory before writing anything, so
        // the folder it found is still exactly as it was.
        error.to_string()
    })?;
    let count = generated.files.len();
    open_project(state, &generated.root)?;
    state.log(
        LogLevel::Info,
        format!("generated {count} files for `{name}`"),
    );
    Ok(())
}

/// The "delete this for real?" window, started from a tree context menu.
fn delete_window(ctx: &egui::Context, state: &mut EditorState) {
    let Some(path) = state.tree.confirm_delete.clone() else {
        return;
    };
    if state.project_root().is_some_and(|root| root == path) {
        state.tree.confirm_delete = None;
        return;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let is_dir = path.is_dir();
    let mut open = true;
    let mut confirm = false;
    let mut cancel = false;
    egui::Window::new("Delete")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(format!(
                "Delete `{name}`{}?",
                if is_dir { " and everything in it" } else { "" }
            ));
            ui.colored_label(
                egui::Color32::from_rgb(0xe5, 0x9b, 0x3b),
                "This cannot be undone.",
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new("Delete").fill(egui::Color32::from_rgb(0x8e, 0x2f, 0x2f)),
                    )
                    .clicked()
                {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

    if !open || cancel {
        state.tree.confirm_delete = None;
        return;
    }
    if !confirm {
        return;
    }
    state.tree.confirm_delete = None;
    if let Err(error) = tree::delete(&path) {
        state.log(LogLevel::Error, error);
        return;
    }
    state.log(LogLevel::Info, format!("deleted {}", shown(state, &path)));
    // Anything open inside the deleted path is gone with it.
    if state
        .scene
        .as_ref()
        .is_some_and(|scene| scene.path.starts_with(&path))
    {
        state.scene = None;
        state.play.cancel();
        state.edits.clear();
        state.hierarchy.clear();
    }
    if state
        .preview
        .as_ref()
        .is_some_and(|preview| preview.path.starts_with(&path))
    {
        state.preview = None;
    }
    if matches!(&state.selection, Selection::File(file) if file.starts_with(&path)) {
        state.selection = Selection::None;
    }
}

/// How a path is named in the log: relative to the project when it is inside.
pub fn shown(state: &EditorState, path: &Path) -> String {
    match state.project_root() {
        Some(root) if path.starts_with(root) => path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string(),
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::PreviewKind;
    use crate::scratch::Scratch;
    use cubic_core::Transform;
    use cubic_core::math::Vec2;
    use cubic_core::scene::SceneRegistry;
    use cubic_core::world::World;

    /// A minimal manifest — every field but the name has a default.
    fn manifest(name: &str) -> String {
        format!("[game]\nname = \"{name}\"\n")
    }

    /// A scene file body with nothing in it.
    fn empty_scene_text() -> String {
        Scene {
            version: Scene::VERSION,
            entities: vec![],
        }
        .to_text()
        .expect("the empty scene serializes")
    }

    #[test]
    fn extensions_decide_what_opens() {
        let cases = [
            ("assets/scenes/main.rsn", Some(OpenKind::Scene)),
            ("assets/sprite.png", Some(OpenKind::Image)),
            ("assets/photo.jpeg", Some(OpenKind::Image)),
            ("src/game.rs", Some(OpenKind::Text)),
            ("game.toml", Some(OpenKind::Text)),
            ("Cargo.lock", Some(OpenKind::Text)),
            ("README", Some(OpenKind::Text)),
            ("assets/model.mesh", None),
            ("weird.RSN", Some(OpenKind::Scene)),
        ];
        for (path, expected) in cases {
            assert_eq!(classify(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn a_directory_with_a_manifest_opens_and_a_stranger_does_not() {
        let scratch = Scratch::new("open-project");
        let root = scratch.join("demo");
        std::fs::create_dir_all(&root).expect("project dir");
        std::fs::write(root.join("game.toml"), manifest("demo")).expect("manifest");

        let mut state = EditorState::new();
        open_project(&mut state, &root).expect("the manifest says it is a project");
        assert_eq!(state.project_root(), Some(root.as_path()));
        assert_eq!(state.project_name(), Some("demo"));
        assert!(
            state.tree.is_expanded(&root),
            "the project root starts open in the tree"
        );

        // The manifest path opens the same project.
        let mut by_manifest = EditorState::new();
        open_project(&mut by_manifest, &root.join("game.toml")).expect("by manifest");
        assert_eq!(by_manifest.project_root(), Some(root.as_path()));

        let empty = scratch.join("not-a-project");
        std::fs::create_dir_all(&empty).expect("dir");
        let error = open_project(&mut EditorState::new(), &empty).unwrap_err();
        assert!(error.contains("game.toml"), "{error}");
    }

    #[test]
    fn closing_a_project_clears_what_it_held() {
        let scratch = Scratch::new("close-project");
        let root = scratch.join("demo");
        std::fs::create_dir_all(&root).expect("project dir");
        std::fs::write(root.join("game.toml"), manifest("demo")).expect("manifest");

        let mut state = EditorState::new();
        open_project(&mut state, &root).expect("open");
        state.scene = Some(OpenScene {
            path: root.join("assets/scenes/main.rsn"),
            world: World::new(),
        });
        state.selection = Selection::File(root.join("game.toml"));

        close_project(&mut state);
        assert!(state.project.is_none());
        assert!(state.scene.is_none());
        assert!(matches!(state.selection, Selection::None));
    }

    #[test]
    fn new_project_generates_one_the_cli_rules_accept() {
        let scratch = Scratch::new("new-project");
        let mut state = EditorState::new();
        let form = NewProjectForm {
            parent: scratch.path.clone(),
            name: "my-game".to_owned(),
            focus: false,
            error: None,
        };

        create_project(&mut state, &form).expect("generated and opened");
        let root = scratch.join("my-game");
        assert_eq!(state.project_root(), Some(root.as_path()));
        assert_eq!(state.project_name(), Some("my-game"));
        for file in ["game.toml", "Cargo.toml", "src/main.rs"] {
            assert!(root.join(file).is_file(), "{file} missing");
        }
        // The generated crate is the project's name, or cargo would refuse it.
        Project::open(&root)
            .expect("reopen")
            .check_crate_name()
            .expect("crate name matches game.toml");

        let bad = NewProjectForm {
            parent: scratch.path.clone(),
            name: "not a name".to_owned(),
            focus: false,
            error: None,
        };
        let error = create_project(&mut EditorState::new(), &bad).unwrap_err();
        assert!(error.contains("crate name"), "{error}");
    }

    #[test]
    fn opening_a_scene_loads_its_world_and_a_text_file_previews() {
        let scratch = Scratch::new("open-file");
        let root = scratch.join("demo");
        std::fs::create_dir_all(root.join("assets/scenes")).expect("dirs");
        std::fs::write(root.join("game.toml"), manifest("demo")).expect("manifest");
        let scene = root.join("assets/scenes/main.rsn");
        let mut world = World::new();
        let hero = world.spawn();
        world.insert(hero, Transform::from_position(Vec2::new(3.0, 4.0)));
        let text = Scene::from_world(&world, &SceneRegistry::engine_defaults())
            .expect("a scene from a world")
            .to_text()
            .expect("serializes");
        std::fs::write(&scene, text).expect("scene file");
        let notes = root.join("notes.txt");
        std::fs::write(&notes, "hello").expect("notes");

        let mut state = EditorState::new();
        open_project(&mut state, &root).expect("open");
        let ctx = egui::Context::default();

        open_file(&mut state, &ctx, scene.clone());
        let open = state.scene.as_ref().expect("a scene is open");
        assert_eq!(open.path, scene);
        assert_eq!(
            open.world.get::<Transform>(0),
            Some(&Transform::from_position(Vec2::new(3.0, 4.0))),
            "opening a scene loads its entities"
        );
        assert!(!state.dirty, "a fresh open is not modified");
        assert!(state.preview.is_none(), "a scene is not a preview");

        open_file(&mut state, &ctx, notes.clone());
        assert_eq!(
            state.scene.as_ref().map(|open| open.path.clone()),
            Some(scene.clone()),
            "peeking at a file must not close the scene underneath"
        );
        let preview = state.preview.as_ref().expect("a preview");
        assert_eq!(preview.path, notes);
        match &preview.kind {
            PreviewKind::Text { text, .. } => assert_eq!(text, "hello"),
            other => panic!("expected text, got {other:?}"),
        }

        // An unknown extension is selected with a warning; it opens nothing, so
        // whatever was previewing stays up.
        let mesh = root.join("model.mesh");
        std::fs::write(&mesh, "???").expect("mesh");
        let before = state.console.len();
        open_file(&mut state, &ctx, mesh.clone());
        assert_eq!(
            state.preview.as_ref().map(|preview| preview.path.clone()),
            Some(notes.clone())
        );
        assert_eq!(state.selection, Selection::File(mesh));
        assert!(
            state.console[before..]
                .iter()
                .any(|line| line.level == LogLevel::Warn),
            "the console should say there is no preview"
        );
    }

    #[test]
    fn a_broken_scene_is_refused_and_keeps_the_previous_one() {
        let scratch = Scratch::new("broken-scene");
        let root = scratch.join("demo");
        std::fs::create_dir_all(root.join("assets/scenes")).expect("dirs");
        std::fs::write(root.join("game.toml"), manifest("demo")).expect("manifest");
        let scene = root.join("assets/scenes/main.rsn");
        std::fs::write(&scene, empty_scene_text()).expect("scene file");
        let broken = root.join("assets/scenes/broken.rsn");
        std::fs::write(&broken, "not a scene").expect("broken scene");

        let mut state = EditorState::new();
        open_project(&mut state, &root).expect("open");
        let ctx = egui::Context::default();
        open_file(&mut state, &ctx, scene.clone());
        assert!(state.scene.is_some(), "the good scene opened");

        open_file(&mut state, &ctx, broken);
        // The failed open left the previous scene in place.
        assert_eq!(
            state.scene.as_ref().map(|open| open.path.clone()),
            Some(scene)
        );
    }

    #[test]
    fn saving_writes_the_open_scene_back_to_disk() {
        let scratch = Scratch::new("save-scene");
        let root = scratch.join("demo");
        std::fs::create_dir_all(root.join("assets/scenes")).expect("dirs");
        std::fs::write(root.join("game.toml"), manifest("demo")).expect("manifest");
        let scene = root.join("assets/scenes/main.rsn");
        std::fs::write(&scene, empty_scene_text()).expect("scene file");

        let mut state = EditorState::new();
        open_project(&mut state, &root).expect("open");
        let ctx = egui::Context::default();
        open_file(&mut state, &ctx, scene.clone());
        state.dirty = true;

        let spawned = {
            let open = state.scene.as_mut().expect("a scene");
            let id = open.world.spawn();
            open.world
                .insert(id, Transform::from_position(Vec2::new(5.0, 6.0)));
            id
        };
        save_scene(&mut state);

        assert!(!state.dirty, "a save clears the dirty flag");
        let saved = Scene::load(&scene).expect("the saved scene parses");
        let world = saved
            .to_world(&SceneRegistry::engine_defaults())
            .expect("loads")
            .world;
        assert_eq!(
            world.get::<Transform>(spawned),
            Some(&Transform::from_position(Vec2::new(5.0, 6.0)))
        );
    }

    /// The path shown in the log is the one inside the project, not the whole
    /// absolute run to it.
    #[test]
    fn paths_inside_the_project_are_shown_short() {
        let scratch = Scratch::new("shown");
        let root = scratch.join("demo");
        std::fs::create_dir_all(&root).expect("dir");
        std::fs::write(root.join("game.toml"), manifest("demo")).expect("manifest");

        let mut state = EditorState::new();
        open_project(&mut state, &root).expect("open");
        assert_eq!(shown(&state, &root.join("game.toml")), "game.toml");
        assert_eq!(
            shown(&state, Path::new("/elsewhere/file.txt")),
            "/elsewhere/file.txt"
        );
    }
}
