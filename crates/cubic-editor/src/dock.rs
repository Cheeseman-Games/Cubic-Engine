//! Bridges the dockable layout and the plain-function panels.

use egui::{Id, Ui, WidgetText};
use egui_dock::{DockArea, DockState, Style, TabViewer};

use crate::panels;
use crate::state::{EditorState, Pane};

/// `egui_dock`'s view of the editor: routes a pane to its panel function.
pub struct EditorTabViewer<'a> {
    pub state: &'a mut EditorState,
}

impl<'a> TabViewer for EditorTabViewer<'a> {
    type Tab = Pane;

    fn id(&mut self, tab: &mut Pane) -> Id {
        // Panes are single-instance with stable identities, so the pane enum
        // itself is a stable id.
        Id::new(*tab)
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Pane) {
        match *tab {
            Pane::Project => panels::project(ui, self.state),
            Pane::Viewport => panels::viewport(ui, self.state),
            Pane::Inspector => panels::inspector(ui, self.state),
            Pane::Console => panels::console(ui, self.state),
        }
    }

    fn title(&mut self, tab: &mut Pane) -> WidgetText {
        tab.title().into()
    }

    fn allowed_in_windows(&self, _tab: &mut Pane) -> bool {
        // The chrome is a fixed four-pane layout; floating windows stay off.
        false
    }

    fn is_closeable(&self, _tab: &Pane) -> bool {
        false
    }
}

/// Hosts the dock in the central region for one frame.
///
/// The dock state lives inside [`EditorState`] so it persists with the rest of
/// the editor state, but `DockArea` owns the dock while the viewer borrows the
/// state; the dock is therefore lifted out of the struct for the frame and put
/// back afterwards. It is called with the root `Ui` eframe hands the app.
pub fn show_docked(ui: &mut Ui, state: &mut EditorState) {
    let mut dock = std::mem::replace(&mut state.dock, DockState::new(Vec::new()));
    let style = Style::from_egui(ui.style());
    egui::CentralPanel::default().show(ui, |ui| {
        let mut viewer = EditorTabViewer { state };
        DockArea::new(&mut dock)
            .style(style)
            .show_inside(ui, &mut viewer);
    });
    state.dock = dock;
}
