//! An offscreen target for hosts that render the engine's commands into their
//! own UI.
//!
//! The window shell ([`crate::app`]) draws straight into its swapchain. A host
//! that embeds the scene in a larger immediate-mode UI — the editor does this
//! through egui — instead needs the same batch drawn into a texture that UI can
//! sample. [`Offscreen2d`] is that target: the quad renderer, the texture store
//! and the glyph pipeline over a device and queue the host already owns, aimed
//! at a texture the host registers with its own compositor.
//!
//! The target format is [`FORMAT`] (`Rgba8Unorm`, gamma-space). That is the
//! format egui requires for registered textures, and it keeps the pipeline's
//! straight-through color writes byte-exact: the bytes in the texture are the
//! `Rgba` values the `DrawList` carries, so what the host samples is what the
//! commands asked for.

use cubic_core::render::{DrawList, Rgba};

use crate::assets::TextureStore;
use crate::render2d::{QuadBatch, WgpuRenderer2d};
use crate::text::{TextError, TextPipeline};

/// The offscreen target's texture format: gamma-space `Rgba8Unorm`.
///
/// Hosts that register the texture with egui-wgpu get the format it documents
/// as required; hosts that read it back (tests) get the bytes the commands
/// wrote, with no sRGB encode in between.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The 2D renderer pointed at a texture of its own, sized by the caller.
///
/// Everything the engine needs for one offscreen frame lives here so a host can
/// treat it as one thing: [`render`](Self::render) takes a [`DrawList`] and
/// leaves the frame's pixels in [`texture`](Self::texture). The device and
/// queue are borrowed-then-cloned at construction, the same way
/// [`WgpuRenderer2d`] holds them — a host such as eframe owns the device, and
/// several targets may share it.
pub struct Offscreen2d {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Owns the textures batched draws sample. It sits here rather than with
    /// the host because the textured pipeline's layout is built from it at
    /// construction, so the two cannot be split apart later.
    textures: TextureStore,
    renderer: WgpuRenderer2d,
    /// Created on the first frame that carries text, which keeps a host that
    /// never draws text from paying for a glyph atlas. Same policy as the app
    /// shell's.
    text: Option<TextPipeline>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
}

impl Offscreen2d {
    /// Create a target of `width` x `height` pixels (clamped to at least 1x1,
    /// since a zero-sized texture is not renderable).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) -> Self {
        let size = [width.max(1), height.max(1)];
        let (texture, view) = create_target(device, size);
        let textures = TextureStore::new(device, queue);
        let renderer = WgpuRenderer2d::new(device, queue, FORMAT, &textures);
        Self {
            device: device.clone(),
            queue: queue.clone(),
            textures,
            renderer,
            text: None,
            texture,
            view,
            size,
        }
    }

    /// The target's size in pixels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// The view [`render`](Self::render) draws into — also the view a host
    /// registers with its compositor so it can sample the result.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// The texture behind [`view`](Self::view), for hosts that read the
    /// rendered pixels back rather than displaying them (the tests do).
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Resize the target, recreating the texture.
    ///
    /// Returns whether anything changed: a same-size call is a no-op, so a
    /// host can ask every frame while its own surface resizes.
    ///
    /// The old texture is dropped, which invalidates the host's registered
    /// view of it — the host re-registers (or updates) it from
    /// [`view`](Self::view) after this returns `true`.
    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        let size = [width.max(1), height.max(1)];
        if size == self.size {
            return false;
        }
        let (texture, view) = create_target(&self.device, size);
        self.texture = texture;
        self.view = view;
        self.size = size;
        true
    }

    /// Draw `list` into the target and submit the work.
    ///
    /// `default_clear` is the pass's load-op color when the list carries no
    /// `Clear` of its own — the caller's backdrop. The command order, run
    /// interleaving and clear semantics are the window shell's: the first
    /// `Clear` becomes the load-op, later ones repaint over what came before.
    ///
    /// A `TextError` means the glyph pass could not be drawn; the frame's rect
    /// work is already submitted either way, so the caller can log and carry
    /// on rather than losing the frame.
    pub fn render(&mut self, list: &DrawList, default_clear: Rgba) -> Result<(), TextError> {
        let [width, height] = self.size;
        let batch = QuadBatch::from_list(list, [width as f32, height as f32], default_clear);

        if self.text.is_none() && !batch.texts.is_empty() {
            let mut text = TextPipeline::new(&self.device, &self.queue, FORMAT);
            text.set_resolution(&self.queue, width, height);
            self.text = Some(text);
        } else if let Some(text) = self.text.as_mut() {
            text.set_resolution(&self.queue, width, height);
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("cubic-2d offscreen"),
            });
        let result = self.renderer.render(
            &mut encoder,
            &self.view,
            &batch,
            self.text.as_mut(),
            &self.textures,
        );
        self.queue.submit([encoder.finish()]);
        if let Some(text) = self.text.as_mut() {
            text.trim();
        }
        result
    }
}

/// Create the texture and its view: renderable for the pipeline, sampleable for
/// the host's compositor, and copyable so tests can read the pixels back.
fn create_target(device: &wgpu::Device, size: [u32; 2]) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cubic-2d offscreen target"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}
