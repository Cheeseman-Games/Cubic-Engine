//! Assets: the server that turns a path into a handle, and the handles games
//! draw with.
//!
//! A game never holds a texture, a font's bytes, or a path string. It asks an
//! [`AssetServer`] for an asset, gets back an [`AssetHandle`], and puts that
//! handle in the components it draws.
//!
//! ```
//! use cubic_core::assets::{AssetBundle, AssetServer, TextureHandle};
//!
//! static BUNDLE: AssetBundle =
//!     AssetBundle::new(&[("assets/hero.png", b"pixels" as &[u8])]);
//! let mut assets = AssetServer::embedded(&BUNDLE);
//!
//! let hero: TextureHandle = assets.load_texture("assets/hero.png").unwrap();
//! // The bytes are waiting for the backend to import them; the handle is
//! // already safe to store on an entity.
//! assert_eq!(assets.drain_pending().len(), 1);
//! assert!(assets.drain_pending().is_empty());
//! # let _ = hero;
//! ```
//!
//! The handle is a *name*, not data. That is the whole reason hot reload is
//! possible: when the file behind it changes, the same handle comes back
//! carrying a newer [`version`](AssetServer::version), and every component that
//! holds it is already pointing at the new bytes. Nothing re-resolves a path at
//! draw time, and nothing has to be rebuilt.
//!
//! # Two sources, one API
//!
//! Where the bytes come from is a property of the build, not of the game:
//!
//! - **Development** — [`AssetServer::from_dir`] reads files under a project
//!   root, and [`AssetServer::watching`] additionally watches them: edit a
//!   texture, and [`pump`](AssetServer::pump) re-reads it on the next frame.
//! - **Release** — [`AssetServer::embedded`] reads an [`AssetBundle`], which is
//!   `include_bytes!` output compiled into the binary, so a shipped game carries
//!   its assets and cannot be broken by a missing directory.
//!
//! The game calls [`load_texture`](AssetServer::load_texture) either way.
//!
//! # Importing is the backend's job
//!
//! This module is GPU-free and knows nothing about wgpu, so it never decodes an
//! image. What it does is produce [`PendingAsset`]s — a handle, a kind, a
//! version and the bytes — for the backend to import with
//! [`drain_pending`](AssetServer::drain_pending). A game that wants to know what
//! happened reads [`drain_events`](AssetServer::drain_events): which asset
//! loaded, which one was re-read, which one failed, which one was dropped.
//!
//! # Paths
//!
//! An asset path is relative to the project root, uses `/` on every platform,
//! and may not leave that root: `..` and absolute paths are refused with
//! [`AssetError::OutsideRoot`]. One asset has one handle however it is spelled,
//! so `"assets/hero.png"`, `"./assets/hero.png"` and `"assets//hero.png"` are the
//! same asset with one reference between them.

use std::borrow::Cow;
use std::collections::BTreeMap;
#[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
use std::collections::BTreeSet;
use std::fmt;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

/// Asset file extensions read as textures.
const TEXTURE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "bmp", "tga", "webp", "ktx"];

/// Asset file extensions read as fonts.
const FONT_EXTENSIONS: [&str; 4] = ["ttf", "otf", "ttc", "otc"];

/// A unique handle to a loaded asset.
///
/// Handles are stable for the lifetime of the asset. When an asset is
/// reloaded (dev mode), the same handle continues to refer to the new data.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AssetHandle(NonZeroU64);

impl AssetHandle {
    /// Create a new handle from a non-zero id.
    pub fn new(id: NonZeroU64) -> Self {
        Self(id)
    }

    /// Get the raw id of this handle.
    pub fn id(&self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for AssetHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "asset#{}", self.id())
    }
}

impl From<NonZeroU64> for AssetHandle {
    fn from(id: NonZeroU64) -> Self {
        Self(id)
    }
}

/// Handle to a 2D texture.
///
/// Produced by [`AssetServer::load_texture`] and what
/// [`DrawCommand::Texture`](crate::render::DrawCommand::Texture) names, so the
/// component a game stores it on is most of what a sprite is.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TextureHandle(AssetHandle);

impl TextureHandle {
    /// Wrap a raw handle id.
    ///
    /// For code that keeps ids rather than handles — a serializer, a debug view.
    /// Ids from an [`AssetServer`] are better kept as the handles it hands out.
    pub fn new(id: NonZeroU64) -> Self {
        Self(AssetHandle::new(id))
    }

    /// The asset handle this is a face of, for releasing it and for the registry
    /// APIs that speak in plain handles.
    pub fn asset(&self) -> AssetHandle {
        self.0
    }
}

impl From<AssetHandle> for TextureHandle {
    fn from(handle: AssetHandle) -> Self {
        Self(handle)
    }
}

impl fmt::Display for TextureHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "texture#{}", self.0.id())
    }
}

/// Handle to a font.
///
/// A font asset is bytes until a backend hands them to a font database; the
/// handle is what a scene stores so a reloaded font is picked up by whatever
/// ends up drawing with it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FontHandle(AssetHandle);

impl FontHandle {
    /// Wrap a raw handle id. See [`TextureHandle::new`].
    pub fn new(id: NonZeroU64) -> Self {
        Self(AssetHandle::new(id))
    }

    /// The asset handle this is a face of.
    pub fn asset(&self) -> AssetHandle {
        self.0
    }
}

impl From<AssetHandle> for FontHandle {
    fn from(handle: AssetHandle) -> Self {
        Self(handle)
    }
}

impl fmt::Display for FontHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "font#{}", self.0.id())
    }
}

/// What an asset is, which is what decides who imports it.
///
/// The kind is fixed by how the asset was asked for — [`load_texture`] rather
/// than [`load_bytes`] — so a project with no `.png`-by-convention naming still
/// gets textures, and a font bundle entry does not need a `.ttf` name. Asking for
/// a loaded path as a different kind is [`AssetError::WrongKind`].
///
/// [`load_texture`]: AssetServer::load_texture
/// [`load_bytes`]: AssetServer::load_bytes
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AssetKind {
    /// An image, which a GPU backend decodes and uploads.
    Texture,
    /// A font file, which a text backend registers with a font database.
    Font,
    /// Anything else, handed to the game as bytes.
    Bytes,
}

impl AssetKind {
    /// The kind a path's extension implies.
    ///
    /// Unknown extensions are [`AssetKind::Bytes`]: an asset the engine has no
    /// opinion about is still loadable.
    pub fn of_path(path: &str) -> Self {
        let Some(extension) = path.rsplit_once('.').map(|(_, tail)| tail) else {
            return Self::Bytes;
        };
        let extension = extension.to_ascii_lowercase();
        if TEXTURE_EXTENSIONS.contains(&extension.as_str()) {
            Self::Texture
        } else if FONT_EXTENSIONS.contains(&extension.as_str()) {
            Self::Font
        } else {
            Self::Bytes
        }
    }
}

impl fmt::Display for AssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Texture => write!(f, "texture"),
            Self::Font => write!(f, "font"),
            Self::Bytes => write!(f, "bytes"),
        }
    }
}

/// Why an asset could not be loaded.
///
/// One type for the whole surface on purpose: a game that fails a load gets a
/// message it can print, not four cases to handle — and only [`Io`](Self::Io)
/// ever means "the disk said no".
#[derive(Debug)]
pub enum AssetError {
    /// Nothing by that name is in the bundle.
    NotFound {
        /// The path as it was asked for.
        path: String,
    },
    /// The path does not stay inside the project root.
    OutsideRoot {
        /// The path as it was asked for.
        path: String,
    },
    /// The path is already loaded as a different kind than it was asked for.
    WrongKind {
        /// The path as it was asked for.
        path: String,
        /// The kind it was asked for.
        wanted: AssetKind,
        /// The kind it is already loaded as.
        loaded: AssetKind,
    },
    /// The file could not be read.
    Io {
        /// The path as it was asked for.
        path: String,
        /// What the filesystem said.
        source: std::io::Error,
    },
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { path } => write!(f, "no asset at `{path}`"),
            Self::OutsideRoot { path } => write!(
                f,
                "`{path}` is not inside the project: asset paths are relative to its root"
            ),
            Self::WrongKind {
                path,
                wanted,
                loaded,
            } => write!(f, "`{path}` is already loaded as {loaded}, not as {wanted}"),
            Self::Io { path, source } => write!(f, "`{path}` could not be read: {source}"),
        }
    }
}

impl std::error::Error for AssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The bytes of a project's assets, compiled into the binary.
///
/// A bundle is what a release build loads from: `include_bytes!` output, listed
/// by the paths a game asks for.
///
/// ```
/// use cubic_core::assets::AssetBundle;
///
/// static LOGO: &[u8] = b"pretend png";
/// static SCENE: &[u8] = b"pretend scene";
///
/// // In a real project the two are `include_bytes!("../assets/logo.png")` and
/// // friends, which is what makes the binary carry its assets.
/// static BUNDLE: AssetBundle = AssetBundle::new(&[
///     ("assets/logo.png", LOGO),
///     ("assets/scenes/main.rsn", SCENE),
/// ]);
///
/// assert_eq!(BUNDLE.get("assets/logo.png"), Some(&b"pretend png"[..]));
/// assert_eq!(
///     BUNDLE.get("./assets/scenes/main.rsn"),
///     Some(&b"pretend scene"[..])
/// );
/// assert_eq!(BUNDLE.get("assets/ghost.png"), None);
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct AssetBundle(&'static [(&'static str, &'static [u8])]);

impl AssetBundle {
    /// The assets a build carries, by path.
    ///
    /// Paths are looked up the way [`AssetServer`] normalizes them, so a game
    /// asking for `"./assets/hero.png"` still finds an `"assets/hero.png"` entry.
    pub const fn new(assets: &'static [(&'static str, &'static [u8])]) -> Self {
        Self(assets)
    }

    /// The bytes of `path`, if the bundle carries it.
    pub fn get(&self, path: &str) -> Option<&'static [u8]> {
        let path = normalize(path).ok()?;
        self.0
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| *bytes)
    }

    /// How many assets the bundle carries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the bundle carries nothing.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// An asset whose bytes are ready for the backend to import.
///
/// The server produces these and never interprets them: a GPU backend decodes
/// and uploads a [`Texture`](AssetKind::Texture), a text backend registers a
/// [`Font`](AssetKind::Font) with its font database, and a game reads
/// [`Bytes`](AssetKind::Bytes) itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingAsset {
    /// The handle the new bytes belong to — the same handle as before, for a
    /// reload.
    pub handle: AssetHandle,
    /// The asset's path, normalized.
    pub path: String,
    /// What the backend should do with the bytes.
    pub kind: AssetKind,
    /// The asset's version: 1 for the load, plus one per successful re-read.
    pub version: u32,
    /// The bytes themselves, borrowed straight out of the bundle when the
    /// project embeds its assets and owned when it reads them from disk.
    pub bytes: Cow<'static, [u8]>,
}

impl PendingAsset {
    /// The bytes to import.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Something that happened to a loaded asset.
///
/// Events are how a game reacts to loading — drawing the first texture once it
/// exists, reporting a missing asset in an inspector, re-laying out when a font
/// changes — and how a host surfaces them without stealing them: whoever asked
/// for events drains them, and the backend only takes
/// [`pending`](AssetServer::drain_pending).
#[derive(Debug)]
pub enum AssetEvent {
    /// An asset was read for the first time and is waiting to be imported.
    Loaded {
        /// The new asset's handle.
        handle: AssetHandle,
        /// Its path, normalized.
        path: String,
        /// What it was loaded as.
        kind: AssetKind,
    },
    /// An asset was re-read and its new bytes are waiting to be imported.
    Reloaded {
        /// The handle that carried the new bytes.
        handle: AssetHandle,
        /// Its path, normalized.
        path: String,
        /// The version the new bytes are.
        version: u32,
    },
    /// An asset could not be re-read; whatever was loaded stays loaded.
    Failed {
        /// The path that failed, normalized.
        path: String,
        /// Why.
        error: AssetError,
    },
    /// An asset's last reference went away and it is gone.
    Released {
        /// The handle that is no longer valid.
        handle: AssetHandle,
        /// Its path, normalized.
        path: String,
    },
}

impl AssetEvent {
    /// The asset's path, however the event happened.
    pub fn path(&self) -> &str {
        match self {
            Self::Loaded { path, .. }
            | Self::Reloaded { path, .. }
            | Self::Failed { path, .. }
            | Self::Released { path, .. } => path,
        }
    }

    /// A short name for the transition, for logs and tests.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Loaded { .. } => "loaded",
            Self::Reloaded { .. } => "reloaded",
            Self::Failed { .. } => "failed",
            Self::Released { .. } => "released",
        }
    }
}

/// One loaded asset, as the registry holds it. What an asset panel lists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadedAsset<'a> {
    /// Its path, normalized.
    pub path: &'a str,
    /// The handle components refer to it by.
    pub handle: AssetHandle,
    /// What it was loaded as.
    pub kind: AssetKind,
    /// Its version, incremented on every successful re-read.
    pub version: u32,
    /// How many times it has been loaded without a matching
    /// [`release`](AssetServer::release).
    pub refs: usize,
}

/// The registry that turns paths into handles and keeps their bytes current.
///
/// See the [module docs](self) for the contract. A server holds no GPU state and
/// is cheap to build; it belongs to the game, and the runtime reaches it through
/// [`Game::assets_mut`](crate::Game::assets_mut) to pump it and import what it
/// queues.
pub struct AssetServer {
    source: Source,
    /// Loaded assets, by normalized path. A [`BTreeMap`] so a listing is sorted
    /// and does not depend on hashing.
    entries: BTreeMap<String, Entry>,
    /// The reverse lookup, so a handle can name its path.
    paths: BTreeMap<u64, String>,
    /// Bytes waiting for the backend, in the order they were queued.
    pending: Vec<PendingAsset>,
    /// What happened, in order, for whoever drains it.
    events: Vec<AssetEvent>,
    /// The next handle id. Monotonic, so an id is never reused and a stale handle
    /// cannot silently address a later asset.
    next_id: u64,
    /// Directories already handed to the watcher, so each is watched once.
    #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
    watched: BTreeSet<PathBuf>,
    /// The filesystem watcher, when the server was built with one.
    #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
    watcher: Option<Watcher>,
}

/// Where a server reads its bytes from.
#[derive(Debug)]
enum Source {
    /// A project directory, read at load time and on every reload.
    Directory { root: PathBuf },
    /// Bytes compiled into the binary.
    Embedded(&'static AssetBundle),
}

/// One loaded asset.
#[derive(Debug)]
struct Entry {
    handle: AssetHandle,
    kind: AssetKind,
    version: u32,
    refs: usize,
}

impl fmt::Debug for AssetServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetServer")
            .field("source", &self.source)
            .field("loaded", &self.entries.len())
            .field("pending", &self.pending.len())
            .field("watching", &self.is_watching())
            .finish()
    }
}

impl AssetServer {
    /// A server that reads assets from a project directory, without watching it.
    ///
    /// `root` is the directory asset paths are relative to — the project root, so
    /// `load_texture("assets/hero.png")` reads `<root>/assets/hero.png`. This is
    /// the development server for a build that does not reload, and the starting
    /// point [`watching`](Self::watching) adds a watcher to.
    pub fn from_dir(root: impl Into<PathBuf>) -> Self {
        Self::new(Source::Directory { root: root.into() })
    }

    /// A server that reads assets from bytes compiled into the binary.
    ///
    /// Nothing is watched and nothing is re-read: a release build ships its
    /// assets, and a project whose bundle is missing one gets
    /// [`AssetError::NotFound`] rather than a file that is not there.
    pub fn embedded(bundle: &'static AssetBundle) -> Self {
        Self::new(Source::Embedded(bundle))
    }

    /// A directory server that notices files being edited.
    ///
    /// The directories an asset is loaded from are watched, and only those: one
    /// that watched the whole project would see `target/` churn. Call
    /// [`pump`](Self::pump) once a frame to re-read whatever changed.
    ///
    /// Watching is the one thing that needs an operating system to tell it, so it
    /// is behind the `watch` feature and absent on wasm.
    #[cfg(feature = "watch")]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn watching(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let mut server = Self::from_dir(root);
        server.watcher = Some(Watcher::new()?);
        Ok(server)
    }

    /// Whether this server watches the filesystem for edits.
    pub fn is_watching(&self) -> bool {
        #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
        {
            self.watcher.is_some()
        }
        #[cfg(not(all(feature = "watch", not(target_arch = "wasm32"))))]
        {
            false
        }
    }

    /// The project root a directory server reads from.
    pub fn root(&self) -> Option<&Path> {
        match &self.source {
            Source::Directory { root } => Some(root),
            Source::Embedded(_) => None,
        }
    }

    /// Load an asset, deciding what it is from its extension.
    ///
    /// The convenience form of the typed loaders: a `.png` becomes a texture, a
    /// `.ttf` a font, anything else bytes. Returns the plain [`AssetHandle`];
    /// the typed loaders are there when the game wants the face.
    pub fn load(&mut self, path: &str) -> Result<AssetHandle, AssetError> {
        self.load_kind(path, AssetKind::of_path(path))
    }

    /// Load an image, and get the handle that names it in a draw command.
    pub fn load_texture(&mut self, path: &str) -> Result<TextureHandle, AssetError> {
        self.load_kind(path, AssetKind::Texture).map(TextureHandle)
    }

    /// Load a font file.
    pub fn load_font(&mut self, path: &str) -> Result<FontHandle, AssetError> {
        self.load_kind(path, AssetKind::Font).map(FontHandle)
    }

    /// Load an asset the game reads itself.
    pub fn load_bytes(&mut self, path: &str) -> Result<AssetHandle, AssetError> {
        self.load_kind(path, AssetKind::Bytes)
    }

    /// Load `path` as `kind`, or take another reference to it if it is loaded.
    fn load_kind(&mut self, path: &str, kind: AssetKind) -> Result<AssetHandle, AssetError> {
        let path = normalize(path)?;
        if let Some(entry) = self.entries.get_mut(&path) {
            if entry.kind != kind {
                return Err(AssetError::WrongKind {
                    path,
                    wanted: kind,
                    loaded: entry.kind,
                });
            }
            entry.refs += 1;
            return Ok(entry.handle);
        }

        // Read before interning: a path that cannot be read leaves nothing
        // behind, so a typo is a failed call rather than a handle that is never
        // ready.
        let bytes = self.read(&path)?;

        let handle = self.intern(path.clone(), kind);
        self.pending.push(PendingAsset {
            handle,
            path: path.clone(),
            kind,
            version: 1,
            bytes,
        });
        self.watch_loaded(&path);
        self.events.push(AssetEvent::Loaded { handle, path, kind });
        Ok(handle)
    }

    /// Whether `path` is loaded, without taking a reference to it.
    ///
    /// How a game checks "did that load?", and how an inspector looks an asset
    /// up without keeping it alive.
    pub fn handle(&self, path: &str) -> Option<AssetHandle> {
        let path = normalize(path).ok()?;
        self.entries.get(&path).map(|entry| entry.handle)
    }

    /// The handle `path` names, taking a reference to it.
    ///
    /// The counterpart of [`handle`](Self::handle) for code that stores paths:
    /// this is what turns "the asset I was told about" into a usable reference,
    /// and it fails for an asset that is not loaded.
    pub fn acquire(&mut self, path: &str) -> Result<AssetHandle, AssetError> {
        let path = normalize(path)?;
        let Some(entry) = self.entries.get_mut(&path) else {
            return Err(AssetError::NotFound { path });
        };
        entry.refs += 1;
        Ok(entry.handle)
    }

    /// Give up one reference to `handle`, dropping the asset if it was the last.
    ///
    /// Returns whether the asset went away, which is the cue for a backend to
    /// free whatever it imported for it. A handle that is not loaded is ignored
    /// rather than an error: releasing twice is a mistake in the game, not a
    /// condition worth failing a frame over.
    pub fn release(&mut self, handle: AssetHandle) -> bool {
        let Some(path) = self.paths.get(&handle.id()).cloned() else {
            return false;
        };
        let Some(entry) = self.entries.get_mut(&path) else {
            return false;
        };
        entry.refs = entry.refs.saturating_sub(1);
        if entry.refs > 0 {
            return false;
        }
        self.entries.remove(&path);
        self.paths.remove(&handle.id());
        self.events.push(AssetEvent::Released { handle, path });
        true
    }

    /// The kind `handle` was loaded as.
    pub fn kind(&self, handle: AssetHandle) -> Option<AssetKind> {
        self.entry(handle).map(|(_, entry)| entry.kind)
    }

    /// How many times `handle` has been loaded without a matching
    /// [`release`](Self::release).
    pub fn refs(&self, handle: AssetHandle) -> Option<usize> {
        self.entry(handle).map(|(_, entry)| entry.refs)
    }

    /// How many times `handle`'s bytes have been read: 1 for the load, plus one
    /// per successful re-read.
    ///
    /// A handle's version is what a component compares to notice that what it
    /// points at has been replaced.
    pub fn version(&self, handle: AssetHandle) -> Option<u32> {
        self.entry(handle).map(|(_, entry)| entry.version)
    }

    /// The path `handle` was loaded from.
    pub fn path(&self, handle: AssetHandle) -> Option<&str> {
        self.entry(handle).map(|(path, _)| path.as_str())
    }

    /// Whether `handle` names a loaded asset.
    pub fn is_loaded(&self, handle: AssetHandle) -> bool {
        self.entry(handle).is_some()
    }

    /// Every loaded asset, sorted by path.
    ///
    /// What the editor's asset panel lists: the registry without its internals.
    pub fn loaded(&self) -> impl Iterator<Item = LoadedAsset<'_>> {
        self.entries.iter().map(|(path, entry)| LoadedAsset {
            path,
            handle: entry.handle,
            kind: entry.kind,
            version: entry.version,
            refs: entry.refs,
        })
    }

    /// How many assets are loaded.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is loaded.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Re-read `path`, if it names a loaded asset.
    ///
    /// Returns whether a loaded asset was affected: a path nobody loaded is
    /// ignored, so a project whose `target/` churns under the watcher costs
    /// nothing, and an asset loaded afterwards is not a reload. A file that cannot
    /// be read is reported as [`AssetEvent::Failed`] and leaves the loaded
    /// version alone — an editor that saves by truncating a file should not blank
    /// the texture for the frame in between.
    ///
    /// This is the seam the watcher drives and the seam a test drives, so the
    /// reload path is checkable without waiting on the filesystem.
    pub fn reload(&mut self, path: &str) -> bool {
        let Ok(path) = normalize(path) else {
            return false;
        };
        let Some((handle, kind)) = self
            .entries
            .get(&path)
            .map(|entry| (entry.handle, entry.kind))
        else {
            return false;
        };

        let bytes = match self.read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.events.push(AssetEvent::Failed { path, error });
                return true;
            }
        };

        // Nothing can remove an entry between the read and this — `reload` holds
        // `&mut self`, and `release` is the only removal path.
        let entry = self
            .entries
            .get_mut(&path)
            .expect("the entry was just read");
        entry.version += 1;
        let version = entry.version;
        self.pending.push(PendingAsset {
            handle,
            path: path.clone(),
            kind,
            version,
            bytes,
        });
        self.events.push(AssetEvent::Reloaded {
            handle,
            path,
            version,
        });
        true
    }

    /// Re-read whatever the watcher has seen since the last pump, returning how
    /// many loaded assets were re-read.
    ///
    /// Cheap and non-blocking, and deduplicated per path because saving one file
    /// routinely produces several events. A server with no watcher returns 0, so
    /// a host can call it every frame without knowing which build it is running.
    #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
    pub fn pump(&mut self) -> usize {
        let Some(watcher) = self.watcher.as_ref() else {
            return 0;
        };
        let mut changed: BTreeSet<String> = BTreeSet::new();
        for path in watcher.drain() {
            if let Some(relative) = self.relative(&path) {
                changed.insert(relative);
            }
        }
        changed.into_iter().filter(|path| self.reload(path)).count()
    }

    /// Re-read whatever changed, in a build with no watcher to ask.
    ///
    /// The hot-reload step is a no-op here rather than missing, so a host — the
    /// runtime, an editor, a test — can call [`pump`](Self::pump) without knowing
    /// whether the `watch` feature is on.
    #[cfg(not(all(feature = "watch", not(target_arch = "wasm32"))))]
    pub fn pump(&mut self) -> usize {
        0
    }

    /// Take the bytes waiting to be imported, leaving the queue empty.
    ///
    /// The backend's share of the contract, and the only thing a host takes from a
    /// game's server: what was imported stays visible to the game, through
    /// [`version`](Self::version) and [`drain_events`](Self::drain_events).
    pub fn drain_pending(&mut self) -> Vec<PendingAsset> {
        std::mem::take(&mut self.pending)
    }

    /// Take what has happened, leaving the event log empty.
    pub fn drain_events(&mut self) -> Vec<AssetEvent> {
        std::mem::take(&mut self.events)
    }

    /// Whether anything is waiting to be imported.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// A server over one source, with nothing loaded.
    fn new(source: Source) -> Self {
        Self {
            source,
            entries: BTreeMap::new(),
            paths: BTreeMap::new(),
            pending: Vec::new(),
            events: Vec::new(),
            next_id: 1,
            #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
            watched: BTreeSet::new(),
            #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
            watcher: None,
        }
    }

    /// Allocate a handle for `path`, which is known to be readable by now.
    fn intern(&mut self, path: String, kind: AssetKind) -> AssetHandle {
        let handle = AssetHandle::new(
            NonZeroU64::new(self.next_id).expect("the next id starts at 1 and only grows"),
        );
        self.next_id += 1;
        self.entries.insert(
            path.clone(),
            Entry {
                handle,
                kind,
                version: 1,
                refs: 1,
            },
        );
        self.paths.insert(handle.id(), path);
        handle
    }

    /// The entry for `handle`, and the path it is under.
    fn entry(&self, handle: AssetHandle) -> Option<(&String, &Entry)> {
        let path = self.paths.get(&handle.id())?;
        Some((path, self.entries.get(path)?))
    }

    /// Read an asset's current bytes from wherever this server reads them.
    fn read(&self, path: &str) -> Result<Cow<'static, [u8]>, AssetError> {
        match &self.source {
            Source::Embedded(bundle) => {
                bundle
                    .get(path)
                    .map(Cow::Borrowed)
                    .ok_or_else(|| AssetError::NotFound {
                        path: path.to_string(),
                    })
            }
            Source::Directory { root } => {
                let full = resolve(root, path).ok_or_else(|| AssetError::OutsideRoot {
                    path: path.to_string(),
                })?;
                std::fs::read(&full)
                    .map(Cow::Owned)
                    .map_err(|source| AssetError::Io {
                        path: path.to_string(),
                        source,
                    })
            }
        }
    }

    /// The project-relative spelling of a path the watcher reported, if it is
    /// under the root at all.
    #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
    fn relative(&self, path: &Path) -> Option<String> {
        let Source::Directory { root } = &self.source else {
            return None;
        };
        normalize(&path.strip_prefix(root).ok()?.to_string_lossy()).ok()
    }

    /// Watch the directory `path` lives in, once.
    #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
    fn watch_loaded(&mut self, path: &str) {
        let root = match &self.source {
            Source::Directory { root } => root.clone(),
            Source::Embedded(_) => return,
        };
        let Some(full) = resolve(&root, path) else {
            return;
        };
        let Some(directory) = full.parent().map(Path::to_path_buf) else {
            return;
        };
        if self.watcher.is_none() || !self.watched.insert(directory.clone()) {
            return;
        }
        let outcome = self
            .watcher
            .as_mut()
            .map(|watcher| watcher.watch(&directory));
        if let Some(Err(error)) = outcome {
            // A directory that cannot be watched costs a hot reload, not a load:
            // the asset is already in memory and still draws.
            log::debug!("{} cannot be watched: {error}", directory.display());
            self.watched.remove(&directory);
        }
    }

    /// A build with no watcher has nothing to watch.
    #[cfg(not(all(feature = "watch", not(target_arch = "wasm32"))))]
    fn watch_loaded(&self, _path: &str) {}
}

/// Normalize an asset path: `/` separators, no `.` or empty segments, and no way
/// out of the project root.
///
/// The result is the one spelling of a path the server keys on, so a game can ask
/// for `"assets/hero.png"` and a Windows build script can write
/// `"assets\\hero.png"` without producing two of everything.
fn normalize(raw: &str) -> Result<String, AssetError> {
    let outside = || AssetError::OutsideRoot {
        path: raw.to_string(),
    };
    let trimmed = raw.trim();
    // A leading separator or a drive letter means the path was spelled absolute:
    // it names something outside the project, wherever it was typed.
    if Path::new(trimmed).components().any(|component| {
        matches!(
            component,
            std::path::Component::Prefix(_) | std::path::Component::RootDir
        )
    }) {
        return Err(outside());
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in trimmed.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            // `..` is refused rather than resolved: an asset path that walks out
            // of the project is a bug in whoever built it, and resolving it would
            // make one path mean different files in different projects.
            ".." => return Err(outside()),
            // A drive prefix is the one part of a path that cannot be made relative
            // portably, and `:` is not legal in a file name on Windows anyway.
            segment if segment.contains(':') => return Err(outside()),
            segment => segments.push(segment),
        }
    }
    if segments.is_empty() {
        return Err(outside());
    }
    Ok(segments.join("/"))
}

/// Join a normalized path onto a project root, refusing anything that leaves it.
///
/// `Path::join` *replaces* the root when the second half is absolute or names a
/// drive, so the path is re-normalized and re-checked rather than trusted.
fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let joined = root.join(normalize(path).ok()?);
    joined.starts_with(root).then_some(joined)
}

/// The filesystem side of hot reload, behind the `watch` feature.
///
/// A thin wrapper over `notify`: a thread turns operating system events into
/// paths on a channel, and [`drain`](Watcher::drain) takes whatever has arrived.
/// Nothing here knows what an asset is — deciding whether a path is one, and what
/// to do about it, is the server's job.
#[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
mod watcher {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Receiver, channel};

    use notify::{RecursiveMode, Watcher as _};

    /// Filesystem notifications, drained as paths.
    pub struct Watcher {
        /// Held only so the watcher thread is stopped when this is dropped.
        _inner: notify::RecommendedWatcher,
        events: Receiver<PathBuf>,
    }

    impl Watcher {
        /// Start watching. Nothing is watched yet: directories are added as assets
        /// are loaded from them.
        pub fn new() -> std::io::Result<Self> {
            let (tx, events) = channel();
            let inner = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    for path in event.paths {
                        // The server re-reads by path and ignores what it does
                        // not load, so the kind of event cannot be trusted to
                        // mean anything here.
                        let _ = tx.send(path);
                    }
                }
            })
            .map_err(io_error)?;
            Ok(Self {
                _inner: inner,
                events,
            })
        }

        /// Watch `directory`, without recursing into it.
        ///
        /// Non-recursive on purpose: a project's assets are a few directories, and
        /// watching `target/` recursively would drown them.
        pub fn watch(&mut self, directory: &Path) -> std::io::Result<()> {
            self._inner
                .watch(directory, RecursiveMode::NonRecursive)
                .map_err(io_error)
        }

        /// Every path reported since the last call.
        pub fn drain(&self) -> Vec<PathBuf> {
            self.events.try_iter().collect()
        }
    }

    /// `notify`'s error is opaque; an asset server wants the same `io::Error` its
    /// own reads report, so a host handles one failure type.
    fn io_error(error: notify::Error) -> std::io::Error {
        std::io::Error::other(error.to_string())
    }
}

#[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
use watcher::Watcher;

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that cleans itself up, so a test can write real files.
    struct TempDir(PathBuf);

    impl TempDir {
        /// A fresh directory named after the test, so two tests never share one.
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!("cubic-assets-{label}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp asset dir");
            Self(path)
        }

        fn write(&self, relative: &str, bytes: &[u8]) {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("temp asset parent");
            }
            std::fs::write(&path, bytes).expect("temp asset file");
        }

        fn remove(&self, relative: &str) {
            let _ = std::fs::remove_file(self.0.join(relative));
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The bundle the embedded-server tests share.
    ///
    /// A `static`, because an embedded server borrows its bundle for `'static` —
    /// exactly the lifetime a release build's `include_bytes!` output has.
    static BUNDLE: AssetBundle = AssetBundle::new(&[
        ("assets/hero.png", b"hero bytes"),
        ("assets/notes.txt", b"notes"),
        ("assets/logo", b"font bytes"),
    ]);

    fn embedded() -> AssetServer {
        AssetServer::embedded(&BUNDLE)
    }

    /// The paths of a server's queued imports, in order.
    fn pending_paths(server: &mut AssetServer) -> Vec<String> {
        server
            .drain_pending()
            .into_iter()
            .map(|asset| asset.path)
            .collect()
    }

    #[test]
    fn the_documented_bundle_lookups_work() {
        let assets = &BUNDLE;
        assert_eq!(assets.get("assets/hero.png"), Some(&b"hero bytes"[..]));
        assert_eq!(assets.get("./assets/notes.txt"), Some(&b"notes"[..]));
        assert_eq!(assets.get("assets/missing.png"), None);
        assert_eq!(assets.len(), 3);
        assert!(!assets.is_empty());
        assert!(AssetBundle::default().is_empty());
    }

    #[test]
    fn one_path_spellings_are_one_asset() {
        let mut server = embedded();
        let first = server.load_texture("assets/hero.png").unwrap();

        for path in ["./assets/hero.png", "assets//hero.png", r"assets\hero.png"] {
            assert_eq!(
                server.load_texture(path).unwrap(),
                first,
                "{path} is one asset"
            );
        }

        assert_eq!(
            server.refs(first.asset()),
            Some(4),
            "every load takes another reference, however it was spelled"
        );
        assert_eq!(server.len(), 1);
    }

    #[test]
    fn a_load_queues_the_bytes_once_for_the_backend() {
        let mut server = embedded();

        let hero = server.load_texture("assets/hero.png").unwrap();

        assert_eq!(pending_paths(&mut server), ["assets/hero.png"]);
        assert!(!server.has_pending(), "the queue is drained, not copied");
        assert_eq!(server.kind(hero.asset()), Some(AssetKind::Texture));
        assert_eq!(server.version(hero.asset()), Some(1));
        assert_eq!(server.path(hero.asset()), Some("assets/hero.png"));
        assert!(server.is_loaded(hero.asset()));
    }

    /// Embedded bytes are borrowed, not copied: a bundle of a hundred megabytes of
    /// textures should not cost a second hundred.
    #[test]
    fn embedded_bytes_are_not_copied_to_be_pending() {
        let mut server = embedded();
        server.load_texture("assets/hero.png").unwrap();

        let PendingAsset {
            ref bytes, kind, ..
        } = server.drain_pending().into_iter().next().unwrap();
        assert!(matches!(bytes, Cow::Borrowed(_)), "bytes were copied");
        assert_eq!(kind, AssetKind::Texture);
    }

    #[test]
    fn a_repeated_load_does_not_queue_a_second_import() {
        let mut server = embedded();
        server.load_texture("assets/hero.png").unwrap();
        server.drain_pending();
        server.drain_events();

        server.load_texture("assets/hero.png").unwrap();

        assert!(
            !server.has_pending(),
            "the same bytes must not import twice"
        );
        assert!(
            server.drain_events().is_empty(),
            "and taking a reference is not a reload"
        );
    }

    #[test]
    fn kinds_come_from_the_loader_not_only_the_extension() {
        let mut server = embedded();

        // A font named without an extension a font database would recognize: the
        // loader is what decides what an asset is.
        let font = server.load_font("assets/logo").unwrap();
        assert_eq!(server.kind(font.asset()), Some(AssetKind::Font));
        assert_eq!(pending_paths(&mut server), ["assets/logo"]);

        assert_eq!(AssetKind::of_path("a/b/c.PNG"), AssetKind::Texture);
        assert_eq!(AssetKind::of_path("a/b/c.ttf"), AssetKind::Font);
        assert_eq!(AssetKind::of_path("a/b/c.ttc"), AssetKind::Font);
        assert_eq!(AssetKind::of_path("a/b/c.wave"), AssetKind::Bytes);
        assert_eq!(AssetKind::of_path("a/b/c"), AssetKind::Bytes);
        assert_eq!(AssetKind::of_path("a/b.c/d"), AssetKind::Bytes);
    }

    #[test]
    fn asking_for_a_loaded_path_as_another_kind_is_refused() {
        let mut server = embedded();
        server.load_texture("assets/hero.png").unwrap();

        let error = server.load_font("assets/hero.png").unwrap_err();

        assert!(
            matches!(
                error,
                AssetError::WrongKind {
                    loaded: AssetKind::Texture,
                    wanted: AssetKind::Font,
                    ..
                }
            ),
            "{error}"
        );
        assert_eq!(server.len(), 1, "the failed load took no reference");
    }

    #[test]
    fn the_last_release_drops_the_asset_and_earlier_ones_do_not() {
        let mut server = embedded();
        server.load_texture("assets/hero.png").unwrap();
        let handle = server.handle("assets/hero.png").unwrap();
        server.acquire("assets/hero.png").unwrap();
        server.drain_events();
        assert_eq!(server.refs(handle), Some(2));

        assert!(!server.release(handle), "one reference is left");
        assert!(server.is_loaded(handle));
        assert!(server.release(handle), "the last one drops it");
        assert!(!server.is_loaded(handle));
        assert!(server.is_empty());
        assert!(
            !server.release(handle),
            "releasing a dropped handle is a no-op"
        );
        assert!(matches!(
            server.drain_events().as_slice(),
            [AssetEvent::Released { handle: released, .. }] if *released == handle
        ));
    }

    #[test]
    fn acquiring_a_path_that_is_not_loaded_fails() {
        let mut server = embedded();
        assert!(matches!(
            server.acquire("assets/hero.png"),
            Err(AssetError::NotFound { .. })
        ));
    }

    #[test]
    fn a_path_that_cannot_be_read_allocates_nothing() {
        let dir = TempDir::new("unreadable");
        let mut server = AssetServer::from_dir(dir.path());

        assert!(matches!(
            server.load_texture("assets/hero.png"),
            Err(AssetError::Io { .. })
        ));

        assert!(server.is_empty());
        assert!(!server.has_pending());
        assert_eq!(server.handle("assets/hero.png"), None);
    }

    #[test]
    fn a_missing_bundle_entry_is_not_found() {
        let mut server = embedded();
        let error = server.load_texture("assets/ghost.png").unwrap_err();
        assert!(
            matches!(error, AssetError::NotFound { ref path } if path == "assets/ghost.png"),
            "{error}"
        );
    }

    #[test]
    fn paths_that_leave_the_project_are_refused() {
        let mut server = embedded();
        for path in [
            "../secrets.png",
            "assets/../../secrets.png",
            "assets/..",
            "..",
            ".",
            "",
            "   ",
            "/etc/passwd",
            r"C:\Windows\System32\drivers\etc\hosts",
        ] {
            assert!(
                matches!(server.load(path), Err(AssetError::OutsideRoot { .. })),
                "{path:?} was accepted"
            );
        }
        assert!(server.is_empty());
    }

    /// The root is a bound, not a hint: a path that would escape it must not reach
    /// the filesystem at all.
    #[test]
    fn a_joined_path_is_checked_against_the_root() {
        let root = Path::new("/games/demo");
        assert_eq!(
            resolve(root, "assets/hero.png").as_deref(),
            Some(root.join("assets/hero.png").as_path())
        );
        assert!(resolve(root, "../other/hero.png").is_none());
        assert!(resolve(root, "C:/Windows/win.ini").is_none());
        assert!(resolve(root, "/etc/passwd").is_none());
    }

    #[test]
    fn handles_are_unique_and_never_reused() {
        let dir = TempDir::new("handles");
        dir.write("assets/hero.png", b"one");
        let mut server = AssetServer::from_dir(dir.path());

        let first = server.load_texture("assets/hero.png").unwrap();
        server.release(first.asset());
        dir.write("assets/other.png", b"two");
        let second = server.load_texture("assets/other.png").unwrap();

        assert_ne!(
            first.asset(),
            second.asset(),
            "a dropped handle must not name the next asset"
        );
        assert!(!server.is_loaded(first.asset()));
    }

    #[test]
    fn editing_a_file_re_reads_it_under_the_same_handle() {
        let dir = TempDir::new("reload");
        dir.write("assets/hero.png", b"first");
        let mut server = AssetServer::from_dir(dir.path());
        let hero = server.load_texture("assets/hero.png").unwrap();
        server.drain_pending();
        server.drain_events();

        dir.write("assets/hero.png", b"second");
        assert!(server.reload("assets/hero.png"));

        assert_eq!(hero.asset(), server.handle("assets/hero.png").unwrap());
        assert_eq!(server.version(hero.asset()), Some(2));
        assert_eq!(
            server.refs(hero.asset()),
            Some(1),
            "a reload takes no reference"
        );
        let pending = server.drain_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].bytes(), b"second");
        assert_eq!(pending[0].version, 2);
        assert!(matches!(
            server.drain_events().as_slice(),
            [AssetEvent::Reloaded { version: 2, .. }]
        ));
    }

    /// The reload seam is what the watcher drives, so it must ignore paths that
    /// are not loaded assets rather than importing whatever it is told about.
    #[test]
    fn reloading_something_unloaded_does_nothing() {
        let dir = TempDir::new("reload-unknown");
        dir.write("assets/hero.png", b"bytes");
        let mut server = AssetServer::from_dir(dir.path());
        server.load_texture("assets/hero.png").unwrap();
        server.drain_pending();
        server.drain_events();

        assert!(!server.reload("assets/other.png"));
        assert!(!server.reload("target/debug/demo.exe"));
        assert!(!server.reload("../outside.png"));
        assert!(!server.has_pending());
        assert!(server.drain_events().is_empty());
    }

    /// Saving a file by truncating it leaves a moment where it cannot be read. The
    /// loaded texture has to survive that rather than blanking.
    #[test]
    fn a_file_that_cannot_be_re_read_keeps_the_version_it_had() {
        let dir = TempDir::new("reload-missing");
        dir.write("assets/hero.png", b"pixels");
        let mut server = AssetServer::from_dir(dir.path());
        let hero = server.load_texture("assets/hero.png").unwrap();
        server.drain_pending();
        server.drain_events();

        dir.remove("assets/hero.png");
        assert!(server.reload("assets/hero.png"), "it was a loaded asset");

        assert_eq!(server.version(hero.asset()), Some(1));
        assert!(!server.has_pending());
        assert!(matches!(
            server.drain_events().as_slice(),
            [AssetEvent::Failed { .. }]
        ));
        assert!(server.is_loaded(handle_of(&server, "assets/hero.png")));
    }

    /// The handle a path still names, so an assertion can be about the handle
    /// rather than about the one the test happens to have kept.
    fn handle_of(server: &AssetServer, path: &str) -> AssetHandle {
        server.handle(path).expect("the asset is still loaded")
    }

    #[test]
    fn events_run_from_load_through_reload_to_release() {
        let dir = TempDir::new("event-order");
        dir.write("assets/hero.png", b"one");
        let mut server = AssetServer::from_dir(dir.path());
        let hero = server.load_texture("assets/hero.png").unwrap();
        dir.write("assets/hero.png", b"two");
        server.reload("assets/hero.png");
        dir.remove("assets/hero.png");
        server.reload("assets/hero.png");
        server.release(hero.asset());

        let names: Vec<&str> = server.drain_events().iter().map(AssetEvent::name).collect();
        assert_eq!(names, ["loaded", "reloaded", "failed", "released"]);
        assert!(
            server.drain_events().is_empty(),
            "events are taken, not copied"
        );
    }

    #[test]
    fn the_registry_is_listed_by_path() {
        let dir = TempDir::new("listing");
        dir.write("assets/b.png", b"b");
        dir.write("assets/a.png", b"a");
        let mut server = AssetServer::from_dir(dir.path());
        server.load_texture("assets/b.png").unwrap();
        server.load_texture("assets/a.png").unwrap();

        let listed: Vec<_> = server
            .loaded()
            .map(|asset| (asset.path, asset.kind, asset.version, asset.refs))
            .collect();

        assert_eq!(
            listed,
            vec![
                ("assets/a.png", AssetKind::Texture, 1, 1),
                ("assets/b.png", AssetKind::Texture, 1, 1),
            ]
        );
        assert_eq!(server.root(), Some(dir.path()));
        assert!(embedded().root().is_none());
    }

    #[test]
    fn handles_and_kinds_say_what_they_are() {
        assert_eq!(AssetKind::Texture.to_string(), "texture");
        assert_eq!(AssetKind::Font.to_string(), "font");
        assert_eq!(AssetKind::Bytes.to_string(), "bytes");
        assert_eq!(
            TextureHandle::new(NonZeroU64::new(7).unwrap()).to_string(),
            "texture#7"
        );
        assert_eq!(
            FontHandle::new(NonZeroU64::new(8).unwrap()).to_string(),
            "font#8"
        );
        assert_eq!(AssetHandle::from(NonZeroU64::new(9).unwrap()).id(), 9);
    }

    #[test]
    fn errors_are_readable_and_carry_their_source() {
        let io = AssetError::Io {
            path: "assets/hero.png".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        };
        assert_eq!(
            io.to_string(),
            "`assets/hero.png` could not be read: no such file"
        );
        assert!(std::error::Error::source(&io).is_some());
        assert!(
            std::error::Error::source(&AssetError::NotFound {
                path: "a".to_string()
            })
            .is_none()
        );
        assert!(
            AssetError::WrongKind {
                path: "a.png".to_string(),
                wanted: AssetKind::Texture,
                loaded: AssetKind::Bytes,
            }
            .to_string()
            .contains("already loaded as bytes")
        );
        assert!(
            AssetError::OutsideRoot {
                path: "..".to_string()
            }
            .to_string()
            .contains("not inside the project")
        );
    }

    #[test]
    fn a_server_without_a_watcher_pumps_to_nothing() {
        let dir = TempDir::new("no-watch");
        dir.write("assets/hero.png", b"pixels");
        let mut server = AssetServer::from_dir(dir.path());
        server.load_texture("assets/hero.png").unwrap();
        server.drain_pending();

        dir.write("assets/hero.png", b"edited");
        assert_eq!(server.pump(), 0, "nothing watched, nothing re-read");
        assert!(!server.is_watching());
    }

    /// The point of the feature: a watched server picks up an edit with nothing
    /// asked of the game but a pump each frame.
    #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
    #[test]
    fn a_watched_server_re_reads_an_edited_file() {
        use std::time::{Duration, Instant};

        let dir = TempDir::new("watched");
        dir.write("assets/hero.png", b"before");
        let mut server = AssetServer::watching(dir.path()).expect("start watching");
        let hero = server.load_texture("assets/hero.png").unwrap();
        assert!(server.is_watching());
        server.drain_pending();
        server.drain_events();

        dir.write("assets/hero.png", b"after");

        // Filesystem notifications are asynchronous, so the test waits for one
        // rather than assuming an edit is visible immediately.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut reloaded = false;
        while Instant::now() < deadline {
            if server.pump() > 0 {
                reloaded = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(reloaded, "the watcher never reported the edit");

        assert_eq!(server.version(hero.asset()), Some(2));
        let pending = server.drain_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].bytes(), b"after");
        assert!(matches!(
            server.drain_events().as_slice(),
            [AssetEvent::Reloaded { version: 2, .. }]
        ));
    }

    /// Only the directories assets are loaded from are watched, so a build
    /// directory churning next to them costs nothing.
    #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
    #[test]
    fn a_watched_server_ignores_what_it_was_not_told_to_watch() {
        use std::time::{Duration, Instant};

        let dir = TempDir::new("watch-scope");
        dir.write("assets/hero.png", b"before");
        dir.write("target/debug/churn.bin", b"build output");
        let mut server = AssetServer::watching(dir.path()).expect("start watching");
        server.load_texture("assets/hero.png").unwrap();
        server.drain_pending();
        server.drain_events();

        std::fs::write(
            dir.path().join("target/debug/churn.bin"),
            b"more build output",
        )
        .expect("rewrite the build output");

        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            server.pump();
            std::thread::sleep(Duration::from_millis(20));
        }

        assert!(!server.has_pending(), "an untracked file must not import");
        assert!(server.drain_events().is_empty());
    }
}
