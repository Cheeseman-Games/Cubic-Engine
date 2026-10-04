//! Where a generated project's engine dependency points.
//!
//! A project depends on `cubic-render`, and there are two honest ways to write
//! that: a path into a local engine checkout, which is what working on the engine
//! and its games together wants, or a git dependency, which is what a project
//! that outlives the checkout wants. A developer running `cubic-cli` out of an
//! engine checkout almost always wants the first, so that is what is detected.
//!
//! Detection order, first hit wins:
//!
//! 1. `--engine <path>` on the command line — the explicit answer.
//! 2. `CUBIC_ENGINE_PATH` in the environment — for a shell that always wants a
//!    particular checkout.
//! 3. The engine checkout this CLI was built from — the everyday case, and the
//!    one that needs no configuration at all.
//! 4. The published repository, as a git dependency.
//!
//! A generated path is relative when it can be short and clearly so (see
//! [`relative_path`]), which keeps a generated `Cargo.toml` working when the
//! whole tree moves. Everything else gets an absolute path or a git URL.

use std::path::{Component, Path, PathBuf};

use crate::Error;

/// The repository a generated project falls back to depending on.
pub const ENGINE_REPOSITORY: &str = "https://github.com/Cheeseman-Games/Cubic-Engine.git";

/// The crate a project depends on. Its prelude re-exports `cubic-core`, so this
/// is the only engine dependency a game needs.
pub const ENGINE_CRATE: &str = "cubic-render";

/// The version a git dependency is resolved against, matching the workspace.
pub const ENGINE_VERSION: &str = "0.1";

/// Environment variable naming an engine checkout, as `--engine` would.
pub const ENGINE_PATH_ENV: &str = "CUBIC_ENGINE_PATH";

/// Longest chain of `..` a generated relative path may use.
///
/// A relative dependency is a convenience, not a requirement: past a few hops
/// the path stops being readable and starts looking like a mistake, so the
/// generator writes an absolute one instead.
const MAX_HOPS: usize = 3;

/// The engine a generated project should depend on.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineSource {
    /// A local checkout, at this crate's directory.
    Checkout { crate_dir: PathBuf },
    /// The published repository.
    Git,
}

impl EngineSource {
    /// Pick a source, honouring an explicit `--engine` path first.
    ///
    /// An explicit path that does not contain the engine crate is an error: a
    /// project generated against the wrong path would not build, and failing
    /// here says why immediately instead of at the project's first build.
    pub fn resolve(explicit: Option<&Path>) -> Result<Self, Error> {
        if let Some(path) = explicit {
            return Ok(Self::Checkout {
                crate_dir: engine_crate_dir(path)?,
            });
        }
        if let Some(path) = std::env::var_os(ENGINE_PATH_ENV).map(PathBuf::from) {
            return Ok(Self::Checkout {
                crate_dir: engine_crate_dir(&path)?,
            });
        }
        Ok(Self::default())
    }

    /// The engine's own crate directory, if this is a checkout.
    fn checkout_dir(&self) -> Option<&Path> {
        match self {
            Self::Checkout { crate_dir } => Some(crate_dir),
            Self::Git => None,
        }
    }

    /// The `[dependencies]` entry for the engine, without the crate name.
    ///
    /// The pieces are what the generator formats into a `Cargo.toml` line, kept
    /// separate so the TOML escaping has one place to live.
    pub fn dependency(&self, project_dir: &Path) -> String {
        match self.checkout_dir() {
            Some(crate_dir) => {
                let path = relative_path(project_dir, crate_dir)
                    .unwrap_or_else(|| crate_dir.to_path_buf());
                format!("path = {}", toml_string(&path.to_string_lossy()))
            }
            None => format!(
                "git = {}, version = \"{ENGINE_VERSION}\"",
                toml_string(ENGINE_REPOSITORY)
            ),
        }
    }

    /// The checkout this CLI was built from, when there is one.
    ///
    /// `cargo` bakes the manifest directory in, so a CLI built inside an engine
    /// checkout always knows where that checkout is — a fact that is false the
    /// moment the binary is copied somewhere else, which is why the other
    /// detection routes exist.
    fn built_from_checkout() -> Option<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
        let crate_dir = root.join("crates").join(ENGINE_CRATE);
        // Canonicalized like the explicit route, so both name the same path.
        let crate_dir = canonical(&crate_dir).ok()?;
        crate_dir
            .join("Cargo.toml")
            .is_file()
            .then_some(Self::Checkout { crate_dir })
    }
}

impl Default for EngineSource {
    /// The engine checkout this CLI came from, or the repository if it did not
    /// come from one.
    fn default() -> Self {
        Self::built_from_checkout().unwrap_or(Self::Git)
    }
}

/// The engine crate inside `root`, canonicalized so it can be written into
/// another project's `Cargo.toml`.
fn engine_crate_dir(root: &Path) -> Result<PathBuf, Error> {
    // A checkout root and the crate inside it are both plausible answers, so
    // accept whichever one actually holds the crate.
    let crate_dir = root.join("crates").join(ENGINE_CRATE);
    let dir = if crate_dir.join("Cargo.toml").is_file() {
        crate_dir
    } else if root.join("Cargo.toml").is_file() {
        root.to_path_buf()
    } else {
        return Err(Error::Project(format!(
            "no engine crate at {} (expected {} or its checkout root)",
            root.display(),
            crate_dir.join("Cargo.toml").display()
        )));
    };
    // Canonicalized because the path ends up in a file the project will be built
    // from later, possibly from a directory that does not exist yet.
    canonical(&dir)
}

/// Canonicalize `path`, in the spelling a person would have written.
///
/// Windows canonicalization answers in the verbatim `\\?\` form. It is a valid
/// path and every Windows API accepts it, but it is a strange thing to find
/// written in a generated `Cargo.toml` that someone will read, so the prefix is
/// dropped. A UNC path keeps its form: stripping `\\?\` from `\\?\UNC\server`
/// would turn a valid path into an invalid one.
fn canonical(path: &Path) -> Result<PathBuf, Error> {
    let canonical = path.canonicalize().map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(match canonical.to_string_lossy().strip_prefix(r"\\?\") {
        Some(stripped) if !stripped.starts_with(r"UNC\") => PathBuf::from(stripped),
        _ => canonical,
    })
}

/// Write `to` as a path relative to `from_dir`, when that is worth doing.
///
/// `None` means "use the absolute path instead", which is the right answer when
/// the two paths share no root (different Windows drives), when either contains
/// a `.`/`..` component that has not been resolved yet, when the chain of `..`
/// would be longer than [`MAX_HOPS`], or when the two are the same directory.
pub fn relative_path(from_dir: &Path, to: &Path) -> Option<PathBuf> {
    let from: Vec<Component<'_>> = from_dir.components().collect();
    let to: Vec<Component<'_>> = to.components().collect();

    // Both absolute or both relative: a root component that differs (or exists on
    // one side only) means there is no relative path between them.
    match (from.first(), to.first()) {
        (Some(a), Some(b)) if a != b => return None,
        (None, Some(_)) | (Some(_), None) => return None,
        _ => {}
    }
    // A `.` or `..` that has not been resolved would make the hop count a lie.
    if from.iter().chain(&to).any(|component| {
        !matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::Normal(_)
        )
    }) {
        return None;
    }

    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let hops = from.len() - shared;
    if hops > MAX_HOPS {
        return None;
    }

    let mut relative = PathBuf::new();
    for _ in 0..hops {
        relative.push("..");
    }
    relative.extend(&to[shared..]);
    (!relative.as_os_str().is_empty()).then_some(relative)
}

/// A TOML basic string for `value`: quotes, backslashes and control characters
/// escaped, so a Windows path survives being written into a `Cargo.toml`.
pub fn toml_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path built from segments, so expectations read the same on every
    /// platform — `PathBuf` knows that `/` and `\` are both separators on
    /// Windows and neither is on unix.
    fn path(segments: &[&str]) -> PathBuf {
        segments.iter().collect()
    }

    /// The engine checkout this CLI was built from, in the spelling the generator
    /// would write.
    fn this_checkout() -> PathBuf {
        canonical(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
            .expect("this crate lives in a checkout")
    }

    #[test]
    fn a_sibling_directory_is_one_hop_up() {
        let from = path(&["/", "src", "Cheeseman"]);
        let to = path(&[
            "/",
            "src",
            "Cheeseman",
            "Cubic-Engine",
            "crates",
            "cubic-render",
        ]);
        assert_eq!(
            relative_path(&from, &to),
            Some(path(&["Cubic-Engine", "crates", "cubic-render"]))
        );
    }

    #[test]
    fn the_nearest_common_ancestor_is_the_anchor() {
        let from = path(&["/", "src", "Cheeseman", "games"]);
        let to = path(&[
            "/",
            "src",
            "Cheeseman",
            "Cubic-Engine",
            "crates",
            "cubic-render",
        ]);
        assert_eq!(
            relative_path(&from, &to),
            Some(path(&["..", "Cubic-Engine", "crates", "cubic-render"]))
        );
    }

    /// Past a few hops the path is unreadable, so the caller keeps the absolute
    /// one instead.
    #[test]
    fn a_long_chain_of_hops_is_declined() {
        let from = path(&["/", "a", "b", "c", "d", "e"]);
        let to = path(&["/", "a", "z"]);
        assert_eq!(relative_path(&from, &to), None);

        let from = path(&["/", "a", "b", "c"]);
        let to = path(&["/", "a", "z"]);
        assert_eq!(relative_path(&from, &to), Some(path(&["..", "..", "z"])));
    }

    /// A relative path between two absolute paths has to name one of the roots,
    /// which makes it absolute again — so mixing the two forms is refused.
    #[test]
    fn paths_of_different_kinds_are_declined() {
        assert_eq!(relative_path(&path(&["/", "a", "b"]), &path(&["y"])), None);
        assert_eq!(
            relative_path(&path(&["a", "b"]), &path(&["/", "a", "b"])),
            None
        );
    }

    /// An unresolved `..` would make the hop count a guess.
    #[test]
    fn unresolved_components_are_declined() {
        assert_eq!(
            relative_path(&path(&["/", "a", "b", "..", "b"]), &path(&["/", "a", "c"])),
            None
        );
        assert_eq!(
            relative_path(&path(&["/", "a", "b"]), &path(&["/", "a", "b", "..", "c"])),
            None
        );
    }

    #[test]
    fn the_same_directory_has_no_relative_form() {
        assert_eq!(
            relative_path(&path(&["/", "a", "b"]), &path(&["/", "a", "b"])),
            None
        );
    }

    /// Windows drive letters are prefixes, and two of them have no relative path
    /// between them.
    #[test]
    #[cfg(windows)]
    fn windows_drive_letters_have_to_agree() {
        assert_eq!(
            relative_path(
                Path::new(r"C:\src\Cheeseman"),
                Path::new(r"C:\src\Cheeseman\Cubic-Engine\crates\cubic-render"),
            ),
            Some(PathBuf::from(r"Cubic-Engine\crates\cubic-render"))
        );
        assert_eq!(
            relative_path(Path::new(r"C:\games"), Path::new(r"D:\engine\cubic-render")),
            None
        );
    }

    /// Windows paths written into `Cargo.toml` need their separators escaped, or
    /// the generated file does not parse.
    #[test]
    fn windows_paths_are_escaped_for_toml() {
        assert_eq!(
            toml_string(r"C:\src\cubic-render"),
            r#""C:\\src\\cubic-render""#
        );
        assert_eq!(toml_string("say \"hi\""), r#""say \"hi\"""#);
        assert_eq!(toml_string("plain/path"), "\"plain/path\"");
    }

    /// Canonical paths are written the way a person would write them: on Windows
    /// the verbatim `\\?\` form is a valid path but an odd thing to find in a
    /// generated manifest.
    #[test]
    fn canonical_paths_lose_their_verbatim_prefix() {
        let canonicalized = canonical(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let text = canonicalized.to_string_lossy().into_owned();
        assert!(!text.starts_with(r"\\?\"), "{text}");
        assert!(canonicalized.join("Cargo.toml").is_file(), "{text}");
    }

    /// The everyday case: a CLI built inside an engine checkout finds it without
    /// being told, and the dependency it generates points at a real crate.
    #[test]
    fn a_cli_built_from_a_checkout_finds_it() {
        let crate_dir = this_checkout().join("crates").join(ENGINE_CRATE);
        assert!(
            crate_dir.join("Cargo.toml").is_file(),
            "{} is not a crate",
            crate_dir.display()
        );

        assert_eq!(
            EngineSource::default(),
            EngineSource::Checkout {
                crate_dir: crate_dir.clone()
            },
            "the CLI should have found the checkout it was built from"
        );
        assert!(
            EngineSource::default()
                .dependency(Path::new("/games/elsewhere"))
                .starts_with("path = ")
        );
    }

    #[test]
    fn an_explicit_engine_path_is_accepted() {
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        assert_eq!(
            EngineSource::resolve(Some(&checkout)).unwrap(),
            EngineSource::Checkout {
                crate_dir: this_checkout().join("crates").join(ENGINE_CRATE),
            }
        );
    }

    /// A path that is not an engine checkout fails here rather than producing a
    /// project that cannot build.
    #[test]
    fn a_path_without_an_engine_crate_is_refused() {
        let error = EngineSource::resolve(Some(Path::new("/tmp"))).unwrap_err();
        assert!(error.to_string().contains("no engine crate at"), "{error}");
    }

    #[test]
    fn the_git_fallback_names_the_repository() {
        let dependency = EngineSource::Git.dependency(&path(&["/", "somewhere"]));
        assert_eq!(
            dependency,
            r#"git = "https://github.com/Cheeseman-Games/Cubic-Engine.git", version = "0.1""#
        );
    }

    /// A project far enough from the checkout that no readable relative path exists
    /// still gets a working one: the absolute path.
    #[test]
    fn an_out_of_the_way_checkout_is_written_as_an_absolute_path() {
        let crate_dir = path(&["/", "z", "cubic-render"]);
        let source = EngineSource::Checkout {
            crate_dir: crate_dir.clone(),
        };
        assert_eq!(
            source.dependency(&path(&["/", "a", "b", "c", "d", "e"])),
            format!("path = {}", toml_string(&crate_dir.to_string_lossy()))
        );
    }
}
