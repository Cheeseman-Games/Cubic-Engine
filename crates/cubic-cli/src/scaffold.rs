//! The generator: `cubic-cli new <name>`.
//!
//! A generated project is an ordinary crate — `Cargo.toml`, `src/`, `assets/` —
//! plus the `game.toml` that describes it. The templates here are the whole
//! project: a manifest that runs as-is, an entry point that reads it, a game
//! that ticks and draws, one system, and a placeholder scene.
//!
//! The templates are filled with `{{name}}`-style placeholders rather than
//! `format!` because they are mostly Rust source, and Rust source is full of
//! braces. Nothing is generated from the manifest schema itself: the manifest
//! template and the runtime that reads it are written by hand and kept honest by
//! the tests, which parse the generated manifest with the same parser a real
//! project is parsed with.

use std::path::{Path, PathBuf};

use crate::Error;
use crate::args::NewArgs;
use crate::engine::EngineSource;

/// Longest crate name cargo accepts.
const MAX_NAME_LEN: usize = 64;

/// A generated project on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    /// The project directory, relative to the working directory it was made in.
    pub root: PathBuf,
    /// Every file written, in write order.
    pub files: Vec<PathBuf>,
}

/// Carry out a `new` invocation: generate, then say what to do next.
pub fn run(new: NewArgs) -> Result<(), Error> {
    let name = name_of(&new.path).map_err(Error::Usage)?;
    let engine = EngineSource::resolve(new.engine.as_deref())?;
    let generated = generate(&new.path, &name, &engine, new.force)?;

    println!("generated {}:", generated.root.display());
    for file in &generated.files {
        let shown = file
            .strip_prefix(&generated.root)
            .unwrap_or(file)
            .display()
            .to_string();
        println!("  {shown}");
    }
    println!(
        "\nengine dependency: {}",
        engine.dependency(&generated.root)
    );
    println!("\nnext:\n  cubic-cli run {}", generated.root.display());
    Ok(())
}

/// The crate name a path implies: its last component, which is what cargo would
/// call the package.
fn name_of(path: &Path) -> Result<String, String> {
    let last = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "`{}` has no directory name to call the crate",
                path.display()
            )
        })?;
    crate_name(last)
}

/// Write a project called `name` into `root`.
///
/// The destination is either missing or empty unless `force` is set, and the
/// check happens before anything is written: a refused generation leaves the
/// directory exactly as it found it, and a half-written project is worse than no
/// project at all.
pub fn generate(
    root: &Path,
    name: &str,
    engine: &EngineSource,
    force: bool,
) -> Result<Generated, Error> {
    if root.is_file() {
        return Err(Error::Project(format!(
            "{} is a file, not a project directory",
            root.display()
        )));
    }
    if !force && !is_empty(root) {
        return Err(Error::Project(format!(
            "{} is not empty; pass --force to generate into it anyway, or generate a project \
             somewhere else",
            root.display()
        )));
    }

    let files = files(root, name, &pascal_case(name), engine);
    for (path, contents) in &files {
        Error::write(path, contents)?;
    }

    Ok(Generated {
        root: root.to_path_buf(),
        files: files.into_iter().map(|(path, _)| path).collect(),
    })
}

/// Whether `dir` holds nothing at all. A directory that does not exist counts as
/// empty: that is the usual case.
fn is_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(true)
}

/// The files a project is made of, each with its contents, already rooted.
///
/// A function rather than a pile of `write` calls so the project's shape can be
/// asserted on directly, and so the clash check and the writing cannot disagree
/// about what the project contains.
fn files(
    root: &Path,
    name: &str,
    game_struct: &str,
    engine: &EngineSource,
) -> Vec<(PathBuf, String)> {
    let dependency = engine.dependency(root);
    let values = [
        ("name", name),
        ("Game", game_struct),
        ("engine_dep", dependency.as_str()),
    ];
    let mut files = Vec::new();
    for (relative, template) in [
        ("game.toml", MANIFEST),
        ("Cargo.toml", CARGO_MANIFEST),
        (".gitignore", GITIGNORE),
        ("README.md", README),
        ("src/main.rs", MAIN_RS),
        ("src/game.rs", GAME_RS),
        ("src/systems/mod.rs", SYSTEMS_RS),
        ("assets/scenes/main.rsn", SCENE_RSN),
    ] {
        files.push((root.join(relative), fill(template, &values)));
    }
    files
}

/// Substitute `{{key}}` placeholders in `template`.
///
/// Deliberately not `format!`: the templates are Rust, so every literal brace in
/// them would have to be doubled, and the files being generated would be
/// unreadable in the source of the generator.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut filled = template.to_string();
    for (key, value) in values {
        filled = filled.replace(&format!("{{{{{key}}}}}"), value);
    }
    filled
}

/// Check `name` against cargo's rules for a package name, and return it trimmed.
///
/// The name becomes a crate name, a directory name and a binary name, so it has
/// to survive all three. Cargo allows a leading dash; requiring a leading
/// alphanumeric does not cost a real project anything and keeps the CLI from
/// generating something that reads like an option.
pub fn crate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a project needs a name".to_string());
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(format!(
            "`{name}` is longer than {MAX_NAME_LEN} characters, which is cargo's limit"
        ));
    }
    let shape = "letters, digits, `-` and `_`";
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("`{name}` cannot be a crate name: use only {shape}"));
    }
    if !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return Err(format!(
            "`{name}` cannot be a crate name: it must start with a letter or digit, not `-` or `_`"
        ));
    }
    Ok(name.to_string())
}

/// `my_game` becomes `MyGame`: the game's type name, from its crate name.
///
/// A name that starts with a digit cannot be a type name either, so the result
/// is prefixed rather than mangled.
pub fn pascal_case(name: &str) -> String {
    let joined: String = name
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect();
    if joined.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{joined}")
    } else {
        joined
    }
}

/// The generated `game.toml`.
const MANIFEST: &str = r##"# {{name}} — the project manifest.
#
# What the engine is told before the game runs. Every field but the name has a
# default, so delete anything you do not need; edit a value and re-run, because
# this file is compiled into the game.

[game]
name = "{{name}}"
version = "0.1.0"
tick_hz = 60
# icon = "assets/icon.png"

[window]
title = "{{name}}"
width = 960
height = 540
resizable = true
clear_color = "#121721"

[features]
# Engine features built into this project. `web` compiles the wasm canvas
# backend alongside the desktop one.
web = false
"##;

/// The generated `Cargo.toml`.
const CARGO_MANIFEST: &str = r#"# Generated by `cubic-cli new {{name}}`.
#
# A game project is an ordinary crate, so cargo is the build system: `cubic-cli`
# is a frontend for it, not a replacement.

[package]
name = "{{name}}"
version = "0.1.0"
edition = "2024"

[dependencies]
cubic-render = { {{engine_dep}}, features = ["manifest"] }

# Its own workspace, so the project builds and caches independently of whatever
# directory it was created in — including inside an engine checkout.
[workspace]
"#;

/// The generated `.gitignore`.
const GITIGNORE: &str = "\
/target
";

/// The generated `README.md`.
const README: &str = r#"# {{name}}

A game project for the cubic engine.

```sh
cubic-cli run .              # build and play
cubic-cli build --release .  # build without playing
```

| Path | What it is |
| --- | --- |
| `game.toml` | The project manifest: window, tick rate, features |
| `src/game.rs` | The game — state, and what a tick and a frame do |
| `src/systems/` | Systems: the steps a tick is made of |
| `assets/` | Scenes and everything else the game loads |

`game.toml` is compiled into the game, so editing it rebuilds and re-runs with
the new values. The whole surface a game touches is `cubic_render::prelude`.
"#;

/// The generated `src/main.rs`.
const MAIN_RS: &str = r#"//! {{name}} — the entry point.
//!
//! The manifest is compiled into the game, so the window this opens in and the
//! rate it ticks at come from `game.toml` rather than from code. `game.toml` is
//! a build input, so editing it rebuilds the game.
//!
//! `engine_main!({{Game}}::new())` is the whole entry point when the defaults are
//! good enough — it takes the title from the crate name and the rest from the
//! engine. This form is the one that lets the manifest decide.

mod game;
mod systems;

use cubic_render::manifest::ProjectManifest;
use cubic_render::run_project;

fn main() {
    let manifest = ProjectManifest::embedded(include_str!("../game.toml"));
    run_project(game::{{Game}}::new(), &manifest).expect("cubic: application ended early");
}
"#;

/// The generated `src/game.rs`.
const GAME_RS: &str = r#"//! The game: what it owns, and what each tick and each frame does with it.
//!
//! This is the project's script. `update` runs on a fixed step — 60 times a
//! second at `dt = 1/60`, whatever the display happens to be doing — and `draw`
//! runs once per presented frame.

use cubic_render::prelude::*;

use crate::systems;

/// Everything the game owns: the world its systems act on, and those systems.
pub struct {{Game}} {
    world: World,
    systems: systems::Systems,
    /// The entity this project's one system moves.
    player: EntityId,
}

impl {{Game}} {
    /// Runs once, before the window opens.
    pub fn new() -> Self {
        let mut world = World::new();

        // The scene's entities go here. `assets/scenes/main.rsn` is the file an
        // authored scene lands in: the editor opens it from this path and
        // writes back to it, so hand-authored entities would be overwritten.
        let player = world.spawn();
        world.insert(player, Transform::from_position(Vec2::ZERO));

        Self {
            world,
            systems: systems::Systems::default(),
            player,
        }
    }
}

impl Default for {{Game}} {
    fn default() -> Self {
        Self::new()
    }
}

impl Game for {{Game}} {
    /// One simulation step.
    fn update(&mut self, dt: f32, input: &InputState, frame: &FrameInput) {
        // Systems are handed the world, the input and the step together — the
        // same bundle every system in the engine sees.
        let mut ctx = TickContext {
            world: &mut self.world,
            input,
            frame,
            dt,
        };
        self.systems.tick(&mut ctx);
    }

    /// One presented frame. Everything on screen comes from here.
    fn draw(&mut self, list: &mut DrawList) {
        list.clear(Rgba::rgb(0.06, 0.08, 0.12));

        // Draw what the world holds, not what the game holds: read the entity's
        // transform back out and place the rectangle with it.
        if let Some(transform) = self.world.get::<Transform>(self.player) {
            list.fill_rect(
                transform.position.x - 24.0,
                transform.position.y - 24.0,
                48.0,
                48.0,
                Rgba::rgb(1.0, 0.55, 0.25),
            );
        }

        list.text("{{name}}", 24.0, 24.0, 32.0, Rgba::rgb(0.94, 0.95, 1.0));
        list.text(
            &format!(
                "t = {:.1}s — close the window to quit",
                self.systems.drift.seconds
            ),
            24.0,
            64.0,
            18.0,
            Rgba::rgb(0.7, 0.78, 0.9),
        );
    }

    /// The backdrop, for frames that do not clear themselves.
    fn clear_color(&self) -> Rgba {
        Rgba::rgb(0.06, 0.08, 0.12)
    }
}
"#;

/// The generated `src/systems/mod.rs`.
const SYSTEMS_RS: &str = r#"//! Systems: the steps a simulation tick is made of.
//!
//! A `System` is handed the world, the input and the fixed `dt`, and does one
//! piece of gameplay. Hold them in [`Systems`] and run them from `Game::update`
//! — the order of the calls in [`Systems::tick`] is the order they run in.

use cubic_render::prelude::*;

/// The middle of the window, in the coordinates the window opens at.
///
/// A system that orbits something needs a centre, and this is where the
/// manifest's `[window]` size puts it. A real game takes it from a camera
/// component instead.
const ARENA: Vec2 = Vec2::new(480.0, 300.0);

/// The systems this game runs, in the order a tick runs them.
#[derive(Default)]
pub struct Systems {
    /// Fixed-step motion for the player's cube.
    pub drift: Drift,
}

impl Systems {
    /// Run every system, once, for one step.
    pub fn tick(&mut self, ctx: &mut TickContext<'_>) {
        self.drift.run(ctx);
    }
}

/// Circles the player's cube around the arena at a constant speed.
///
/// Driven by the fixed step rather than the frame, so the motion is the same
/// speed on a 30 Hz laptop and a 240 Hz monitor — and the same every time the
/// game is run.
#[derive(Default)]
pub struct Drift {
    /// Seconds of simulation so far, which is what `draw` reports.
    pub seconds: f32,
}

impl System for Drift {
    fn run(&mut self, ctx: &mut TickContext<'_>) {
        self.seconds += ctx.dt;
        let angle = self.seconds * 0.8;

        // This project has one entity, so "the one with a transform" is it. With
        // more, a marker component would say which is which.
        for (_, transform) in ctx.world.iter_mut::<Transform>() {
            transform.position = ARENA + Vec2::new(180.0 * angle.cos(), 120.0 * angle.sin());
        }
    }
}
"#;

/// The generated scene placeholder.
///
/// An authored scene starts where an empty one does: `Scene(version: 1, ...)`
/// with no entities. It even is one — round-tripping this file through
/// `cubic_core::scene::Scene` is a test — so the editor opens a fresh project
/// to a scene it can already save.
const SCENE_RSN: &str = r#"// Scene: main
//
// Scenes are written by the editor: entities and their components, serialized
// in a diffable text format — one component per line, one entity per block.
// This one is empty, which is how a new level begins.
Scene(
    version: 1,
    entities: [],
)
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine source the tests render against: the checkout this CLI was
    /// built from, so the generated dependency points somewhere real.
    fn engine() -> EngineSource {
        EngineSource::default()
    }

    /// Every generated file, as `(relative path, contents)`.
    fn generated(name: &str) -> Vec<(String, String)> {
        let root = Path::new(name);
        files(root, name, &pascal_case(name), &engine())
            .into_iter()
            .map(|(path, contents)| {
                let relative = path
                    .strip_prefix(root)
                    .expect("generated files live under the project root")
                    .to_string_lossy()
                    .replace('\\', "/");
                (relative, contents)
            })
            .collect()
    }

    /// The contents of one generated file.
    fn file(name: &str, path: &str) -> String {
        generated(name)
            .into_iter()
            .find(|(relative, _)| relative == path)
            .unwrap_or_else(|| panic!("{path} is not generated"))
            .1
    }

    #[test]
    fn cargo_package_names_are_accepted() {
        for name in ["demo", "my-game", "my_game", "Game2", "2d-shooter", "a"] {
            assert_eq!(crate_name(name).unwrap(), name);
        }
    }

    /// `new` takes a path, and only the last component names the crate — so a
    /// directory of games needs no ceremony.
    #[test]
    fn a_crate_is_named_after_the_last_component_of_its_path() {
        assert_eq!(name_of(Path::new("demo")).unwrap(), "demo");
        assert_eq!(name_of(Path::new("../games/my-game")).unwrap(), "my-game");
        assert!(
            name_of(Path::new("/")).is_err(),
            "a filesystem root has no name to call the crate"
        );
        assert!(
            name_of(Path::new("games/my game")).is_err(),
            "the directory name still has to be a legal crate name"
        );
    }

    /// Surrounding whitespace is a paste accident, not a name.
    #[test]
    fn a_name_is_trimmed_before_it_is_used() {
        assert_eq!(crate_name("  demo \n").unwrap(), "demo");
    }

    #[test]
    fn a_name_that_cannot_be_a_crate_is_refused_with_a_reason() {
        for bad in [
            "",
            "   ",
            "my game",
            "my.game",
            "my/game",
            "café",
            "demo!",
            "../escape",
        ] {
            let error = crate_name(bad).unwrap_err();
            assert!(!error.is_empty(), "{bad:?} was refused without a reason");
        }
        assert!(
            crate_name("-demo").is_err(),
            "a leading dash reads like a flag"
        );
        assert!(
            crate_name("_demo").is_err(),
            "a leading underscore is not a name"
        );
        assert!(crate_name(&"a".repeat(MAX_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn a_crate_name_becomes_a_type_name() {
        assert_eq!(pascal_case("demo"), "Demo");
        assert_eq!(pascal_case("my-game"), "MyGame");
        assert_eq!(pascal_case("my_game"), "MyGame");
        assert_eq!(pascal_case("my-cool-game"), "MyCoolGame");
    }

    /// A name that starts with a digit cannot be a type name, and the generated
    /// source would not compile if it were used as one.
    #[test]
    fn a_name_that_starts_with_a_digit_still_becomes_a_valid_type_name() {
        let generated = pascal_case("2d-shooter");
        assert_eq!(generated, "_2dShooter");
        assert!(
            generated
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        );
    }

    /// The project layout the session calls for, spelled out so a change to it
    /// has to be deliberate.
    #[test]
    fn a_project_is_a_manifest_a_crate_a_game_a_system_and_an_assets_folder() {
        let paths: Vec<String> = generated("demo")
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        assert_eq!(
            paths,
            vec![
                "game.toml",
                "Cargo.toml",
                ".gitignore",
                "README.md",
                "src/main.rs",
                "src/game.rs",
                "src/systems/mod.rs",
                "assets/scenes/main.rsn",
            ]
        );
    }

    /// A generated project's empty scene must stay a scene the engine can
    /// both read and write, or a fresh project would corner its editor.
    #[test]
    fn the_generated_scene_is_a_valid_empty_scene() {
        let text = file("demo", "assets/scenes/main.rsn");
        let scene: cubic_core::scene::Scene = text
            .parse()
            .expect("the generated scene must parse as a scene");
        assert_eq!(scene.version, cubic_core::scene::Scene::VERSION);
        assert!(scene.entities.is_empty());
        let round = scene.to_text().expect("the empty scene must serialize");
        assert_eq!(round, "Scene(\n    version: 1,\n    entities: [],\n)\n");
    }

    /// A generated manifest that the engine's own parser refuses would be a
    /// project that cannot start, so the template is parsed here the same way a
    /// real one is.
    #[test]
    fn the_generated_manifest_is_usable_as_written() {
        let text = file("demo", "game.toml");
        let parsed = cubic_core::manifest::ProjectManifest::parse(&text)
            .expect("the generated manifest must parse");
        assert_eq!(parsed.game.name, "demo");
        assert_eq!(parsed.game.version, "0.1.0");
        assert_eq!(parsed.game.icon, None);
        assert_eq!(parsed.window.title, "demo");
        assert_eq!(parsed.window.width, 960);
        assert_eq!(parsed.window.height, 540);
        assert!(parsed.window.resizable);
        assert_eq!(
            parsed.enabled_features().collect::<Vec<_>>(),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn the_generated_manifest_names_the_project_it_generates() {
        let parsed =
            cubic_core::manifest::ProjectManifest::parse(&file("cool-game", "game.toml")).unwrap();
        assert_eq!(parsed.game.name, "cool-game");
        assert_eq!(parsed.window_title(), "cool-game");
    }

    /// The crate name is what cargo calls the project, so the two files that
    /// name it have to agree — which is what `Project::check_crate_name` later
    /// enforces on a real project.
    #[test]
    fn the_generated_cargo_manifest_and_manifest_agree_on_the_name() {
        let cargo = file("demo", "Cargo.toml");
        let manifest =
            cubic_core::manifest::ProjectManifest::parse(&file("demo", "game.toml")).unwrap();

        let parsed: toml::Table = toml::from_str(&cargo).expect("generated Cargo.toml is TOML");
        assert_eq!(
            parsed["package"]["name"].as_str(),
            Some(manifest.game.name.as_str())
        );
        assert_eq!(parsed["package"]["edition"].as_str(), Some("2024"));
        assert!(
            parsed["workspace"].is_table(),
            "a project must be its own workspace root"
        );
    }

    /// The generated crate depends on the engine, and on nothing else: its
    /// prelude re-exports the core, so one dependency is the whole engine
    /// surface a game needs.
    #[test]
    fn the_generated_crate_depends_on_the_engine_and_the_manifest_feature() {
        let parsed: toml::Table =
            toml::from_str(&file("demo", "Cargo.toml")).expect("generated Cargo.toml is TOML");
        let dependency = &parsed["dependencies"]["cubic-render"];
        assert!(
            dependency.get("path").is_some() || dependency.get("git").is_some(),
            "the engine dependency must name a source: {dependency:?}"
        );
        assert_eq!(
            dependency["features"][0].as_str(),
            Some("manifest"),
            "the generated code reads its manifest, so the feature must be on"
        );
    }

    #[test]
    fn the_generated_entry_point_runs_the_game_from_the_manifest() {
        let main = file("demo", "src/main.rs");
        assert!(main.contains("mod game;"));
        assert!(main.contains("mod systems;"));
        assert!(
            main.contains(r#"include_str!("../game.toml")"#),
            "the entry point must read the manifest from the project root: {main}"
        );
        assert!(main.contains("run_project(game::Demo::new(), &manifest)"));
    }

    #[test]
    fn the_generated_game_is_named_after_the_project() {
        let game = file("cool-game", "src/game.rs");
        assert!(game.contains("pub struct CoolGame"));
        assert!(game.contains("impl Game for CoolGame"));
        assert!(game.contains(r#"list.text("cool-game""#));
        assert!(!game.contains("{{"), "a placeholder survived generation");
    }

    /// The generated source is the engine's own contract: one glob import, one
    /// `Game` impl, a world its systems act on.
    #[test]
    fn the_generated_game_stays_inside_the_prelude() {
        let game = file("demo", "src/game.rs");
        assert!(game.contains("use cubic_render::prelude::*;"));
        assert!(!game.contains("cubic_core::"), "{game}");
        assert!(!game.contains("wgpu"), "{game}");
        assert!(!game.contains("winit"), "{game}");
    }

    #[test]
    fn the_generated_system_runs_on_the_fixed_step() {
        let systems = file("demo", "src/systems/mod.rs");
        assert!(systems.contains("impl System for Drift"));
        assert!(
            systems.contains("self.seconds += ctx.dt;"),
            "motion must be driven by the step, not the frame"
        );
        assert!(systems.contains("ctx.world.iter_mut::<Transform>()"));
    }

    /// Placeholders are substituted everywhere they appear, and ordinary braces
    /// — Rust's, and TOML's — are left alone.
    #[test]
    fn only_placeholders_are_substituted() {
        let filled = fill(
            "impl {{Game}} { fn tick(&mut self, ctx: &mut TickContext<'_>) {} }",
            &[("Game", "Demo")],
        );
        assert_eq!(
            filled,
            "impl Demo { fn tick(&mut self, ctx: &mut TickContext<'_>) {} }"
        );
    }

    #[test]
    fn no_generated_file_keeps_a_placeholder() {
        for (path, contents) in generated("my-game") {
            assert!(!contents.contains("{{"), "{path} kept a placeholder");
            assert!(!contents.contains("}}"), "{path} kept a placeholder");
        }
    }

    /// The templates are written for rustfmt: four spaces, no tabs, no trailing
    /// whitespace, and lines that fit the workspace's 100 columns. A template
    /// that ignores those ends up reformatted the first time a project builds.
    ///
    /// Only the Rust sources are held to the column limit: `Cargo.toml` is data,
    /// and an absolute dependency path is as long as the machine it lives on.
    #[test]
    fn the_generated_source_is_formatted_the_way_rustfmt_would_leave_it() {
        for (path, contents) in generated("demo") {
            for (number, line) in contents.lines().enumerate() {
                assert!(!line.contains('\t'), "{}:{} has a tab", path, number + 1);
                assert_eq!(
                    line.trim_end(),
                    line,
                    "{}:{} has trailing whitespace",
                    path,
                    number + 1
                );
                if path.ends_with(".rs") {
                    assert!(
                        line.chars().count() <= 100,
                        "{}:{} is {} columns wide",
                        path,
                        number + 1,
                        line.chars().count()
                    );
                }
            }
        }
    }

    /// The dependency line is allowed to be long, but the templates themselves are
    /// not: an editor would rewrap them.
    #[test]
    fn no_template_line_is_unreasonably_wide() {
        for (name, template) in [
            ("Cargo.toml", CARGO_MANIFEST),
            ("game.toml", MANIFEST),
            ("README.md", README),
        ] {
            for (number, line) in template.lines().enumerate() {
                assert!(
                    line.chars().count() <= 100,
                    "{name}:{number} in the template is {} columns wide",
                    line.chars().count()
                );
            }
        }
    }
}
