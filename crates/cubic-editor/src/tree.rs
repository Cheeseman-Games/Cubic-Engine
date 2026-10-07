//! Project tree state and the file operations behind it.
//!
//! The panel renders one row per entry every frame; this module owns what
//! outlives a frame (which folders are open, which row is being renamed) and
//! the filesystem moves those rows perform. Every operation validates its name
//! and reports why it refused in a sentence meant for the console.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A file or folder as listed by [`entries`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    /// Absolute path on disk.
    pub path: PathBuf,
    /// File name, as displayed.
    pub name: String,
    /// Whether it is a directory (and therefore expandable).
    pub is_dir: bool,
}

/// An in-progress rename: the path being renamed and the edited name.
#[derive(Clone, Debug)]
pub struct Rename {
    /// The path whose name is being edited.
    pub path: PathBuf,
    /// The edited name (not yet applied).
    pub name: String,
    /// Whether the text edit still wants keyboard focus (first frame only).
    pub focus: bool,
}

/// What [`create`] should make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateKind {
    File,
    Folder,
}

/// An in-progress "new item" row: where it goes and what it is called.
#[derive(Clone, Debug)]
pub struct Create {
    /// The directory the new entry lands in.
    pub dir: PathBuf,
    /// File or folder.
    pub kind: CreateKind,
    /// The typed name (not yet applied).
    pub name: String,
    /// Whether the text edit still wants keyboard focus (first frame only).
    pub focus: bool,
}

/// Expansion plus the one active inline editor, for the open project's tree.
#[derive(Debug, Default)]
pub struct TreeState {
    /// Directories the user has opened.
    expanded: HashSet<PathBuf>,
    /// The row being renamed, if any.
    pub rename: Option<Rename>,
    /// The "new file/folder" row being typed, if any.
    pub create: Option<Create>,
    /// The path awaiting delete confirmation, if any.
    pub confirm_delete: Option<PathBuf>,
}

impl TreeState {
    /// Opens or closes `path`'s children.
    pub fn toggle(&mut self, path: &Path) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_path_buf());
        }
    }

    /// Opens `path`'s children without touching the others.
    ///
    /// Used when a project opens, so its root folder starts shown.
    pub fn expand(&mut self, path: &Path) {
        self.expanded.insert(path.to_path_buf());
    }

    /// Whether `path`'s children are showing.
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// Whether `path`'s row is being renamed.
    pub fn is_renaming(&self, path: &Path) -> bool {
        self.rename
            .as_ref()
            .is_some_and(|rename| rename.path == path)
    }

    /// Starts renaming `path`, seeded with its current name.
    ///
    /// Only one row edits at a time; a second request replaces the first.
    pub fn begin_rename(&mut self, path: &Path) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.create = None;
        self.rename = Some(Rename {
            path: path.to_path_buf(),
            name,
            focus: true,
        });
    }

    /// Starts the "new entry" row inside `dir`, opening it so the row is visible.
    pub fn begin_create(&mut self, dir: &Path, kind: CreateKind) {
        self.rename = None;
        self.create = Some(Create {
            dir: dir.to_path_buf(),
            kind,
            name: String::new(),
            focus: true,
        });
        self.expand(dir);
    }

    /// Drops any in-progress rename/create row (project switch, focus loss).
    pub fn clear_edits(&mut self) {
        self.rename = None;
        self.create = None;
    }
}

/// The entries of `dir`, directories first and each group alphabetically.
///
/// Read on demand: the tree is small, an unreadable directory simply shows no
/// children, and an error every frame would flood the console.
pub fn entries(dir: &Path) -> Vec<TreeEntry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<TreeEntry> = read
        .filter_map(|entry| entry.ok())
        .map(|entry| TreeEntry {
            path: entry.path(),
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false),
        })
        .collect();
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    entries
}

/// Creates `dir/<name>`, as a file or a folder, and returns its path.
///
/// Refuses a name that is empty, whitespace, `.`/`..`, or contains a path
/// separator, and an entry that already exists — both would otherwise surface
/// as an opaque OS error.
pub fn create(dir: &Path, kind: CreateKind, name: &str) -> Result<PathBuf, String> {
    let name = check_name(name)?;
    let path = dir.join(name);
    if path.exists() {
        return Err(format!("`{name}` already exists there"));
    }
    match kind {
        CreateKind::File => {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        }
        CreateKind::Folder => {
            std::fs::create_dir(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        }
    }
    Ok(path)
}

/// Renames `path` to `<its directory>/<name>` and returns the new path.
///
/// A rename to the same name is a success that moves nothing, which is what the
/// inline editor produces when it loses focus untouched.
pub fn rename(path: &Path, name: &str) -> Result<PathBuf, String> {
    let name = check_name(name)?;
    let current = path
        .file_name()
        .map(|current| current.to_string_lossy())
        .unwrap_or_default();
    if current == name {
        return Ok(path.to_path_buf());
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    let destination = parent.join(name);
    if destination.exists() {
        return Err(format!("`{name}` already exists there"));
    }
    std::fs::rename(path, &destination).map_err(|error| format!("{}: {error}", error))?;
    Ok(destination)
}

/// Deletes `path`: a folder with everything inside it, or a file.
pub fn delete(path: &Path) -> Result<(), String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path).map_err(|error| format!("{}: {error}", path.display()))
    } else {
        std::fs::remove_file(path).map_err(|error| format!("{}: {error}", path.display()))
    }
}

/// A name the filesystem can host: non-empty, no separators, no `.` games.
fn check_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a name is needed".to_string());
    }
    if name == "." || name == ".." {
        return Err(format!("`{name}` is not a usable name"));
    }
    if name.contains(['/', '\\']) {
        return Err(format!("`{name}` cannot contain a path separator"));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    fn names(dir: &Path) -> Vec<String> {
        entries(dir).into_iter().map(|entry| entry.name).collect()
    }

    #[test]
    fn entries_list_directories_first_then_files() {
        let scratch = Scratch::new("tree-entries");
        std::fs::write(scratch.join("zebra.txt"), "z").expect("a file");
        std::fs::write(scratch.join("alpha.txt"), "a").expect("a file");
        std::fs::create_dir(scratch.join("src")).expect("a dir");
        std::fs::create_dir(scratch.join("assets")).expect("a dir");

        assert_eq!(
            names(&scratch.path),
            ["assets", "src", "alpha.txt", "zebra.txt"]
        );
    }

    #[test]
    fn a_missing_directory_lists_nothing() {
        assert!(entries(Path::new("/definitely/not/here")).is_empty());
    }

    #[test]
    fn create_makes_files_and_folders_and_refuses_duplicates() {
        let scratch = Scratch::new("tree-create");
        let file = create(&scratch.path, CreateKind::File, "main.rs").expect("a file");
        assert!(file.is_file());
        let folder = create(&scratch.path, CreateKind::Folder, "scenes").expect("a folder");
        assert!(folder.is_dir());

        let duplicate = create(&scratch.path, CreateKind::File, "main.rs").unwrap_err();
        assert!(duplicate.contains("already exists"), "{duplicate}");
        let empty = create(&scratch.path, CreateKind::File, "  ").unwrap_err();
        assert!(empty.contains("name"), "{empty}");
        let nested = create(&scratch.path, CreateKind::File, "a/b.rs").unwrap_err();
        assert!(nested.contains("separator"), "{nested}");
    }

    #[test]
    fn rename_moves_a_file_and_wont_clobber() {
        let scratch = Scratch::new("tree-rename");
        let old = create(&scratch.path, CreateKind::File, "old.txt").expect("a file");
        create(&scratch.path, CreateKind::File, "taken.txt").expect("a file");

        let renamed = rename(&old, "new.txt").expect("renamed");
        assert_eq!(renamed, scratch.join("new.txt"));
        assert!(renamed.is_file());
        assert!(!old.exists());

        // Renaming to the current name is a no-op success.
        assert_eq!(rename(&renamed, "new.txt").expect("same name"), renamed);

        let collision = rename(&renamed, "taken.txt").unwrap_err();
        assert!(collision.contains("already exists"), "{collision}");
    }

    #[test]
    fn delete_removes_files_and_whole_folders() {
        let scratch = Scratch::new("tree-delete");
        let file = create(&scratch.path, CreateKind::File, "loose.txt").expect("a file");
        let folder = create(&scratch.path, CreateKind::Folder, "stuff").expect("a folder");
        create(&folder, CreateKind::File, "inner.txt").expect("a file");

        delete(&file).expect("file deleted");
        assert!(!file.exists());
        delete(&folder).expect("folder deleted");
        assert!(!folder.exists());

        let missing = delete(&scratch.join("nope")).unwrap_err();
        assert!(missing.contains("nope"), "{missing}");
    }

    #[test]
    fn expansion_is_per_path() {
        let mut tree = TreeState::default();
        let a = PathBuf::from("/a");
        let b = PathBuf::from("/b");
        assert!(!tree.is_expanded(&a));
        tree.toggle(&a);
        assert!(tree.is_expanded(&a));
        tree.toggle(&b);
        assert!(tree.is_expanded(&a) && tree.is_expanded(&b));
        tree.toggle(&a);
        assert!(!tree.is_expanded(&a) && tree.is_expanded(&b));
    }

    /// One editor, one row: a second request takes over rather than stacking.
    #[test]
    fn only_one_inline_editor_is_active() {
        let mut tree = TreeState::default();
        tree.begin_rename(Path::new("/a/file.txt"));
        assert_eq!(
            tree.rename.as_ref().map(|rename| rename.name.as_str()),
            Some("file.txt")
        );
        tree.begin_create(Path::new("/a"), CreateKind::Folder);
        assert!(tree.rename.is_none());
        assert!(
            tree.is_expanded(Path::new("/a")),
            "the new row must be visible"
        );

        tree.begin_rename(Path::new("/a/file.txt"));
        assert!(tree.create.is_none());
        tree.clear_edits();
        assert!(tree.rename.is_none() && tree.create.is_none());
    }
}
