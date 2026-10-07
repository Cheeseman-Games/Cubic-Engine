//! The editor's state model, kept independent of the UI layer.
//!
//! Panels are plain functions of [`EditorState`]; the dockable layout lives in
//! the same struct so the whole editor state can be persisted with one `serde`
//! round-trip (see `app::EditorApp::save`). Anything that must outlive a frame
//! — the open project, tree expansion, the active preview — is here.

use std::path::{Path, PathBuf};

use cubic_cli::Project;
use cubic_core::scene::SceneRegistry;
use cubic_core::world::World;
use egui_dock::{DockState, NodeIndex};

use crate::preview::Preview;
use crate::tree::TreeState;

/// A dockable editor pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Pane {
    Project,
    Viewport,
    Inspector,
    Console,
}

impl Pane {
    /// Display name for the pane's dock tab.
    pub fn title(self) -> &'static str {
        match self {
            Self::Project => "Project",
            Self::Viewport => "Viewport",
            Self::Inspector => "Inspector",
            Self::Console => "Console",
        }
    }
}

/// What the editor has selected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    None,
    /// A file or folder in the project tree.
    File(PathBuf),
    /// A scene entity id.
    Entity(u64),
}

/// Severity of a console line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// A single retained console line.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LogLine {
    pub level: LogLevel,
    pub text: String,
}

/// A file dialog a menu item or shortcut asked for, run once the frame's UI is
/// done (see [`crate::project::run_pending`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    NewProject,
    OpenProject,
}

/// The name form for `File > New Project…`, after a folder was picked.
pub struct NewProjectForm {
    /// The directory the new project is generated inside.
    pub parent: PathBuf,
    /// The typed project (and crate) name.
    pub name: String,
    /// Whether the name field still wants keyboard focus (first frame only).
    pub focus: bool,
    /// Why the last attempt failed, shown in the window.
    pub error: Option<String>,
}

/// Cap on retained console lines; keeps the editor lean over long sessions.
const MAX_CONSOLE_LINES: usize = 500;

/// A `.rsn` scene open for editing: where it lives and what it currently
/// holds. The world is only in memory until `project::save_scene` writes it
/// back out — that is what turns edits into a file.
pub struct OpenScene {
    pub path: PathBuf,
    pub world: World,
}

/// All editor state that outlives a single frame.
pub struct EditorState {
    /// The open project: its root directory and parsed `game.toml`.
    pub project: Option<Project>,
    /// The open scene's file path and world, when one is loaded.
    pub scene: Option<OpenScene>,
    /// Component names the scene files in this project may hold, and how each
    /// one travels between a `World` and a `.rsn` file.
    pub registry: SceneRegistry,
    /// The current selection.
    pub selection: Selection,
    /// Whether the open scene has unsaved changes.
    pub dirty: bool,
    /// The dockable panel layout.
    pub dock: DockState<Pane>,
    /// Retained console lines (most recent last).
    pub console: Vec<LogLine>,
    /// Console command input.
    pub console_input: String,
    /// Transient message shown in the status bar.
    pub status: String,
    /// Project tree expansion and inline editors.
    pub tree: TreeState,
    /// The file open in the viewport preview, if any.
    pub preview: Option<Preview>,
    /// Dialog waiting to run this frame.
    pub pending: Option<Pending>,
    /// The new-project name window, while it is up.
    pub new_project: Option<NewProjectForm>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            project: None,
            scene: None,
            registry: SceneRegistry::engine_defaults(),
            selection: Selection::None,
            dirty: false,
            dock: default_dock(),
            console: Vec::new(),
            console_input: String::new(),
            status: String::new(),
            tree: TreeState::default(),
            preview: None,
            pending: None,
            new_project: None,
        }
    }
}

impl EditorState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The open project's root directory, if a project is open.
    pub fn project_root(&self) -> Option<&Path> {
        self.project.as_ref().map(|project| project.root.as_path())
    }

    /// The open project's name from `game.toml`, if a project is open.
    pub fn project_name(&self) -> Option<&str> {
        self.project
            .as_ref()
            .map(|project| project.manifest.game.name.as_str())
    }

    /// Appends a line to the console, trimming the oldest lines past the cap.
    pub fn log(&mut self, level: LogLevel, text: impl Into<String>) {
        self.console.push(LogLine {
            level,
            text: text.into(),
        });
        if self.console.len() > MAX_CONSOLE_LINES {
            let overflow = self.console.len() - MAX_CONSOLE_LINES;
            self.console.drain(..overflow);
        }
    }

    /// Clears the dirty flag after `project::save_scene` has written the open
    /// scene back to its file.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
        self.status = "saved".to_owned();
        self.log(LogLevel::Info, "Changes saved");
    }

    /// Restores the default four-pane layout.
    pub fn reset_dock(&mut self) {
        self.dock = default_dock();
    }
}

/// The default four-pane layout: project on the left, viewport centre-top,
/// inspector on the right, console along the bottom.
///
/// Split `fraction`s describe the left/top child's share, so a fraction of
/// `0.22` on `split_left` gives the new (project) pane 22% of the width and the
/// viewport the rest.
fn default_dock() -> DockState<Pane> {
    let mut dock = DockState::new(vec![Pane::Viewport]);
    let tree = dock.main_surface_mut();
    let [viewport, _project] = tree.split_left(NodeIndex::root(), 0.22, vec![Pane::Project]);
    let [viewport, _inspector] = tree.split_right(viewport, 0.80, vec![Pane::Inspector]);
    let [_viewport, _console] = tree.split_below(viewport, 0.72, vec![Pane::Console]);
    dock
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_set(state: &EditorState) -> Vec<Pane> {
        state.dock.iter_all_tabs().map(|(_, tab)| *tab).collect()
    }

    #[test]
    fn default_dock_has_all_four_panes() {
        let tabs = tab_set(&EditorState::default());
        for expected in [
            Pane::Project,
            Pane::Viewport,
            Pane::Inspector,
            Pane::Console,
        ] {
            assert!(tabs.contains(&expected), "missing pane {expected:?}");
        }
    }

    #[test]
    fn console_is_capped() {
        let mut state = EditorState::default();
        for i in 0..600 {
            state.log(LogLevel::Info, format!("line {i}"));
        }
        assert_eq!(state.console.len(), MAX_CONSOLE_LINES);
        assert!(state.console.last().is_some_and(|l| l.text == "line 599"));
    }

    #[test]
    fn reset_dock_restores_default_layout() {
        let mut state = EditorState::new();
        state.dock = DockState::new(vec![Pane::Viewport]);
        state.reset_dock();
        assert!(tab_set(&state).contains(&Pane::Project));
        assert!(tab_set(&state).contains(&Pane::Console));
    }

    #[test]
    fn project_helpers_are_none_without_a_project() {
        let state = EditorState::default();
        assert_eq!(state.project_root(), None);
        assert_eq!(state.project_name(), None);
    }
}
