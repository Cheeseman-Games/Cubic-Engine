//! Thumbnails for the asset panel: a small decode of each texture, cached.
//!
//! The panel lists every texture a project holds, and a decode per frame would
//! be wasteful for something that only changes when the project does. Each
//! thumbnail is decoded once, on first sight, and kept until the project
//! changes; a file that cannot be decoded is remembered as `None` so a broken
//! image is not retried every frame.

use std::collections::BTreeMap;
use std::path::Path;

/// The longest side of a thumbnail, in pixels.
const SIZE: u32 = 96;

/// Small previews of a project's textures, decoded on demand.
#[derive(Default)]
pub struct Thumbnails {
    cache: BTreeMap<String, Option<egui::TextureHandle>>,
}

impl Thumbnails {
    /// Drops every cached thumbnail — for when the open project changes.
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// The thumbnail for the project-relative `path`, decoding it on first
    /// sight. `None` means the file could not be read or decoded.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        root: &Path,
        path: &str,
    ) -> Option<&egui::TextureHandle> {
        self.cache
            .entry(path.to_owned())
            .or_insert_with(|| decode(ctx, root, path))
            .as_ref()
    }
}

/// Decodes one image into a thumbnail texture, or `None` if it cannot be.
fn decode(ctx: &egui::Context, root: &Path, path: &str) -> Option<egui::TextureHandle> {
    let bytes = std::fs::read(root.join(path)).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let thumb = image.thumbnail(SIZE, SIZE).to_rgba8();
    let size = [thumb.width() as usize, thumb.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, thumb.as_raw());
    Some(ctx.load_texture(
        format!("thumbnail:{path}"),
        color,
        egui::TextureOptions::LINEAR,
    ))
}
