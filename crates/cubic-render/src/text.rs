//! Glyph-atlas text rendering for the wgpu backend.
//!
//! [`TextPipeline`] turns the `DrawCommand::Text` spans of a frame
//! (`cubic_core::render`, the same backend-agnostic command model the wasm canvas
//! and headless runs consume) into textured glyph quads. Shaping, rasterization
//! and the atlas itself come from `glyphon`; this module owns the parts that
//! belong to the engine:
//!
//! - the mapping from one text command to one laid-out string,
//! - the **per-size layout cache** ([`TextPipeline::draw`] re-uses one layout
//!   buffer per span position, re-shaping only when the string, the size, or the
//!   family actually changed), and
//! - the coordinate/color conventions, so game code gets the same result on the
//!   wgpu and canvas backends.
//!
//! Glyph *pixels* are cached by `glyphon` itself, keyed by font face, pixel size
//! and subpixel position — so the same glyph at a new size is rasterized once and
//! then served from the atlas.
//!
//! Conventions (they match [`crate::render2d`], which works in physical pixels):
//! - `x`, `y` are the top-left of the text's line box in physical pixels, and
//!   `size` is both the font size and the line height.
//! - `Rgba` is converted to 8-bit per channel, so text and rects shade alike.

use std::path::{Path, PathBuf};

use cubic_core::render::Rgba;
use glyphon::{
    Attrs, Buffer, Color, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport, fontdb,
};

pub use glyphon::{Family, FamilyOwned};

/// Environment variable naming a font file, or a directory of them, to add to the
/// font database on top of the system fonts. Useful on machines whose system
/// fonts are missing or unusable (containers, minimal CI images) — nothing is
/// bundled with the engine.
pub const FONT_PATH_ENV: &str = "CUBIC_FONT";

/// Line height as a multiple of the font size, so a text command's `y` is the top
/// of a line box exactly `size` tall.
const LINE_HEIGHT_SCALE: f32 = 1.0;

/// Font size a layout buffer is created with before it is shaped for real text.
const PLACEHOLDER_SIZE: f32 = 16.0;

/// The file extensions a font *directory* is scanned for.
const FONT_EXTENSIONS: [&str; 4] = ["ttf", "otf", "ttc", "otc"];

/// One `DrawCommand::Text` resolved against the font pipeline, borrowed out of the
/// frame's `DrawList`. The rect stream (`render2d::QuadBatch`) never owns the
/// string, so building the spans costs no allocations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextSpan<'a> {
    pub text: &'a str,
    /// Left edge of the line box, in physical pixels.
    pub x: f32,
    /// Top edge of the line box, in physical pixels.
    pub y: f32,
    /// Font size (and line height), in physical pixels.
    pub size: f32,
    pub color: Rgba,
}

/// Why a frame's text could not be drawn. None of these are configuration
/// errors, so the caller gets a `log` line rather than a panic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextError {
    /// The glyph atlas filled up with glyphs that are all still in use.
    AtlasFull,
    /// A glyph was evicted from the atlas while the frame was being recorded,
    /// because the frame overflowed the atlas mid-pass.
    GlyphEvicted,
    /// The render target changed size after the frame's layout was prepared.
    ResolutionChanged,
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AtlasFull => write!(
                f,
                "the glyph atlas is full: a single frame needs more distinct glyphs than it can hold"
            ),
            Self::GlyphEvicted => write!(
                f,
                "a glyph was evicted from the atlas mid-frame: the frame overflows the atlas"
            ),
            Self::ResolutionChanged => write!(
                f,
                "the render target was resized after the frame's text was laid out"
            ),
        }
    }
}

impl std::error::Error for TextError {}

impl From<glyphon::PrepareError> for TextError {
    fn from(error: glyphon::PrepareError) -> Self {
        match error {
            glyphon::PrepareError::AtlasFull => Self::AtlasFull,
        }
    }
}

impl From<glyphon::RenderError> for TextError {
    fn from(error: glyphon::RenderError) -> Self {
        match error {
            glyphon::RenderError::RemovedFromAtlas => Self::GlyphEvicted,
            glyphon::RenderError::ScreenResolutionChanged => Self::ResolutionChanged,
        }
    }
}

/// Text rendering over wgpu: font database, glyph atlas, and the layout cache.
///
/// The pipeline is sized to the render target, so create it with the same format
/// the 2D renderer draws into ([`TextPipeline::new`]) and keep
/// [`set_resolution`](Self::set_resolution) in step with the surface size.
pub struct TextPipeline {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    /// Rasterizes a frame's glyphs without ever drawing them, so the atlas is
    /// already at its final size before the first draw of the frame. See
    /// [`prepare_frame`](Self::prepare_frame) for why that matters.
    warmup: TextRenderer,
    /// One glyph-vertex buffer per text run, drawn in order. Runs need separate
    /// buffers because a `write_buffer` only lands when the frame is submitted:
    /// sharing one buffer across runs would make every run's draw read the last
    /// run's vertices.
    runners: Vec<TextRenderer>,
    /// How many runs have been drawn this frame, i.e. the next runner's index.
    next_run: usize,
    /// Family every span is shaped with. Monospace by default, matching the
    /// canvas backend's hardcoded `monospace`.
    family: FamilyOwned,
    /// Clip rectangle for glyphs; the default never clips beyond the target.
    bounds: TextBounds,
    /// Layout buffers, indexed by span position within a frame.
    slots: Vec<Slot>,
    /// How many spans the current frame's [`prepare_frame`](Self::prepare_frame)
    /// covered, so a run that indexes past them is caught instead of silently
    /// overwriting another run's layout.
    frame_len: usize,
}

impl TextPipeline {
    /// Create a pipeline backed by the system font database, plus whatever
    /// [`FONT_PATH_ENV`] points at.
    ///
    /// This loads the system fonts, which takes a noticeable moment on first
    /// call — create the pipeline once and keep it.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self::with_font_system(device, queue, format, system_font_system())
    }

    /// Create a pipeline with a caller-built font database, for games that ship
    /// their own fonts instead of the system ones.
    pub fn with_font_system(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        font_system: FontSystem,
    ) -> Self {
        let swash_cache = SwashCache::new();
        let cache = glyphon::Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let warmup = TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        Self {
            font_system,
            swash_cache,
            viewport,
            atlas,
            warmup,
            runners: Vec::new(),
            next_run: 0,
            family: FamilyOwned::Monospace,
            bounds: TextBounds::default(),
            slots: Vec::new(),
            frame_len: 0,
        }
    }

    /// Tell the pipeline how big the render target is, in physical pixels. Glyph
    /// positions are subpixel-binned against the resolution, so a size change
    /// invalidates the layout prepared for the previous one. A no-op when the
    /// resolution is unchanged, so it is cheap to call every frame.
    pub fn set_resolution(&mut self, queue: &wgpu::Queue, width: u32, height: u32) {
        self.viewport.update(queue, Resolution { width, height });
    }

    /// Shape all spans with `family` from the next frame on.
    pub fn set_family(&mut self, family: FamilyOwned) {
        self.family = family;
    }

    /// The font database, to register game-supplied fonts on.
    pub fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    /// Register font file bytes and name the family they define.
    ///
    /// This is the font half of the asset pipeline: the bytes come from an
    /// [`AssetServer`](cubic_core::assets::AssetServer) font asset, and the family
    /// name that comes back is what [`set_family`](Self::set_family) takes — so
    /// the same file can be reloaded without the game re-plumbing anything.
    ///
    /// Returns `None` if the bytes are not a font the database can read, leaving it
    /// untouched: a game then keeps the family it had rather than falling back to
    /// one it never asked for. `path` is only in the log line.
    pub fn add_font(&mut self, path: &str, bytes: &[u8]) -> Option<String> {
        // `load_font_source` rather than `load_font_data`, because only the former
        // says which faces arrived — and naming one of those faces is the whole
        // point of registering a font.
        let ids = self
            .font_system
            .db_mut()
            .load_font_source(fontdb::Source::Binary(std::sync::Arc::new(bytes.to_vec())));
        let db = self.font_system.db();
        let family = ids
            .first()
            .and_then(|id| db.face(*id))
            .and_then(|face| face.families.first())
            .map(|(name, _)| name.clone());
        match family {
            Some(family) => {
                log::info!("registered `{path}` as `{family}`");
                Some(family)
            }
            None => {
                log::error!("`{path}` defined no font family");
                None
            }
        }
    }

    /// Size the glyph atlas for a whole frame.
    ///
    /// Call this once per frame, with every text span the frame will draw, before
    /// the first [`draw`](Self::draw). Glyphon grows the atlas *inside* `prepare`,
    /// which replaces its texture — and wgpu invalidates recorded commands that
    /// reference a destroyed resource. A frame interleaves text with rects, so
    /// each run is prepared and drawn in turn; preparing the whole frame first
    /// grows the atlas to its final size, and no run can then invalidate the runs
    /// already recorded into the pass.
    ///
    /// This pass's glyph vertices are thrown away: the next `prepare` (the first
    /// run's) clears them, and each run only ever records its own glyphs.
    pub fn prepare_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        spans: &[TextSpan<'_>],
    ) -> Result<(), TextError> {
        self.frame_len = spans.len();
        self.next_run = 0;
        self.prepare(GlyphTarget::Warmup, device, queue, spans, 0)
    }

    /// Lay out one run of spans and draw it into an in-progress render pass.
    ///
    /// `start` is where `spans` begins in the frame's span list, which is also the
    /// run's index into the layout cache: the frame's `n`th text command always
    /// reuses the `n`th slot, whichever run it lands in. Call this once per run,
    /// in command order, so the pass records glyph quads where the run is
    /// recorded — that is what interleaves text with rects.
    /// [`prepare_frame`](Self::prepare_frame) must have run for the frame first.
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        start: usize,
        spans: &[TextSpan<'_>],
    ) -> Result<(), TextError> {
        if spans.is_empty() {
            return Ok(());
        }
        let run = self.next_run;
        self.next_run += 1;
        self.prepare(GlyphTarget::Run(run), device, queue, spans, start)?;
        let renderer = &self.runners[run];
        renderer.render(&self.atlas, &self.viewport, pass)?;
        Ok(())
    }

    /// Shape `spans` — the frame's spans `start..start + spans.len()` — into the
    /// layout cache, then hand them to `target` for rasterization.
    ///
    /// Slots are indexed by span position, so a frame that emits a stable set of
    /// labels re-shapes only the ones whose text, size or family changed.
    fn prepare(
        &mut self,
        target: GlyphTarget,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        spans: &[TextSpan<'_>],
        start: usize,
    ) -> Result<(), TextError> {
        if spans.is_empty() {
            return Ok(());
        }
        let end = start + spans.len();
        self.grow_slots(end);
        debug_assert!(
            end <= self.frame_len,
            "a run reaches past the spans `prepare_frame` was given for this frame"
        );

        let Self {
            font_system,
            swash_cache,
            viewport,
            atlas,
            warmup,
            runners,
            family,
            bounds,
            slots,
            ..
        } = self;
        let areas: Vec<TextArea<'_>> = spans
            .iter()
            .zip(&mut slots[start..end])
            .map(|(span, slot)| {
                slot.sync(font_system, span, family);
                TextArea {
                    buffer: &slot.buffer,
                    left: span.x,
                    top: span.y,
                    scale: 1.0,
                    bounds: *bounds,
                    default_color: color(span.color),
                    custom_glyphs: &[],
                }
            })
            .collect();

        // Every runner shares the atlas's cached pipeline, so growing the pool
        // costs a 4 KiB vertex buffer per run and nothing else.
        if let GlyphTarget::Run(run) = target
            && runners.len() <= run
        {
            runners.resize_with(run + 1, || {
                TextRenderer::new(atlas, device, wgpu::MultisampleState::default(), None)
            });
        }
        let runner = match target {
            GlyphTarget::Warmup => &mut *warmup,
            GlyphTarget::Run(run) => &mut runners[run],
        };
        runner.prepare(
            device,
            queue,
            font_system,
            atlas,
            viewport,
            areas,
            swash_cache,
        )?;
        Ok(())
    }

    /// Release atlas space held by glyphs that are no longer in use. Call once
    /// per frame, after the commands that read the atlas have been submitted.
    pub fn trim(&mut self) {
        self.atlas.trim();
    }

    /// Keep at least `needed` layout buffers. Slots are reused across frames by
    /// index, so a game that emits a stable set of labels never re-shapes them.
    fn grow_slots(&mut self, needed: usize) {
        if self.slots.len() >= needed {
            return;
        }
        let capacity = needed.next_power_of_two();
        for _ in self.slots.len()..capacity {
            self.slots
                .push(Slot::new(&mut self.font_system, PLACEHOLDER_SIZE));
        }
    }
}

/// Which glyph-vertex buffer a prepared run is rasterized into.
#[derive(Clone, Copy)]
enum GlyphTarget {
    /// The throwaway buffer behind [`TextPipeline::prepare_frame`].
    Warmup,
    /// The buffer owned by the frame's `n`th text run, which that run then draws.
    Run(usize),
}

/// One reusable layout buffer plus the input it was last shaped for.
struct Slot {
    buffer: Buffer,
    shaped: Option<Shaped>,
    /// How many times this slot has been (re)shaped. Only meaningful to tests.
    reshapes: u32,
}

/// The input a [`Slot`] is currently shaped for.
///
/// `cosmic-text` has no incremental shaping mode here — every `set_text`
/// re-shapes the whole string — so the layout cache only pays off by skipping
/// `set_text` altogether when nothing changed. Hence the whole shaping input, not
/// just the string, is what gets compared. Size is stored as its `f32` bit
/// pattern so a size change is always noticed, including `-0.0` and `NaN`.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Shaped {
    text: Box<str>,
    size_bits: u32,
    family: FamilyOwned,
}

impl Shaped {
    fn matches(&self, text: &str, size_bits: u32, family: &FamilyOwned) -> bool {
        self.size_bits == size_bits && self.family == *family && self.text.as_ref() == text
    }
}

impl Slot {
    fn new(font_system: &mut FontSystem, size: f32) -> Self {
        Self {
            buffer: Buffer::new(font_system, metrics(size)),
            shaped: None,
            reshapes: 0,
        }
    }

    /// Bring the layout buffer in line with `span`, re-shaping only when the
    /// string, size, or family differs from the last shape.
    fn sync(&mut self, font_system: &mut FontSystem, span: &TextSpan<'_>, family: &FamilyOwned) {
        let size_bits = span.size.to_bits();
        if self
            .shaped
            .as_ref()
            .is_some_and(|shaped| shaped.matches(span.text, size_bits, family))
        {
            return;
        }
        self.buffer.set_metrics(metrics(span.size));
        self.buffer
            .set_text(span.text, &attrs(family), Shaping::Advanced, None);
        self.buffer.shape_until_scroll(font_system, false);
        self.shaped = Some(Shaped {
            text: span.text.into(),
            size_bits,
            family: family.clone(),
        });
        self.reshapes += 1;
    }
}

/// Line box for a span: the font size, with the line height pinned to it.
fn metrics(size: f32) -> Metrics {
    Metrics::relative(size, LINE_HEIGHT_SCALE)
}

fn attrs(family: &FamilyOwned) -> Attrs<'_> {
    Attrs::new().family(family.as_family())
}

/// `cubic-core`'s normalized color to the 8-bit channels the glyph atlas shades
/// with.
fn color(rgba: Rgba) -> Color {
    Color::rgba(
        channel(rgba.r),
        channel(rgba.g),
        channel(rgba.b),
        channel(rgba.a),
    )
}

fn channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The system font database, extended with [`FONT_PATH_ENV`] when it is set.
fn system_font_system() -> FontSystem {
    match std::env::var_os(FONT_PATH_ENV) {
        Some(path) => {
            let sources = font_sources(PathBuf::from(path));
            log::info!("{FONT_PATH_ENV} contributed {} font file(s)", sources.len());
            FontSystem::new_with_fonts(sources)
        }
        None => FontSystem::new(),
    }
}

/// The font files named by `path`: itself when it is a file, or every font inside
/// it when it is a directory (sorted, so face selection is reproducible).
fn font_sources(path: PathBuf) -> Vec<fontdb::Source> {
    if !path.is_dir() {
        return vec![fontdb::Source::File(path)];
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_font_file(path))
        .collect();
    files.sort();
    files.into_iter().map(fontdb::Source::File).collect()
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| FONT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span<'a>(text: &'a str, size: f32) -> TextSpan<'a> {
        TextSpan {
            text,
            x: 0.0,
            y: 0.0,
            size,
            color: Rgba::rgb(1.0, 1.0, 1.0),
        }
    }

    /// The font database slots are shaped against.
    ///
    /// `FontSystem` always loads the system fonts — there is no way to make an
    /// empty one — which is also why [`TextPipeline::new`] builds it lazily: the
    /// load is the pipeline's startup cost.
    fn empty_font_system() -> FontSystem {
        FontSystem::new_with_fonts([])
    }

    #[test]
    fn line_box_is_one_font_size_tall() {
        assert_eq!(metrics(12.0), Metrics::new(12.0, 12.0));
        assert_eq!(metrics(31.5), Metrics::new(31.5, 31.5));
    }

    #[test]
    fn color_rounds_and_clamps_channels() {
        assert_eq!(
            color(Rgba::new(1.0, 0.0, 0.5, 0.25)).as_rgba(),
            [255, 0, 128, 64]
        );
        // Out-of-range and non-finite components must not wrap around.
        assert_eq!(
            color(Rgba::new(-1.0, 2.0, f32::NAN, 1.0)).as_rgba(),
            [0, 255, 0, 255]
        );
    }

    #[test]
    fn shaping_key_matches_only_identical_input() {
        let mono = FamilyOwned::Monospace;
        let key = Shaped {
            text: "hp".into(),
            size_bits: 16.0f32.to_bits(),
            family: mono.clone(),
        };

        assert!(key.matches("hp", 16.0f32.to_bits(), &mono));
        assert!(
            !key.matches("hp", 17.0f32.to_bits(), &mono),
            "size is part of the key"
        );
        assert!(
            !key.matches("hpp", 16.0f32.to_bits(), &mono),
            "so is the text"
        );
        assert!(
            !key.matches("hp", 16.0f32.to_bits(), &FamilyOwned::SansSerif),
            "so is the family"
        );
    }

    #[test]
    fn slots_reshape_only_when_their_input_changes() {
        // One font database for the whole test: `FontSystem` always loads the
        // system fonts, so building one is the expensive part.
        let mut font_system = empty_font_system();
        let mut slot = Slot::new(&mut font_system, PLACEHOLDER_SIZE);
        let mono = FamilyOwned::Monospace;

        slot.sync(&mut font_system, &span("hello", 16.0), &mono);
        assert_eq!(slot.reshapes, 1, "a fresh slot always shapes once");

        slot.sync(&mut font_system, &span("hello", 16.0), &mono);
        slot.sync(&mut font_system, &span("hello", 16.0), &mono);
        assert_eq!(slot.reshapes, 1, "an unchanged span keeps its layout");

        slot.sync(&mut font_system, &span("hellp", 16.0), &mono);
        assert_eq!(slot.reshapes, 2, "changed text re-shapes");

        slot.sync(
            &mut font_system,
            &span("hellp", 16.0),
            &FamilyOwned::SansSerif,
        );
        assert_eq!(slot.reshapes, 3, "changed family re-shapes");

        slot.sync(
            &mut font_system,
            &span("hellp", 32.0),
            &FamilyOwned::SansSerif,
        );
        assert_eq!(slot.reshapes, 4, "changed size re-shapes");

        // Position and color are applied per frame by the draw call, not baked
        // into the layout, so moving or recoloring a label must not re-shape it.
        let mut moved = span("hellp", 32.0);
        moved.x = 120.0;
        moved.color = Rgba::rgb(0.2, 0.4, 0.6);
        slot.sync(&mut font_system, &moved, &FamilyOwned::SansSerif);
        assert_eq!(slot.reshapes, 4);
    }

    #[test]
    fn font_path_env_names_a_file_or_scans_a_directory() {
        let missing = std::env::temp_dir().join("cubic-no-such-font.ttf");
        let sources = font_sources(missing.clone());
        assert!(matches!(sources.as_slice(), [fontdb::Source::File(path)] if *path == missing));

        let dir = std::env::temp_dir().join("cubic-font-dir-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp font dir");
        std::fs::write(dir.join("b.ttf"), b"not really a font").expect("fake font");
        std::fs::write(dir.join("a.OTF"), b"not really a font either").expect("fake font");
        std::fs::write(dir.join("notes.txt"), b"ignored").expect("decoy");

        let files: Vec<PathBuf> = font_sources(dir.clone())
            .into_iter()
            .map(|source| match source {
                fontdb::Source::File(path) => path,
                other => panic!("expected a file source, got {other:?}"),
            })
            .collect();
        assert_eq!(
            files,
            vec![dir.join("a.OTF"), dir.join("b.ttf")],
            "only font files, in a stable order"
        );

        std::fs::remove_dir_all(&dir).expect("clean up temp font dir");
    }

    #[test]
    fn font_file_extensions_are_matched_case_insensitively() {
        for name in ["a.ttf", "a.TTF", "a.otf", "a.ttc", "a.otc"] {
            assert!(is_font_file(Path::new(name)), "{name} should be a font");
        }
        for name in ["a.txt", "a.png", "a", "a.ttf.bak"] {
            assert!(!is_font_file(Path::new(name)), "{name} should be skipped");
        }
    }
}
