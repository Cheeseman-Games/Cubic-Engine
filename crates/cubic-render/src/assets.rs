//! The GPU half of the asset pipeline: importing an asset's bytes.
//!
//! [`cubic_core::assets::AssetServer`] is the game's half — it turns a path into
//! a [`TextureHandle`] and keeps its bytes
//! current, without knowing what a GPU is. This module is the other half: it
//! takes the [`PendingAsset`]s the server queues, decodes them, and keeps the
//! textures they name on the device.
//!
//! ```text
//! AssetServer::load_texture("assets/logo.png")
//!   → handle + PendingAsset { handle, bytes, version, .. }
//!   → TextureStore::import(&pending)          (this module)
//!   → DrawCommand::Texture { texture: handle, .. }
//!   → WgpuRenderer2d::render(.., &store)      (crate::render2d)
//! ```
//!
//! Handles, not paths, are what every step after the load passes around, which is
//! what makes hot reload work: a re-import replaces the bytes behind a handle the
//! game is already drawing, so nothing outside this module has to know the file
//! changed.
//!
//! Decoding is [`image`]'s job and is deliberately narrow — the formats the
//! engine is built with. Bytes that are not one of those are refused with a
//! [`TextureError`] rather than uploaded as noise.

use std::collections::BTreeMap;

use cubic_core::assets::{AssetHandle, PendingAsset, TextureHandle};

/// Why a texture's bytes could not become GPU state.
///
/// Only one way to fail, and that is deliberate: the decoder decides what an image
/// is, so a file it refuses is refused here with its own explanation attached. A
/// separate "decoded but unusable" case would only mean this module doubted the
/// decoder — and a zero-sized image never gets that far, because every format the
/// engine reads rejects one at the decode.
#[derive(Debug)]
pub enum TextureError {
    /// The bytes are not an image the decoder understands.
    Decode {
        /// The asset's path, normalized.
        path: String,
        /// What the decoder said.
        error: image::ImageError,
    },
}

impl std::fmt::Display for TextureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode { path, error } => write!(f, "`{path}` is not a usable image: {error}"),
        }
    }
}

impl std::error::Error for TextureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode { error, .. } => Some(error),
        }
    }
}

/// A texture's size in pixels, for a caller that needs its dimensions before it
/// has sampled one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureSize {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl TextureSize {
    /// The size as the `[f32; 2]` batch and instance data speak.
    pub fn as_f32(self) -> [f32; 2] {
        [self.width as f32, self.height as f32]
    }
}

/// One imported texture: the GPU objects a draw needs, plus what they were made
/// from.
struct Imported {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
    /// The [`PendingAsset::version`] the current pixels came from, so a re-import
    /// is observable without comparing images.
    version: u32,
}

/// Everything a draw of one texture needs: what to sample and with what.
pub struct TextureBinding<'a> {
    /// The sampled image.
    pub view: &'a wgpu::TextureView,
    /// The texture bound to the store's sampler, ready for the draw pipeline.
    pub bind_group: &'a wgpu::BindGroup,
}

/// The device-side texture cache, keyed by asset handle.
///
/// One store per device, owned by the runtime and handed to
/// [`WgpuRenderer2d::render`](crate::render2d::WgpuRenderer2d::render) so the
/// renderer can resolve the handles a frame's commands name. The store owns the
/// bind group layout those draws use, because what a texture has to be bound with
/// is this module's business rather than the renderer's.
pub struct TextureStore {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// One sampler for every texture, so a draw needs one binding rather than one
    /// per texture. Linear filtering with edge clamping: the filter a sprite
    /// wants, and no way for a filtered tap to read past the edge.
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
    imported: BTreeMap<u64, Imported>,
}

impl TextureStore {
    /// Create a store for `device`, with no textures in it.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let device = device.clone();
        let queue = queue.clone();
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cubic-texture sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cubic-texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        Self {
            device,
            queue,
            sampler,
            layout,
            imported: BTreeMap::new(),
        }
    }

    /// The bind group layout every texture is bound with, and that the textured
    /// draw pipeline must be built against.
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// Decode `asset` and make it drawable, replacing whatever the handle held.
    ///
    /// A first import allocates; a re-import reuses the texture when the image is
    /// the same size and only writes new pixels into it, so editing a texture in a
    /// running game does not churn GPU memory. A size change needs a new texture —
    /// the old one is the wrong shape — and the handle's bind group is rebuilt to
    /// match.
    pub fn import(&mut self, asset: &PendingAsset) -> Result<(), TextureError> {
        let image = decode(&asset.path, asset.bytes())?;
        let (width, height) = image.dimensions();

        let id = asset.handle.id();
        let created = match self.imported.get_mut(&id) {
            Some(imported) if imported.size == (width, height) => {
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &imported.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    image.as_raw(),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * 4),
                        rows_per_image: Some(height),
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
                imported.version = asset.version;
                false
            }
            // Either a new handle or a resized image: both need a new texture.
            _ => {
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("cubic-texture"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    // The pipeline samples an sRGB texture, so the draw is tinted
                    // like every other color on screen rather than twice-corrected.
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    image.as_raw(),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * 4),
                        rows_per_image: Some(height),
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
                let view = texture.create_view(&Default::default());
                let bind_group = self.bind_group(&view);
                self.imported.insert(
                    id,
                    Imported {
                        texture,
                        view,
                        bind_group,
                        size: (width, height),
                        version: asset.version,
                    },
                );
                true
            }
        };

        log::debug!(
            "imported {} ({}x{}, version {}) {}",
            asset.path,
            width,
            height,
            asset.version,
            if created {
                "as a new texture"
            } else {
                "in place"
            }
        );
        Ok(())
    }

    /// Drop whatever GPU state `handle` was holding, for an asset whose last
    /// reference went away.
    ///
    /// Returns whether there was anything to drop. Releasing an asset the store
    /// never imported — a font, or bytes the game read itself — is not an error:
    /// the store holds only what it was asked to draw.
    pub fn forget(&mut self, handle: AssetHandle) -> bool {
        self.imported.remove(&handle.id()).is_some()
    }

    /// What a draw of `texture` needs, or `None` if it has not been imported.
    ///
    /// `None` is a normal state, not a failure: a game can emit a texture command
    /// before the backend has imported it, and a re-import that failed leaves the
    /// previous version in place. Both skip the draw.
    pub fn binding(&self, texture: TextureHandle) -> Option<TextureBinding<'_>> {
        self.imported
            .get(&texture.asset().id())
            .map(|imported| TextureBinding {
                view: &imported.view,
                bind_group: &imported.bind_group,
            })
    }

    /// The size of an imported texture.
    pub fn size(&self, texture: TextureHandle) -> Option<TextureSize> {
        self.imported
            .get(&texture.asset().id())
            .map(|imported| TextureSize {
                width: imported.size.0,
                height: imported.size.1,
            })
    }

    /// The asset version an imported texture's pixels came from.
    pub fn version(&self, texture: TextureHandle) -> Option<u32> {
        self.imported
            .get(&texture.asset().id())
            .map(|imported| imported.version)
    }

    /// How many textures are imported.
    pub fn len(&self) -> usize {
        self.imported.len()
    }

    /// Whether nothing is imported.
    pub fn is_empty(&self) -> bool {
        self.imported.is_empty()
    }

    /// A bind group sampling `view` with the store's sampler.
    fn bind_group(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cubic-texture bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
            ],
        })
    }
}

/// Decode asset bytes into the RGBA8 pixels the pipeline samples.
///
/// The one place that knows what a texture's bytes look like: everything else
/// here deals in GPU objects. Split out from [`TextureStore::import`] so what an
/// engine can decode can be checked without a device — and so a rejected image is
/// a decision about *bytes*, not about whether a GPU happens to be present.
fn decode(path: &str, bytes: &[u8]) -> Result<image::RgbaImage, TextureError> {
    image::load_from_memory(bytes)
        .map(|decoded| decoded.to_rgba8())
        .map_err(|error| TextureError::Decode {
            path: path.to_string(),
            error,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine's own test image: a real PNG in the crate's assets directory,
    /// committed so the example and these tests share one file and no test needs a
    /// decoder-specific fixture of its own.
    ///
    /// Four quadrants — red, green, blue, white — so a test can say *which* corner
    /// of the image arrived, not merely that something decoded.
    const LOGO: &[u8] = include_bytes!("../assets/logo.png");

    #[test]
    fn the_test_image_decodes_to_the_quadrants_it_looks_like() {
        let image = decode("assets/logo.png", LOGO).expect("the logo is a png");

        assert_eq!(image.dimensions(), (64, 64));
        let pixel = |x: u32, y: u32| {
            let [r, g, b, _] = image.get_pixel(x, y).0;
            (r, g, b)
        };
        assert_eq!(pixel(8, 8), (255, 0, 0), "top left is red");
        assert_eq!(pixel(56, 8), (0, 255, 0), "top right is green");
        assert_eq!(pixel(8, 56), (0, 0, 255), "bottom left is blue");
        assert_eq!(pixel(56, 56), (255, 255, 255), "bottom right is white");
    }

    /// Row order is the pipeline's assumption — `uv = corner` puts row 0 at the top
    /// — so a decoder that handed the rows over the other way would draw the image
    /// upside down rather than fail.
    #[test]
    fn decoding_keeps_the_first_row_first() {
        let image = decode("assets/logo.png", LOGO).expect("the logo is a png");
        let top: Vec<_> = (0..4).map(|x| image.get_pixel(x, 0).0).collect();
        let bottom: Vec<_> = (0..4).map(|x| image.get_pixel(x, 63).0).collect();
        assert_ne!(top, bottom, "the rows must not be swapped");
    }

    /// Bytes that are not an image are refused with a message a game can print,
    /// rather than uploaded as noise.
    #[test]
    fn bytes_that_are_not_an_image_are_refused() {
        let error = decode("assets/hero.png", b"this is not a png").unwrap_err();

        assert!(
            matches!(&error, TextureError::Decode { path, .. } if path == "assets/hero.png"),
            "{error}"
        );
        assert!(
            error.to_string().contains("is not a usable image"),
            "{error}"
        );
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn texture_size_reports_the_float_pair_instances_speak() {
        assert_eq!(
            TextureSize {
                width: 64,
                height: 32
            }
            .as_f32(),
            [64.0, 32.0]
        );
    }
}
