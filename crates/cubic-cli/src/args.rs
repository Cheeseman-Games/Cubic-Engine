//! The command line: what was asked for, parsed from arguments alone.
//!
//! Hand-rolled, because the surface is four commands and a handful of flags, and
//! because a CLI that a game project depends on should not drag an argument
//! parser into every project's build graph. Everything here is pure: it turns
//! strings into a [`Command`] and never touches the filesystem, so the whole
//! grammar is testable without a project on disk.
//!
//! Two conventions, both standard: `--help`/`--version` anywhere win over
//! everything else, and after `--` every remaining argument belongs to the game
//! rather than to the CLI.

use std::ffi::OsString;
use std::path::PathBuf;

/// What the CLI was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Generate a project.
    New(NewArgs),
    /// Build and run a project's game.
    Run(RunArgs),
    /// Build a project without running it.
    Build(BuildArgs),
    /// Print [`USAGE`].
    Help,
    /// Print the CLI's version.
    Version,
}

/// `cubic-cli new <path>`.
#[derive(Clone, Debug, PartialEq)]
pub struct NewArgs {
    /// Where to write the project. Its last component names the crate.
    pub path: PathBuf,
    /// Engine checkout to depend on, instead of the detected one.
    pub engine: Option<PathBuf>,
    /// Write into a directory that already has files in it.
    pub force: bool,
}

/// `cubic-cli run [project]`.
#[derive(Clone, Debug, PartialEq)]
pub struct RunArgs {
    /// Project directory, or its `game.toml`. Defaults to the working directory.
    pub project: Option<PathBuf>,
    /// Build with optimizations.
    pub release: bool,
    /// Arguments after `--`, handed to the game.
    pub game_args: Vec<String>,
}

/// `cubic-cli build [project]`.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildArgs {
    /// Project directory, or its `game.toml`. Defaults to the working directory.
    pub project: Option<PathBuf>,
    /// Build with optimizations.
    pub release: bool,
}

/// The help text, also printed on a usage error.
pub const USAGE: &str = "\
cubic-cli - generate, build and run cubic game projects

Usage:
  cubic-cli new <path> [--engine <path>] [--force]
  cubic-cli run [project] [--release] [-- <game args>...]
  cubic-cli build [project] [--release]
  cubic-cli help | --version

Commands:
  new       Generate a project: game.toml, Cargo.toml, src/, assets/
  run       Build a project and play its game
  build     Build a project without playing it

Arguments:
  project   Project directory or its game.toml (default: the current directory)
  path      Where to generate a project; its last name is the crate's

Options:
  --engine <path>   Engine checkout to depend on, instead of the detected one
  --force           Generate into a directory that already has files in it
  --release         Build with optimizations
  -h, --help        Print this message
  -V, --version     Print the CLI version

A project is a directory holding game.toml. Its manifest says which engine
features to build in, so run and build need nothing else to know how to build
one. Anything after -- goes to the game itself.
";

/// Parse a whole command line — the program name is *not* included.
pub fn parse<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = OsString>,
{
    let words: Vec<String> = args
        .into_iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect();

    // `--help` and `--version` answer for any command, including a half-typed
    // one: someone who cannot remember the flags is exactly who should get them.
    // Past `--` nothing is the CLI's business any more, so a game that has its
    // own `--help` still gets it.
    let own: Vec<&String> = words
        .iter()
        .take_while(|word| word.as_str() != "--")
        .collect();
    if own
        .iter()
        .any(|word| word.as_str() == "--help" || word.as_str() == "-h")
    {
        return Ok(Command::Help);
    }
    if own
        .iter()
        .any(|word| word.as_str() == "--version" || word.as_str() == "-V")
    {
        return Ok(Command::Version);
    }

    let command = match words.first().map(String::as_str) {
        Some("help") => return Ok(Command::Help),
        Some("version") => return Ok(Command::Version),
        Some("new") => Command::New(parse_new(&words[1..])?),
        Some("run") => Command::Run(parse_run(&words[1..])?),
        Some("build") => Command::Build(parse_build(&words[1..])?),
        Some(other) => return Err(format!("unknown command `{other}`")),
        None => return Ok(Command::Help),
    };
    Ok(command)
}

/// `new <path> [--engine <path>] [--force]`
fn parse_new(words: &[String]) -> Result<NewArgs, String> {
    let mut args = NewArgs {
        path: PathBuf::new(),
        engine: None,
        force: false,
    };
    let mut positional: Option<String> = None;
    let mut words = words.iter();

    while let Some(word) = words.next() {
        match word.as_str() {
            "--force" => args.force = true,
            "--engine" => {
                let path = words
                    .next()
                    .ok_or("`--engine` needs a path to an engine checkout")?;
                args.engine = Some(PathBuf::from(path));
            }
            other if other.starts_with('-') => {
                return Err(format!("`new` does not take the option `{other}`"));
            }
            other => {
                if positional.is_some() {
                    return Err(format!(
                        "`new` takes one path, but was also given `{other}`"
                    ));
                }
                positional = Some(other.to_string());
            }
        }
    }

    match positional {
        Some(path) => args.path = PathBuf::from(path),
        None => return Err("`new` needs somewhere to put the project".to_string()),
    }
    Ok(args)
}

/// `run [project] [--release] [-- <game args>...]`
fn parse_run(words: &[String]) -> Result<RunArgs, String> {
    let mut args = RunArgs {
        project: None,
        release: false,
        game_args: Vec::new(),
    };
    let mut positional: Option<String> = None;
    let mut words = words.iter();

    while let Some(word) = words.next() {
        match word.as_str() {
            "--release" => args.release = true,
            // Everything past `--` is the game's own business, flags included.
            "--" => {
                args.game_args.extend(words.map(|word| word.to_string()));
                break;
            }
            other if other.starts_with('-') => {
                return Err(format!(
                    "`run` does not take the option `{other}`; arguments for the game go after `--`"
                ));
            }
            other => {
                if positional.is_some() {
                    return Err(format!(
                        "`run` takes one project, but was also given `{other}`; \
                         arguments for the game go after `--`"
                    ));
                }
                positional = Some(other.to_string());
            }
        }
    }

    args.project = positional.map(PathBuf::from);
    Ok(args)
}

/// `build [project] [--release]`
fn parse_build(words: &[String]) -> Result<BuildArgs, String> {
    let mut args = BuildArgs {
        project: None,
        release: false,
    };
    let mut positional: Option<String> = None;

    for word in words {
        match word.as_str() {
            "--release" => args.release = true,
            other if other.starts_with('-') => {
                return Err(format!("`build` does not take the option `{other}`"));
            }
            other => {
                if positional.is_some() {
                    return Err(format!(
                        "`build` takes one project, but was also given `{other}`"
                    ));
                }
                positional = Some(other.to_string());
            }
        }
    }

    args.project = positional.map(PathBuf::from);
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a command line written the way a person would type it.
    fn parse_args(line: &str) -> Result<Command, String> {
        parse(line.split_whitespace().map(OsString::from))
    }

    fn path(value: &str) -> PathBuf {
        PathBuf::from(value)
    }

    #[test]
    fn a_bare_invocation_prints_the_usage() {
        assert_eq!(parse_args(""), Ok(Command::Help));
    }

    /// Help and version answer from anywhere before `--`, but never past it.
    #[test]
    fn help_and_version_answer_from_anywhere() {
        for line in [
            "help",
            "--help",
            "-h",
            "new demo --help",
            "run -h",
            "--help run demo",
        ] {
            assert_eq!(parse_args(line), Ok(Command::Help), "{line}");
        }
        for line in ["version", "--version", "-V", "new demo --version"] {
            assert_eq!(parse_args(line), Ok(Command::Version), "{line}");
        }
    }

    /// A game's own `--help` belongs to the game: `cubic-cli run demo -- --help`
    /// must not swallow it and print the CLI's usage instead.
    #[test]
    fn help_after_a_double_dash_belongs_to_the_game() {
        assert_eq!(
            parse_args("run demo -- --help"),
            Ok(Command::Run(RunArgs {
                project: Some(path("demo")),
                release: false,
                game_args: vec!["--help".to_string()],
            }))
        );
    }

    /// The flags come in the order a person tends to type them, and none of them
    /// are positional.
    #[test]
    fn new_takes_a_path_and_its_options_in_any_order() {
        assert_eq!(
            parse_args("new demo"),
            Ok(Command::New(NewArgs {
                path: path("demo"),
                engine: None,
                force: false,
            }))
        );
        assert_eq!(
            parse_args("new --force --engine ../Cubic-Engine my-game"),
            Ok(Command::New(NewArgs {
                path: path("my-game"),
                engine: Some(path("../Cubic-Engine")),
                force: true,
            }))
        );
        assert_eq!(
            parse_args("new --engine ../Cubic-Engine my-game --force"),
            Ok(Command::New(NewArgs {
                path: path("my-game"),
                engine: Some(path("../Cubic-Engine")),
                force: true,
            }))
        );
        // A path is a path: the crate is named after its last component, so a
        // project can be generated straight into a directory of games.
        assert_eq!(
            parse_args("new ../games/my-game"),
            Ok(Command::New(NewArgs {
                path: path("../games/my-game"),
                engine: None,
                force: false,
            }))
        );
    }

    #[test]
    fn new_needs_exactly_one_path() {
        assert!(parse_args("new").is_err());
        assert!(parse_args("new --force").is_err());
        assert!(parse_args("new one two").is_err());
        assert!(parse_args("new --engine").is_err());
        assert!(parse_args("new --wat demo").is_err());
    }

    #[test]
    fn run_defaults_to_the_working_directory() {
        assert_eq!(
            parse_args("run"),
            Ok(Command::Run(RunArgs {
                project: None,
                release: false,
                game_args: Vec::new(),
            }))
        );
    }

    #[test]
    fn run_takes_a_project_and_a_release_flag() {
        assert_eq!(
            parse_args("run demo"),
            Ok(Command::Run(RunArgs {
                project: Some(path("demo")),
                release: false,
                game_args: Vec::new(),
            }))
        );
        assert_eq!(
            parse_args("run --release demo"),
            Ok(Command::Run(RunArgs {
                project: Some(path("demo")),
                release: true,
                game_args: Vec::new(),
            }))
        );
        assert_eq!(
            parse_args("run ./some/where/game.toml --release"),
            Ok(Command::Run(RunArgs {
                project: Some(path("./some/where/game.toml")),
                release: true,
                game_args: Vec::new(),
            }))
        );
    }

    /// Everything after `--` goes to the game, flags and all.
    #[test]
    fn everything_after_a_double_dash_goes_to_the_game() {
        assert_eq!(
            parse_args("run demo --release -- --ticks 100 --seed 7"),
            Ok(Command::Run(RunArgs {
                project: Some(path("demo")),
                release: true,
                game_args: vec![
                    "--ticks".to_string(),
                    "100".to_string(),
                    "--seed".to_string(),
                    "7".to_string(),
                ],
            }))
        );
        assert_eq!(
            parse_args("run -- --help"),
            Ok(Command::Run(RunArgs {
                project: None,
                release: false,
                game_args: vec!["--help".to_string()],
            }))
        );
    }

    /// A second positional is nearly always a game argument someone forgot to
    /// fence off, so the error says so instead of silently eating it.
    #[test]
    fn a_second_positional_is_refused_with_a_hint() {
        let error = parse_args("run demo --ticks 100").unwrap_err();
        assert!(error.contains("`--ticks`"), "{error}");
        assert!(error.contains("after `--`"), "{error}");
    }

    #[test]
    fn build_mirrors_run_without_game_arguments() {
        assert_eq!(
            parse_args("build"),
            Ok(Command::Build(BuildArgs {
                project: None,
                release: false,
            }))
        );
        assert_eq!(
            parse_args("build --release demo"),
            Ok(Command::Build(BuildArgs {
                project: Some(path("demo")),
                release: true,
            }))
        );
        assert!(parse_args("build --wat").is_err());
        assert!(parse_args("build one two").is_err());
        assert!(parse_args("build demo -- --release").is_err());
    }

    #[test]
    fn an_unknown_command_is_named_in_the_error() {
        let error = parse_args("frobnicate demo").unwrap_err();
        assert!(error.contains("frobnicate"), "{error}");
    }

    /// The usage text has to stay honest about what is accepted, or it is worse
    /// than no help at all.
    #[test]
    fn the_usage_text_documents_every_command_and_option() {
        for expected in [
            "cubic-cli new",
            "cubic-cli run",
            "cubic-cli build",
            "--engine",
            "--force",
            "--release",
            "--help",
            "--version",
            "game.toml",
        ] {
            assert!(
                USAGE.contains(expected),
                "usage does not mention {expected}"
            );
        }
    }
}
