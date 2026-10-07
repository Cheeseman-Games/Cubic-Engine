//! Opening a file from the project tree: what the viewport previews.
//!
//! The kind comes from [`crate::project::classify`] (an extension decision);
//! this module does the work — decoding an image into an egui texture, or
//! reading text under a cap — and turns every failure into an error preview
//! rather than a dead click.

use std::path::{Path, PathBuf};

use crate::project::OpenKind;

/// Longest image side kept, in pixels; larger previews are shrunk to fit so a
/// big backdrop neither overflows the GPU limit nor floods memory.
const MAX_IMAGE_SIDE: u32 = 2048;

/// Bytes of a text file read into the preview.
const MAX_TEXT_BYTES: usize = 512 * 1024;

/// Lines of a text file shown; the rest is cut and the preview says so.
const MAX_TEXT_LINES: usize = 4000;

/// What the viewport shows for an opened file.
#[derive(Clone)]
pub enum PreviewKind {
    /// A decoded image, uploaded as an egui texture.
    Image {
        /// The GPU texture to paint.
        texture: egui::TextureHandle,
        /// Its size in pixels, for the header readout.
        size: [usize; 2],
    },
    /// A file shown as read-only text.
    Text {
        /// The (possibly truncated) contents.
        text: String,
        /// Whether `text` is not the whole file.
        truncated: bool,
    },
    /// The file could not be previewed; the message says why.
    Error(String),
}

/// An opened file awaiting display in the viewport.
#[derive(Clone)]
pub struct Preview {
    /// The file it was read from.
    pub path: PathBuf,
    /// The decoded contents, or the reason it failed.
    pub kind: PreviewKind,
}

impl Preview {
    /// Whether only part of the file is shown (text over its caps).
    pub fn truncated(&self) -> bool {
        matches!(
            &self.kind,
            PreviewKind::Text {
                truncated: true,
                ..
            }
        )
    }
}

/// Debug without the texture: a GPU handle's id means nothing in a log line.
impl std::fmt::Debug for PreviewKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Image { size, .. } => f.debug_struct("Image").field("size", size).finish(),
            Self::Text { truncated, .. } => f
                .debug_struct("Text")
                .field("truncated", truncated)
                .finish(),
            Self::Error(message) => f.debug_tuple("Error").field(message).finish(),
        }
    }
}

/// Reads `path` for display, never failing: errors become previews.
///
/// `ctx` is where an image texture is registered; egui frees it when the
/// preview is dropped or replaced.
pub fn load(path: &Path, kind: OpenKind, ctx: &egui::Context) -> Preview {
    let loaded = match kind {
        OpenKind::Image => load_image(path, ctx),
        OpenKind::Text => load_text(path),
        // Scenes are not deserialized until the scene model lands; opening one
        // is handled by the caller, which only records the path.
        OpenKind::Scene => Err("scenes open as the active scene, not a preview".to_owned()),
    };
    Preview {
        path: path.to_path_buf(),
        kind: loaded.unwrap_or_else(PreviewKind::Error),
    }
}

/// Decodes an image and uploads it, shrinking anything past [`MAX_IMAGE_SIDE`].
fn load_image(path: &Path, ctx: &egui::Context) -> Result<PreviewKind, String> {
    let name = file_name(path);
    let bytes = std::fs::read(path).map_err(|error| format!("{name}: {error}"))?;
    let mut image = image::load_from_memory(&bytes).map_err(|error| format!("{name}: {error}"))?;
    if image.width() > MAX_IMAGE_SIDE || image.height() > MAX_IMAGE_SIDE {
        image = image.thumbnail(MAX_IMAGE_SIDE, MAX_IMAGE_SIDE);
    }
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let texture = ctx.load_texture(name, color, egui::TextureOptions::LINEAR);
    Ok(PreviewKind::Image { texture, size })
}

/// Reads a text file, capping it at [`MAX_TEXT_BYTES`] and [`MAX_TEXT_LINES`].
///
/// Invalid UTF-8 survives as replacement characters: an extension that says
/// "text" gets text, and the cap flags that there is more.
fn load_text(path: &Path) -> Result<PreviewKind, String> {
    let name = file_name(path);
    let mut bytes = std::fs::read(path).map_err(|error| format!("{name}: {error}"))?;
    let mut truncated = bytes.len() > MAX_TEXT_BYTES;
    if truncated {
        bytes.truncate(MAX_TEXT_BYTES);
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.lines().count() > MAX_TEXT_LINES {
        truncated = true;
        text = text
            .lines()
            .take(MAX_TEXT_LINES)
            .collect::<Vec<_>>()
            .join("\n");
    }
    Ok(PreviewKind::Text { text, truncated })
}

/// The file's name, for messages that should name what went wrong.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(preview: &Preview) -> (&str, bool) {
        match &preview.kind {
            PreviewKind::Text { text, truncated } => (text.as_str(), *truncated),
            other => panic!("expected a text preview, got {:?}", kind_name(other)),
        }
    }

    fn kind_name(kind: &PreviewKind) -> &'static str {
        match kind {
            PreviewKind::Image { .. } => "image",
            PreviewKind::Text { .. } => "text",
            PreviewKind::Error(_) => "error",
        }
    }

    #[test]
    fn a_short_text_file_opens_whole() {
        let scratch = crate::scratch::Scratch::new("preview-text");
        let path = scratch.join("notes.txt");
        std::fs::write(&path, "two\nlines\n").expect("write");

        let preview = load(&path, OpenKind::Text, &egui::Context::default());
        assert_eq!(text_of(&preview), ("two\nlines\n", false));
    }

    #[test]
    fn a_long_text_file_is_capped_and_says_so() {
        let scratch = crate::scratch::Scratch::new("preview-cap");
        let path = scratch.join("big.txt");
        let line = "0123456789\n";
        let body: String = line.repeat(MAX_TEXT_BYTES / line.len() + 64);
        std::fs::write(&path, &body).expect("write");

        let context = egui::Context::default();
        let preview = load(&path, OpenKind::Text, &context);
        let (text, truncated) = text_of(&preview);
        assert!(truncated);
        assert!(text.len() <= MAX_TEXT_BYTES);
    }

    #[test]
    fn a_file_with_too_many_lines_is_capped() {
        let scratch = crate::scratch::Scratch::new("preview-lines");
        let path = scratch.join("many.txt");
        let body: String = (0..MAX_TEXT_LINES + 10)
            .map(|index| format!("line {index}\n"))
            .collect();
        std::fs::write(&path, body).expect("write");

        let context = egui::Context::default();
        let preview = load(&path, OpenKind::Text, &context);
        let (text, truncated) = text_of(&preview);
        assert!(truncated);
        assert_eq!(text.lines().count(), MAX_TEXT_LINES);
    }

    #[test]
    fn an_unreadable_file_is_an_error_preview() {
        let scratch = crate::scratch::Scratch::new("preview-missing");
        let preview = load(
            &scratch.join("gone.txt"),
            OpenKind::Text,
            &egui::Context::default(),
        );
        match &preview.kind {
            PreviewKind::Error(message) => assert!(message.contains("gone.txt"), "{message}"),
            other => panic!("expected an error, got {:?}", kind_name(other)),
        }
    }

    #[test]
    fn a_png_decodes_into_a_texture() {
        let scratch = crate::scratch::Scratch::new("preview-image");
        let path = scratch.join("sprite.png");
        let pixels = image::RgbaImage::from_pixel(8, 4, image::Rgba([0x40, 0x80, 0xc0, 0xff]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .expect("encode png");
        std::fs::write(&path, encoded.into_inner()).expect("write png");

        let preview = load(&path, OpenKind::Image, &egui::Context::default());
        match &preview.kind {
            PreviewKind::Image { size, .. } => assert_eq!(*size, [8, 4]),
            other => panic!("expected an image, got {:?}", kind_name(other)),
        }
    }
}
