//! The desktop editor shell for Cubic-Engine projects.
//!
//! [`run`] opens the editor window (eframe on the wgpu backend `cubic-render`
//! targets), with a dockable layout of plain-function panels driven by
//! [`state::EditorState`]. A project (`game.toml`) opens and is created through
//! [`project`], browsed in a file tree ([`tree`]), opened files preview in the
//! viewport ([`preview`]), and the open scene renders through the engine into
//! that same viewport ([`viewport`]).
//!
//! Desktop-only: the crate compiles to an empty unit on wasm so the workspace's
//! `wasm32-unknown-unknown` build stays green (the editor ports to wasm later).

#![cfg(not(target_arch = "wasm32"))]

pub mod app;
pub mod dock;
pub mod logging;
pub mod panels;
pub mod play;
pub mod preview;
pub mod project;
pub mod run;
pub mod state;
pub mod tree;
pub mod viewport;

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
    logging::install();
    eframe::run_native(
        "Cubic Editor",
        options,
        Box::new(|cc| Ok(Box::new(app::EditorApp::new(cc)))),
    )
}

/// A scratch directory that removes itself, named after the test and the clock
/// so concurrent runs cannot collide.
#[cfg(test)]
pub(crate) mod scratch {
    use std::path::{Path, PathBuf};

    pub struct Scratch {
        pub path: PathBuf,
    }

    impl Scratch {
        pub fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "cubic-editor-{}-{label}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.as_nanos())
                    .unwrap_or_default(),
            ));
            std::fs::create_dir_all(&path).expect("scratch directory");
            Self { path }
        }

        pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
            self.path.join(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
