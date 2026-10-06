//! The desktop editor shell for Cubic-Engine projects.
//!
//! [`run`] opens the editor window (eframe on the wgpu backend `cubic-render`
//! targets), with a dockable layout of plain-function panels driven by
//! [`state::EditorState`].
//!
//! Desktop-only: the crate compiles to an empty unit on wasm so the workspace's
//! `wasm32-unknown-unknown` build stays green (the editor ports to wasm later).

#![cfg(not(target_arch = "wasm32"))]

pub mod app;
pub mod dock;
pub mod panels;
pub mod state;

pub use app::EditorApp;
pub use state::EditorState;

/// Opens the editor with the default window layout. Returns the `eframe` run
/// result, which is an error if the event loop fails to start.
pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Cubic Editor")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 640.0]),
        persist_window: true,
        ..Default::default()
    };
    run_with_options(options)
}

/// Opens the editor with explicit eframe options.
pub fn run_with_options(options: eframe::NativeOptions) -> eframe::Result<()> {
    eframe::run_native(
        "Cubic Editor",
        options,
        Box::new(|cc| Ok(Box::new(app::EditorApp::new(cc)))),
    )
}
