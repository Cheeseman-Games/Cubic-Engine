//! `cubic-cli`: generate, build and run cubic game projects.
//!
//! The engine is a library; this is the thin layer in front of it that knows
//! what a *project* is — a directory with a `game.toml` at its root — and hands
//! the work to cargo. There is no build system here and no template engine: a
//! project is a crate, so `run` and `build` are `cargo run` and `cargo build`
//! with the manifest's feature flags and engine dependency filled in.
//!
//! ```text
//! cubic-cli new demo        # generate a project
//! cubic-cli run demo        # build and play it
//! cubic-cli build demo      # build it without playing
//! ```
//!
//! ```
//! use cubic_cli::args::parse;
//!
//! let args = |words: &[&str]| words.iter().map(Into::into).collect::<Vec<_>>();
//!
//! assert!(parse(args(&["new", "demo"])).is_ok());
//! assert!(parse(args(&["run", "demo", "--", "--seed", "7"])).is_ok());
//! assert!(parse(args(&["run", "demo", "--nonsense"])).is_err());
//! ```
//!
//! The manifest is the contract. Everything the runtime needs to open a window
//! and tick a simulation is in `game.toml` (`cubic_core::manifest`), which means
//! the CLI can find a project, read what it wants built into it, and hand it to
//! the engine — and the editor later opens the very same file.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use cubic_core::manifest::{ManifestError, ProjectManifest};

pub mod args;
pub mod cargo;
pub mod engine;
pub mod project;
pub mod scaffold;

pub use args::{BuildArgs, Command, NewArgs, RunArgs, USAGE};
pub use engine::EngineSource;
pub use project::Project;

/// Run one CLI invocation over `args` — the program name is *not* included, as
/// `std::env::args_os().skip(1)` hands it over.
///
/// Printing is part of the job here, so a successful run may have written to
/// stdout (`--help`, `--version`) before returning `Ok`. Every failure comes back
/// as an [`Error`] with the reason already phrased for a person to read;
/// [`Error::exit_code`] is what the binary should exit with.
pub fn run<I>(args: I) -> Result<(), Error>
where
    I: IntoIterator<Item = OsString>,
{
    match args::parse(args).map_err(Error::Usage)? {
        Command::Help => {
            println!("{}", USAGE.trim_end());
            Ok(())
        }
        Command::Version => {
            println!("cubic-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::New(new) => scaffold::run(new),
        Command::Run(run) => {
            let project = Project::open(run.project.as_deref().unwrap_or(Path::new(".")))?;
            project.check_crate_name()?;
            cargo::exec(cargo::Action::Run, &project, run.release, &run.game_args)
        }
        Command::Build(build) => {
            let project = Project::open(build.project.as_deref().unwrap_or(Path::new(".")))?;
            project.check_crate_name()?;
            cargo::exec(cargo::Action::Build, &project, build.release, &[])
        }
    }
}

/// Why a CLI invocation could not be carried out.
#[derive(Debug)]
pub enum Error {
    /// The arguments did not describe a command. Exit code 2, by the usual
    /// convention: the shell script that called us did something wrong.
    Usage(String),
    /// Something about the project on disk. The message says what to do about
    /// it.
    Project(String),
    /// A file could not be read or written.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A `game.toml` was found but is not usable.
    Manifest { path: PathBuf, error: ManifestError },
    /// Cargo ran and failed; its own diagnostics are already on the terminal.
    CargoFailed,
}

impl Error {
    /// The process exit code this failure deserves.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            _ => 1,
        }
    }

    /// A failed read or write of `path`.
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Read `path` into a string, reporting the path if that is what went wrong.
    fn read(path: &Path) -> Result<String, Self> {
        std::fs::read_to_string(path).map_err(|source| Self::io(path, source))
    }

    /// Write `contents` to `path`, creating the directories above it.
    pub fn write(path: &Path, contents: &str) -> Result<(), Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Self::io(parent, source))?;
        }
        std::fs::write(path, contents).map_err(|source| Self::io(path, source))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}\n\n{USAGE}"),
            Self::Project(message) => write!(f, "{message}"),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Manifest { path, error } => write!(f, "{}: {error}", path.display()),
            Self::CargoFailed => write!(f, "cargo failed"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Manifest { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// Load and parse a `game.toml`, reporting the path in any failure.
///
/// Shared by project discovery and the generator, which both have a manifest
/// path in hand and want the same "which file, which line" diagnosis.
pub(crate) fn parse_manifest(path: &Path) -> Result<ProjectManifest, Error> {
    let text = Error::read(path)?;
    ProjectManifest::parse(&text).map_err(|error| Error::Manifest {
        path: path.to_path_buf(),
        error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A usage failure is a different class of mistake from a broken project,
    /// and a script wrapping the CLI needs to tell them apart.
    #[test]
    fn a_usage_failure_exits_two_and_everything_else_exits_one() {
        assert_eq!(Error::Usage("nope".to_string()).exit_code(), 2);
        assert_eq!(Error::Project("nope".to_string()).exit_code(), 1);
        assert_eq!(
            Error::CargoFailed.exit_code(),
            1,
            "a failed build is not a bad invocation"
        );
        assert_eq!(
            Error::Io {
                path: PathBuf::from("x"),
                source: std::io::Error::other("boom"),
            }
            .exit_code(),
            1
        );
    }

    #[test]
    fn a_usage_error_prints_the_reason_and_the_usage() {
        let text = Error::Usage("unknown command `frobnicate`".to_string()).to_string();
        assert!(text.starts_with("unknown command `frobnicate`"));
        assert!(text.contains("cubic-cli run"), "{text}");
    }

    #[test]
    fn a_file_failure_names_the_file() {
        let error = Error::io("game.toml", std::io::Error::other("not found"));
        assert_eq!(error.to_string(), "game.toml: not found");
    }

    /// A manifest failure names the file *and* the reason, so an author editing
    /// `game.toml` knows which file and which line to look at.
    #[test]
    fn a_manifest_failure_names_the_file_and_the_reason() {
        // A crate manifest is valid TOML and an invalid game manifest, which is
        // exactly the mix-up worth reporting well.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let error = parse_manifest(&path).unwrap_err();
        let text = error.to_string();
        assert!(
            text.starts_with(&format!("{}: game.toml is not valid", path.display())),
            "{text}"
        );
    }

    #[test]
    fn a_missing_manifest_names_the_file_it_could_not_read() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-manifest.toml");
        let text = parse_manifest(&path).unwrap_err().to_string();
        assert!(text.contains(&path.display().to_string()), "{text}");
    }
}
