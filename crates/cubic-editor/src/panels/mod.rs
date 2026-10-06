//! Plain-function panels: each `fn(&mut egui::Ui, &mut EditorState)` owns one
//! region of the editor chrome.

mod console;
mod inspector;
mod menu;
mod project;
mod status;
mod viewport;

pub use console::console;
pub use inspector::inspector;
pub use menu::menu_bar;
pub use project::project;
pub use status::status_bar;
pub use viewport::viewport;
