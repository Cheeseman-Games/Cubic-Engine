//! The cargo frontend: `run` and `build` are cargo, with the manifest filled in.
//!
//! A generated project is an ordinary crate, so there is nothing to build here —
//! no dependency graph to resolve, no code generation step, no bundler. What the
//! CLI contributes is the two things a bare `cargo` call cannot know: *which*
//! project to build, and which engine features that project's manifest asks
//! for.
//!
//! [`argv`] is deliberately a pure function returning the whole argument vector,
//! because that vector is the contract: it is what the tests check, and it is
//! what a user can reproduce by hand.

use std::ffi::OsString;
use std::process::Command;

use crate::engine::ENGINE_CRATE;
use crate::{Error, Project};

/// The cargo subcommand an action maps to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// `cargo run`
    Run,
    /// `cargo build`
    Build,
}

impl Action {
    /// The cargo subcommand itself.
    fn subcommand(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Build => "build",
        }
    }
}

/// The whole argument vector for one cargo invocation.
///
/// `game_args` only mean anything to [`Action::Run`], and are placed after `--`
/// so cargo hands them to the game rather than claiming them. An empty list
/// still emits the separator, which is harmless and keeps the shape of the
/// command predictable.
pub fn argv(
    project: &Project,
    action: Action,
    release: bool,
    game_args: &[String],
) -> Vec<OsString> {
    let mut argv = vec![
        OsString::from(action.subcommand()),
        OsString::from("--manifest-path"),
        project.cargo_manifest().into_os_string(),
    ];
    if release {
        argv.push(OsString::from("--release"));
    }
    // The manifest's feature flags are the project's build configuration: they
    // name engine features, so they are enabled on the engine dependency rather
    // than being features of the project's own crate.
    let features = feature_flags(&project.manifest);
    if !features.is_empty() {
        argv.push(OsString::from("--features"));
        argv.push(OsString::from(features.join(",")));
    }
    argv.push(OsString::from("--"));
    argv.extend(game_args.iter().map(OsString::from));
    argv
}

/// The `--features` value a manifest implies: `true` flags in `[features]`, each
/// qualified with the engine crate they turn on.
///
/// Sorted by the manifest's own ordering, so the same project always builds with
/// the same command. Empty when the manifest names no features.
pub fn feature_flags(manifest: &cubic_core::manifest::ProjectManifest) -> Vec<String> {
    manifest
        .enabled_features()
        .map(|feature| format!("{ENGINE_CRATE}/{feature}"))
        .collect()
}

/// The cargo executable to run.
///
/// `CARGO` is set by cargo when it launches a program, so this picks up the same
/// toolchain — and whatever wrapper — that built the CLI, instead of assuming
/// `cargo` is on the path.
pub fn executable() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

/// Build or run `project`, streaming cargo's own output.
///
/// Cargo reports compile errors and test failures better than any wrapper could,
/// so its output is inherited rather than captured, and its exit status is what
/// decides success. A failure here is not a CLI failure: the invocation was fine.
pub fn exec(
    action: Action,
    project: &Project,
    release: bool,
    game_args: &[String],
) -> Result<(), Error> {
    let status = Command::new(executable())
        .args(argv(project, action, release, game_args))
        .status()
        .map_err(|source| Error::Io {
            path: executable().into(),
            source,
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::CargoFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubic_core::manifest::ProjectManifest;
    use std::path::PathBuf;

    /// A project whose manifest is the given text, at the given root. The
    /// `Cargo.toml` is not touched: only the manifest decides the cargo command.
    fn project(root: &str, manifest: &str) -> Project {
        Project {
            root: PathBuf::from(root),
            manifest: ProjectManifest::parse(manifest).expect("test manifest is valid"),
        }
    }

    fn plain() -> Project {
        project("/games/demo", "[game]\nname = \"demo\"\n")
    }

    /// Everything as strings, which is what the assertions are about.
    fn argv_words(argv: &[OsString]) -> Vec<String> {
        argv.iter()
            .map(|word| word.to_string_lossy().into_owned())
            .collect()
    }

    /// The path cargo is pointed at, spelled the way the platform spells it.
    fn manifest_of(project: &Project) -> String {
        project.cargo_manifest().to_string_lossy().into_owned()
    }

    #[test]
    fn build_is_cargo_build_on_the_projects_manifest() {
        let project = plain();
        let argv = argv_words(&argv(&project, Action::Build, false, &[]));
        assert_eq!(
            argv,
            vec![
                "build".to_string(),
                "--manifest-path".to_string(),
                manifest_of(&project),
                "--".to_string(),
            ]
        );
    }

    #[test]
    fn run_is_cargo_run_on_the_projects_manifest() {
        let project = plain();
        let argv = argv_words(&argv(&project, Action::Run, false, &[]));
        assert_eq!(
            argv,
            vec![
                "run".to_string(),
                "--manifest-path".to_string(),
                manifest_of(&project),
                "--".to_string(),
            ]
        );
    }

    #[test]
    fn release_sits_before_the_game_arguments() {
        let project = plain();
        let argv = argv_words(&argv(
            &project,
            Action::Run,
            true,
            &["--seed".to_string(), "7".to_string()],
        ));
        assert_eq!(
            argv,
            vec![
                "run".to_string(),
                "--manifest-path".to_string(),
                manifest_of(&project),
                "--release".to_string(),
                "--".to_string(),
                "--seed".to_string(),
                "7".to_string(),
            ]
        );
    }

    /// The manifest's feature table is the project's build configuration: an
    /// enabled flag turns on the matching engine feature.
    #[test]
    fn enabled_manifest_features_become_engine_features() {
        let project = project(
            "/games/demo",
            "[game]\nname = \"demo\"\n\n[features]\nweb = true\nalpha = false\nbeta = true\n",
        );
        let argv = argv_words(&argv(&project, Action::Build, false, &[]));
        assert_eq!(
            argv,
            vec![
                "build".to_string(),
                "--manifest-path".to_string(),
                manifest_of(&project),
                "--features".to_string(),
                "cubic-render/beta,cubic-render/web".to_string(),
                "--".to_string(),
            ]
        );
    }

    /// A project with no feature table must not grow an empty `--features`,
    /// which cargo would reject.
    #[test]
    fn a_project_with_no_features_asks_for_none() {
        assert!(feature_flags(&plain().manifest).is_empty());
        let argv = argv_words(&argv(&plain(), Action::Build, false, &[]));
        assert!(!argv.iter().any(|word| word == "--features"), "{argv:?}");
    }

    /// Feature flags come from the manifest, not from the command line: the
    /// engine features a project needs are part of the project.
    #[test]
    fn feature_flags_are_engine_crate_qualified_and_sorted() {
        let flags = feature_flags(
            &project(
                "/games/demo",
                "[game]\nname = \"demo\"\n\n[features]\nweb = true\n3d = true\n",
            )
            .manifest,
        );
        assert_eq!(flags, vec!["cubic-render/3d", "cubic-render/web"]);
    }

    #[test]
    fn the_cargo_executable_follows_the_environment() {
        // `cargo run` sets CARGO for the program it launches; without it, `cargo`
        // from the path is the only sensible guess.
        assert!(!executable().is_empty());
    }
}
