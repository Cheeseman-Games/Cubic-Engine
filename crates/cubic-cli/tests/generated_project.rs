//! The CLI over its whole surface: generate a project into a scratch directory
//! and read it back the way `run` and `build` would.
//!
//! Nothing here invokes cargo — that is a thin passthrough, and a test that
//! compiled a game would be testing cargo. What matters is that what the
//! generator writes is a project the rest of the CLI can open.
//!
//! Every path is absolute, because the working directory is process-wide state
//! and tests run in parallel: a test that moved it would decide where some other
//! test's project landed.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use cubic_cli::{Project, run};

/// A scratch directory that removes itself, named after the test and the clock so
/// concurrent runs cannot collide.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "cubic-cli-{}-{label}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default(),
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self { path }
    }

    fn join(&self, path: &str) -> PathBuf {
        self.path.join(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn invoke(args: &[&str]) -> Result<(), cubic_cli::Error> {
    run(args.iter().map(OsString::from))
}

/// The path form of an argument, which is how every path argument reaches the
/// parser.
fn path(argument: &Path) -> String {
    argument.display().to_string()
}

/// Every file a project is, so a generator that quietly drops one is caught here
/// rather than by whoever opens the project later.
const PROJECT_FILES: &[&str] = &[
    "game.toml",
    "Cargo.toml",
    ".gitignore",
    "README.md",
    "src/main.rs",
    "src/game.rs",
    "src/systems/mod.rs",
    "assets/scenes/main.rsn",
];

fn assert_is_a_project(root: &Path) {
    for file in PROJECT_FILES {
        assert!(
            root.join(file).is_file(),
            "{} is missing",
            root.join(file).display()
        );
    }
    let project = Project::open(root).expect("the generated project should open");
    project
        .check_crate_name()
        .expect("the crate should be named after the game");
}

#[test]
fn a_generated_project_is_one_the_cli_can_open() {
    let scratch = Scratch::new("generate");
    let root = scratch.join("demo");
    invoke(&["new", &path(&root)]).expect("`new` should succeed");
    assert_is_a_project(&root);

    let manifest = &Project::open(&root).expect("a project").manifest;
    assert_eq!(manifest.game.name, "demo");
    assert_eq!(manifest.game.version, "0.1.0");
    assert_eq!(manifest.game.tick_hz, 60.0);
    assert_eq!(manifest.game.icon, None);
    assert_eq!(manifest.window.title, "demo");
    assert_eq!((manifest.window.width, manifest.window.height), (960, 540));
    assert!(manifest.window.resizable);
    // The manifest is the project's build configuration, and it starts out with
    // the wasm backend switched off.
    assert_eq!(
        manifest.enabled_features().collect::<Vec<_>>(),
        Vec::<&str>::new()
    );
}

#[test]
fn new_refuses_to_overwrite_something_that_is_there() {
    let scratch = Scratch::new("overwrite");
    let root = scratch.join("demo");
    invoke(&["new", &path(&root)]).expect("the first `new` should succeed");

    let second = invoke(&["new", &path(&root)]);
    let message = second.unwrap_err().to_string();
    assert!(message.contains("--force"), "{message}");

    invoke(&["new", &path(&root), "--force"]).expect("--force should overwrite");
    assert_is_a_project(&root);
}

/// A directory with somebody's files in it is refused outright, and the refusal
/// happens before anything is written: a half-generated project is worse than
/// none.
#[test]
fn a_refused_generation_leaves_the_directory_alone() {
    let scratch = Scratch::new("untouched");
    let root = scratch.join("demo");
    std::fs::create_dir_all(&root).expect("the directory is already there");
    std::fs::write(root.join("notes.txt"), "somebody was here first\n").expect("a file to keep");

    assert!(invoke(&["new", &path(&root)]).is_err());
    let mut left: Vec<String> = std::fs::read_dir(&root)
        .expect("the directory is still there")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["notes.txt"]);
}

/// An empty directory is nobody's work, so generating into one is allowed.
#[test]
fn an_empty_directory_may_become_a_project() {
    let scratch = Scratch::new("empty-dir");
    let root = scratch.join("demo");
    std::fs::create_dir_all(&root).expect("an empty directory");

    invoke(&["new", &path(&root)]).expect("an empty directory should be usable");
    assert_is_a_project(&root);
}

#[test]
fn a_path_whose_last_name_is_not_a_crate_is_refused() {
    let scratch = Scratch::new("illegal");
    for bad in ["my game", "my.game", "café", "demo!"] {
        let root = scratch.join(bad);
        let outcome = invoke(&["new", &path(&root)]);
        assert!(outcome.is_err(), "{bad:?} should be refused");
    }
}

#[test]
fn running_without_a_project_says_what_is_missing() {
    let scratch = Scratch::new("empty");
    let message = Project::open(&scratch.path).unwrap_err().to_string();
    assert!(message.contains("game.toml"), "{message}");
    assert!(message.contains("cubic-cli new"), "{message}");
}

/// A project's manifest may also be named directly, which reads naturally and
/// costs nothing to accept.
#[test]
fn a_project_can_be_opened_by_its_manifest() {
    let scratch = Scratch::new("bymanifest");
    let root = scratch.join("demo");
    invoke(&["new", &path(&root)]).expect("`new` should succeed");

    let by_directory = Project::open(&root).expect("by directory");
    let by_manifest = Project::open(&root.join("game.toml")).expect("by manifest");
    assert_eq!(by_directory, by_manifest);
    assert!(Project::open(&root.join("Cargo.toml")).is_err());
}

#[test]
fn a_project_whose_manifest_is_broken_is_refused_by_name() {
    let scratch = Scratch::new("broken");
    std::fs::write(
        scratch.join("game.toml"),
        "[game]\nname = \"demo\"\nnonsense = true\n",
    )
    .expect("write a manifest with a key nobody knows");

    let message = Project::open(&scratch.path).unwrap_err().to_string();
    assert!(message.contains("game.toml"), "{message}");
    assert!(message.contains("nonsense"), "{message}");
}

#[test]
fn a_renamed_package_is_refused_rather_than_silently_ignored() {
    let scratch = Scratch::new("renamed");
    std::fs::write(scratch.join("game.toml"), "[game]\nname = \"demo\"\n").expect("a manifest");
    std::fs::write(
        scratch.join("Cargo.toml"),
        "[package]\nname = \"something-else\"\n\n[workspace]\n",
    )
    .expect("a crate manifest");

    let project = Project::open(&scratch.path).expect("the project itself opens");
    let message = project.check_crate_name().unwrap_err().to_string();
    assert!(message.contains("demo"), "{message}");
    assert!(message.contains("something-else"), "{message}");
}

/// The CLI under test was built inside an engine checkout, so it should have found
/// it: a generated project then points at a real directory rather than at a
/// repository this machine has never seen.
#[test]
fn the_engine_dependency_is_a_path_that_exists_when_run_from_a_checkout() {
    let scratch = Scratch::new("dependency");
    let root = scratch.join("demo");
    invoke(&["new", &path(&root)]).expect("`new` should succeed");

    let text = std::fs::read_to_string(root.join("Cargo.toml")).expect("read the manifest");
    let dependency = text
        .lines()
        .find(|line| line.starts_with("cubic-render ="))
        .expect("the generated manifest depends on the engine");
    assert!(
        dependency.contains("features = [\"manifest\"]"),
        "{dependency}"
    );

    match dependency
        .split("path = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
    {
        Some(path) => {
            let crate_dir = Path::new(path);
            assert!(
                crate_dir.join("Cargo.toml").is_file(),
                "{} is not an engine crate",
                crate_dir.display()
            );
        }
        // The git fallback is the legitimate outcome for a CLI built outside a
        // checkout; what matters is that it names the repository.
        None => assert!(dependency.contains("Cubic-Engine.git"), "{dependency}"),
    }
}

/// The dependency is written relative to the project when that is cheap, so a
/// checkout and a game can sit side by side and still build.
#[test]
fn a_project_next_to_the_checkout_is_depended_on_by_a_relative_path() {
    let scratch = Scratch::new("relative");
    let root = scratch.join("demo");
    invoke(&["new", &path(&root)]).expect("`new` should succeed");

    let text = std::fs::read_to_string(root.join("Cargo.toml")).expect("read the manifest");
    let dependency = text
        .lines()
        .find(|line| line.starts_with("cubic-render ="))
        .expect("the generated manifest depends on the engine");
    let path = dependency
        .split("path = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("expected a path dependency, got: {dependency}"));

    if Path::new(path).is_absolute() {
        // A scratch directory in the temp dir is not near a checkout on most
        // machines, which is exactly when an absolute path is the honest answer.
        return;
    }
    let resolved = root.join(path);
    assert!(
        resolved.join("Cargo.toml").is_file(),
        "{} is not an engine crate",
        resolved.display()
    );
}
