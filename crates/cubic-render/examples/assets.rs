//! Hot reload, end to end: load a texture, watch the file, and repaint the sprite
//! while the window stays up.
//!
//! Run it with the project root as its argument, or from a directory containing
//! `assets/logo.png`:
//!
//! ```text
//! cargo run -p cubic-render --example assets --features "wgpu,watch" -- assets
//! ```
//!
//! Edit `assets/logo.png` while it runs. The sprite changes within a frame or two
//! and nothing else has to happen — no window reload, no handle changing, no code
//! re-reading the file. That is the point of the pipeline: the game holds a
//! handle, and the handle's pixels change underneath it.
//!
//! The window closes on Escape.

use cubic_render::prelude::*;

struct Viewer {
    assets: AssetServer,
    logo: Option<TextureHandle>,
    /// The version last drawn, so a reload can be reported rather than guessed at.
    seen: Option<u32>,
    events: Vec<String>,
    message: String,
}

impl Viewer {
    fn new(root: std::path::PathBuf) -> Self {
        // A watcher needs an operating system to ask, and the `watch` feature is
        // what this example is built with. A directory server without one would
        // still load the image, just never notice an edit.
        let assets = match AssetServer::watching(root) {
            Ok(assets) => {
                log::info!("watching for edits");
                assets
            }
            Err(error) => {
                log::error!("could not watch: {error}");
                AssetServer::from_dir(std::env::current_dir().unwrap_or_default())
            }
        };
        Self {
            assets,
            logo: None,
            seen: None,
            events: Vec::new(),
            message: String::new(),
        }
    }
}

impl Game for Viewer {
    fn update(&mut self, _dt: f32, _input: &InputState, frame: &FrameInput) {
        if frame.pressed(KeyCode::Escape) {
            std::process::exit(0);
        }
        if self.logo.is_none() {
            match self.assets.load_texture("assets/logo.png") {
                Ok(handle) => {
                    log::info!("loaded {handle}");
                    self.logo = Some(handle);
                }
                Err(error) => {
                    // Not fatal: the file may still be on its way, and a missing
                    // asset should say so rather than end the program.
                    self.message = format!("{error}");
                    log::error!("{error}");
                }
            }
        }
        // The runtime drains pending imports; events are the game's to read, and
        // this is the game's answer to "what changed".
        for event in self.assets.drain_events() {
            let line = match &event {
                AssetEvent::Loaded { handle, path, .. } => format!("loaded {path} as {handle}"),
                AssetEvent::Reloaded { path, version, .. } => {
                    format!("{path} changed (version {version})")
                }
                AssetEvent::Failed { path, error } => format!("{path} failed: {error}"),
                AssetEvent::Released { path, .. } => format!("{path} was released"),
            };
            log::info!("{line}");
            self.events.push(line);
        }
        // Keep the log short enough to stay readable in a terminal.
        if self.events.len() > 6 {
            self.events.remove(0);
        }
    }

    fn draw(&mut self, list: &mut DrawList) {
        list.clear(Rgba::rgb(0.06, 0.07, 0.09));

        if let Some(logo) = self.logo {
            // A square 256 px sprite: the destination is the game's choice, which
            // is why the handle carries no size. A game that cares would ask the
            // backend once it has one.
            let side = 256.0;
            let y = 40.0;
            list.draw_texture(logo, 40.0, y, side, side, Rgba::rgb(1.0, 1.0, 1.0));

            self.seen = self.assets.version(logo.asset());
            list.text(
                &format!(
                    "version {} — this handle has not changed",
                    self.seen.unwrap_or(0)
                ),
                40.0,
                y + side + 20.0,
                16.0,
                Rgba::rgb(0.7, 0.75, 0.85),
            );
        } else {
            list.text("no texture yet", 40.0, 40.0, 20.0, Rgba::rgb(0.8, 0.4, 0.4));
        }

        if !self.message.is_empty() {
            list.text(&self.message, 40.0, 90.0, 16.0, Rgba::rgb(0.9, 0.5, 0.3));
        }

        list.text(
            "edit assets/logo.png to see it change",
            40.0,
            140.0,
            16.0,
            Rgba::rgb(0.5, 0.55, 0.6),
        );
        for (index, line) in self.events.iter().enumerate() {
            list.text(
                line,
                40.0,
                180.0 + index as f32 * 22.0,
                14.0,
                Rgba::rgb(0.65, 0.7, 0.75),
            );
        }
        list.text("esc to quit", 40.0, 420.0, 14.0, Rgba::rgb(0.4, 0.42, 0.45));
    }

    fn assets_mut(&mut self) -> Option<&mut AssetServer> {
        Some(&mut self.assets)
    }

    fn clear_color(&self) -> Rgba {
        Rgba::rgb(0.06, 0.07, 0.09)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // The first argument is the project root, so the example works from anywhere;
    // without one it assumes the current directory is the project.
    let root = match std::env::args().nth(1) {
        Some(arg) => std::path::PathBuf::from(arg),
        None => std::env::current_dir()?,
    };
    let config = WindowConfig {
        title: "cubic — assets".to_string(),
        ..Default::default()
    };
    run_game(Viewer::new(root), config)?;
    Ok(())
}
