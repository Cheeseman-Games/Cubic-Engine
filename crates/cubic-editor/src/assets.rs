//! The editor's view of a project's asset files.
//!
//! The inspector's asset pickers need to offer the files a field can point at,
//! and they need to name them the way the engine's [`AssetServer`] does — by a
//! path relative to the project root. This module walks a project's `assets`
//! directory once, when the project opens, and hands the pickers that list. It
//! never loads or decodes anything: choosing an asset is what does the load, so
//! a project with a thousand sprites costs one directory walk, not a thousand
//! decodes.
//!
//! [`AssetServer`]: cubic_core::assets::AssetServer

use std::path::Path;

use cubic_core::assets::AssetKind;

/// The directory a project keeps its assets in, relative to its root.
const ASSET_DIR: &str = "assets";

/// One asset file a project holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetEntry {
    /// The path an [`AssetServer`](cubic_core::assets::AssetServer) loads it
    /// by: relative to the project root, `/`-separated, so `assets/hero.png`.
    pub path: String,
    /// What the extension implied.
    pub kind: AssetKind,
}

/// The asset files under a project root, sorted by path.
///
/// Built by [`scan`](Self::scan) when a project opens; the inspector reads it
/// for every asset field it draws.
#[derive(Default)]
pub struct AssetCatalog {
    entries: Vec<AssetEntry>,
}

impl AssetCatalog {
    /// Scans `<root>/assets`, recursively.
    ///
    /// A project with no `assets` directory yields an empty catalog rather than
    /// an error: the pickers then simply offer no options, which is the truth
    /// of a project that holds no assets.
    pub fn scan(root: &Path) -> Self {
        let mut entries = Vec::new();
        collect(&root.join(ASSET_DIR), ASSET_DIR, &mut entries);
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Self { entries }
    }

    /// Every asset, sorted by path.
    pub fn entries(&self) -> &[AssetEntry] {
        &self.entries
    }

    /// Whether any assets were found.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The assets a field of `kind` can point at.
    ///
    /// A raw [`AssetKind::Bytes`] field holds an untyped handle and may name any
    /// asset, so it sees them all; a texture or font field sees only its own
    /// family, because a picker offering a `.txt` for a texture is noise.
    pub fn matching(&self, kind: AssetKind) -> impl Iterator<Item = &AssetEntry> {
        self.entries
            .iter()
            .filter(move |entry| kind == AssetKind::Bytes || entry.kind == kind)
    }
}

/// Walks `directory`, appending every file under it to `entries`.
///
/// `prefix` is the project-relative path of `directory`, so each entry is
/// spelled the way an `AssetServer` keys it. A directory that cannot be read is
/// skipped rather than fatal: a picker missing an option beats a project that
/// will not open.
fn collect(directory: &Path, prefix: &str, entries: &mut Vec<AssetEntry>) {
    let Ok(read) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let relative = format!("{prefix}/{name}");
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => collect(&path, &relative, entries),
            Ok(_) => entries.push(AssetEntry {
                kind: AssetKind::of_path(&relative),
                path: relative,
            }),
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::scratch::Scratch;

    #[test]
    fn scan_lists_asset_files_by_kind_sorted() {
        let root = Scratch::new("asset-scan");
        fs::create_dir_all(root.join("assets/sub")).expect("dirs");
        fs::write(root.join("assets/hero.png"), b"x").expect("png");
        fs::write(root.join("assets/sub/notes.txt"), b"y").expect("txt");
        fs::write(root.join("assets/font.ttf"), b"z").expect("ttf");

        let catalog = AssetCatalog::scan(&root.path);
        let paths: Vec<&str> = catalog.entries().iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            ["assets/font.ttf", "assets/hero.png", "assets/sub/notes.txt"],
            "entries are project-relative and sorted"
        );

        assert_eq!(catalog.matching(AssetKind::Texture).count(), 1);
        assert_eq!(catalog.matching(AssetKind::Font).count(), 1);
        assert_eq!(
            catalog.matching(AssetKind::Bytes).count(),
            3,
            "a raw handle may name any asset"
        );
    }

    #[test]
    fn a_project_without_assets_scans_empty() {
        let root = Scratch::new("asset-none");
        assert!(AssetCatalog::scan(&root.path).is_empty());
    }
}
