//! The editor's state model, kept independent of the UI layer.
//!
//! Panels are plain functions of [`EditorState`]; the dockable layout lives in
//! the same struct so the whole editor state can be persisted with one `serde`
//! round-trip (see `app::EditorApp::save`).

use std::path::PathBuf;

use egui_dock::{DockState, NodeIndex};

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

/// What the viewport currently has selected.
#[derive(Clone, Debug, Default)]
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

/// Cap on retained console lines; keeps the editor lean over long sessions.
const MAX_CONSOLE_LINES: usize = 500;

/// All editor state that outlives a single frame.
pub struct EditorState {
    /// Open project root, when one is loaded.
    pub project: Option<PathBuf>,
    /// Open scene file, when one is loaded.
    pub scene: Option<PathBuf>,
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
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            project: None,
            scene: None,
            selection: Selection::None,
            dirty: false,
            dock: default_dock(),
            console: Vec::new(),
            console_input: String::new(),
            status: String::new(),
        }
    }
}

impl EditorState {
    pub fn new() -> Self {
        Self::default()
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

    /// Marks the open scene as saved. Scene serialization itself lands with the
    /// scene model; this only clears the dirty flag for the shell.
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
        let mut state = EditorState::default();
        state.dock = DockState::new(vec![Pane::Viewport]);
        state.reset_dock();
        assert!(tab_set(&state).contains(&Pane::Project));
        assert!(tab_set(&state).contains(&Pane::Console));
    }
}
