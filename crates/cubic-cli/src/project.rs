//! Finding a project: a directory with a `game.toml` at its root.
//!
//! `game.toml` is the whole discovery rule, and it is the same rule the editor
//! will use later — one file says "this is a cubic project, and here is what it
//! is". Everything else about the project (`Cargo.toml`, `src/`, `assets/`) is
//! found relative to it, so a project can be moved or renamed without anything
//! but its own manifest pointing anywhere.
//!
//! A second check belongs here too: the crate name in `Cargo.toml` must still
//! agree with `game.name`. Nothing makes cargo complain when they drift, and a
//! project whose two names disagree is confusing to build, to read and to ship.

use std::path::{Path, PathBuf};

use cubic_core::manifest::ProjectManifest;

use crate::{Error, parse_manifest};

/// A project on disk: its root directory and its manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    /// The directory holding `game.toml`.
    pub root: PathBuf,
    /// The parsed manifest.
    pub manifest: ProjectManifest,
}

impl Project {
    /// Open the project at `path`, which may be the directory or its
    /// `game.toml`.
    ///
    /// The manifest is the only required file here: the crate check happens in
    /// [`Project::check_crate_name`], so a project that is being written can be
    /// inspected before its `Cargo.toml` exists.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let (root, manifest_path) = root_and_manifest(path)?;
        let manifest = parse_manifest(&manifest_path)?;
        Ok(Self { root, manifest })
    }

    /// The project's `Cargo.toml`, next to its manifest by definition.
    pub fn cargo_manifest(&self) -> PathBuf {
        self.root.join("Cargo.toml")
    }

    /// Refuse a project whose crate name has drifted from its manifest name.
    ///
    /// The manifest's name is what the project is called — it is the title a
    /// window opens under and the identity the editor shows — so a `Cargo.toml`
    /// that disagrees is a mistake worth reporting rather than a difference to
    /// quietly accept.
    pub fn check_crate_name(&self) -> Result<(), Error> {
        let path = self.cargo_manifest();
        if !path.is_file() {
            return Err(Error::Project(format!(
                "{} is not a project: it has no Cargo.toml",
                self.root.display()
            )));
        }
        let Some(crate_name) = crate_name_of(&Error::read(&path)?) else {
            return Ok(());
        };
        if crate_name == self.manifest.game.name {
            return Ok(());
        }
        Err(Error::Project(format!(
            "{}: the crate is named `{crate_name}` but game.toml calls the project `{}`; \
             make them the same name",
            path.display(),
            self.manifest.game.name
        )))
    }
}

/// The directory holding a manifest, and the manifest's path.
///
/// A path may be the project directory or the manifest inside it, because both
/// read naturally on a command line and there is no reason to make the caller
/// care which it has.
fn root_and_manifest(path: &Path) -> Result<(PathBuf, PathBuf), Error> {
    if path.is_file() {
        if path.file_name().is_some_and(|name| name == "game.toml") {
            let root = path
                .parent()
                .ok_or_else(|| {
                    Error::Project(format!("{} has no parent directory", path.display()))
                })?
                .to_path_buf();
            return Ok((root, path.to_path_buf()));
        }
        return Err(Error::Project(format!(
            "{} is not a project manifest; pass the project directory or its game.toml",
            path.display()
        )));
    }

    let root = path.to_path_buf();
    let manifest = root.join("game.toml");
    if manifest.is_file() {
        return Ok((root, manifest));
    }
    if !root.is_dir() {
        return Err(Error::Project(format!("{} does not exist", root.display())));
    }
    Err(Error::Project(format!(
        "{} is not a cubic project (no game.toml); generate one with `cubic-cli new <name>`",
        root.display()
    )))
}

/// The `package.name` out of a `Cargo.toml`, if it declares one.
///
/// Read as a table rather than through a typed schema: a crate manifest is full
/// of sections this CLI has no opinion about, and only the one key matters.
fn crate_name_of(manifest: &str) -> Option<String> {
    toml::from_str::<toml::Table>(manifest)
        .ok()?
        .get("package")?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_name_is_read_out_of_a_cargo_manifest() {
        assert_eq!(
            crate_name_of(
                r#"
[package]
name = "demo"
version = "0.1.0"
edition = "2024"
"#
            )
            .as_deref(),
            Some("demo")
        );
    }

    /// A workspace member inherits nothing, and a manifest with no package table
    /// is not a crate — either way there is no name to compare against.
    #[test]
    fn a_manifest_without_a_package_name_has_none_to_compare() {
        assert_eq!(crate_name_of("[workspace]\nmembers = [\"a\"]\n"), None);
        assert_eq!(crate_name_of("not toml at all ]["), None);
        assert_eq!(crate_name_of(""), None);
    }

    /// A workspace-inherited name still resolves for cargo, so refusing to check
    /// it would be a hole rather than a mercy.
    #[test]
    fn a_non_string_package_name_is_not_treated_as_a_mismatch() {
        assert_eq!(crate_name_of("[package]\nname = 42\n"), None);
    }
}
