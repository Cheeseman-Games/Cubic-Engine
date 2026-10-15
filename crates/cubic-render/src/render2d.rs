//! Immediate-mode 2D renderer over wgpu.
//!
//! [`WgpuRenderer2d`] flushes a `cubic_core::render::DrawList` — the same
//! command model the wasm canvas and headless backends consume — into GPU
//! memory. Every `DrawCommand::Rect` becomes one instanced quad, every
//! `DrawCommand::Texture` one instanced textured quad, and every
//! `DrawCommand::Text` becomes one run of glyph quads (see [`crate::text`]), so
//! gameplay code stays backend-agnostic: games emit commands into a `DrawList`
//! and the flush target is invisible to them.
//!
//! [`QuadBatch`] is the CPU-side translation of a `DrawList` into instance data
//! (plus the load-op clear color). It is written to be unit-testable without a
//! GPU device; only [`WgpuRenderer2d::render`] talks to wgpu.
//!
//! Draw order is preserved: contiguous quads of one kind collapse into a single
//! instanced draw call, and a different kind — or a different texture — starts a
//! new run, so a rect emitted after a sprite still paints over it, exactly as the
//! canvas backend does.
//!
//! Textures are resolved per draw through a [`TextureStore`], which holds the GPU
//! side of the asset pipeline: a handle a frame names may not be imported yet, and
//! such a run is skipped rather than drawing something wrong.

use std::borrow::Cow;

use cubic_core::assets::TextureHandle;
use cubic_core::render::{DrawCommand, DrawList, Rgba};
use wgpu::util::DeviceExt;

use crate::assets::TextureStore;
use crate::text::{TextError, TextPipeline, TextSpan};

/// Unit-quad corners in local space, 0..=1, wound counter-clockwise.
pub const UNIT_QUAD: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// Two triangles over [`UNIT_QUAD`]; every rect reuses this index list.
pub const UNIT_QUAD_INDICES: [u16; 6] = [0, 1, 2, 0, 2, 3];

/// Per-instance GPU data: an `x, y, w, h` rectangle plus an RGBA color.
/// One instance per `DrawCommand::Rect` (and per drop-in full-screen clear).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectInstance {
    pub rect: [f32; 4],
    pub color: [f32; 4],
}

// SAFETY: `RectInstance` is `repr(C)` and only holds plain f32 arrays.
unsafe impl bytemuck::Zeroable for RectInstance {}
unsafe impl bytemuck::Pod for RectInstance {}

/// Per-instance GPU data for a textured quad: the same rectangle and tint as a
/// rect, plus the region of the texture to sample, a rotation about the
/// rectangle's centre and the filter to sample the region with.
///
/// The tint is multiplied into the sampled color, so a fully white pixel draws as
/// the tint alone — which is what lets one image be tinted per sprite.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureInstance {
    pub rect: [f32; 4],
    /// `u0, v0, u1, v1` in 0..=1. A whole image is `[0, 0, 1, 1]`; a
    /// sub-rectangle is what a sprite sheet slices out, and a flip is a
    /// sub-rectangle with its edges swapped.
    pub uv: [f32; 4],
    pub color: [f32; 4],
    /// Rotation about the rectangle's centre, in radians.
    pub rotation: f32,
    /// `1.0` to pick the nearest sampler, `0.0` the smooth one. A float rather
    /// than a bool because the vertex layout has no integer attribute here, and
    /// the shader wants a select in any case.
    pub nearest: f32,
}

// SAFETY: `TextureInstance` is `repr(C)` and only holds plain f32 arrays and
// scalars.
unsafe impl bytemuck::Zeroable for TextureInstance {}
unsafe impl bytemuck::Pod for TextureInstance {}

/// The nearest/linear flag a `TextureInstance` carries, as the float the shader
/// selects with.
fn filter_flag(nearest: bool) -> f32 {
    if nearest { 1.0 } else { 0.0 }
}

fn into_array(color: Rgba) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}

/// Which stream a [`BatchRun`] draws from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchKind {
    Rects,
    Textures,
    Text,
}

/// A contiguous span of one stream, emitted as its own draw call.
///
/// Runs appear in `DrawList` order and alternate: consecutive quads of one kind
/// share a run (one instanced draw), while a different kind — or a different
/// texture — splits them. So what lands on screen is painter's order, the order
/// the commands were pushed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatchRun {
    pub kind: BatchKind,
    /// First element of the run, indexed into `rects`, `textures` or `texts` per
    /// `kind`.
    pub start: usize,
    pub len: usize,
    /// The texture this run draws: set for [`BatchKind::Textures`], `None` for the
    /// others. Carrying it here is what lets a run be one draw call — a run only
    /// ever spans quads that sample the same image.
    pub texture: Option<TextureHandle>,
}

/// The CPU translation of a `DrawList` into quads plus glyph runs.
///
/// Clear semantics mirror the canvas backend: the *first* `Clear` becomes the
/// render pass load-op color (and wipes everything drawn before it), any later
/// `Clear` becomes a full-screen rect instance that repaints over prior quads.
#[derive(Clone, Debug, PartialEq)]
pub struct QuadBatch<'a> {
    /// Color to clear the backbuffer with for this frame.
    pub clear: Rgba,
    /// Screen size in physical pixels.
    pub size: [f32; 2],
    /// One instance per `Rect`, plus full-screen rects for extra `Clear`s.
    pub rects: Vec<RectInstance>,
    /// One instance per `Texture`, each sampling the region its `uv` names.
    pub textures: Vec<TextureInstance>,
    /// The frame's `Text` commands, borrowed from the `DrawList`.
    pub texts: Vec<TextSpan<'a>>,
    /// How to interleave [`rects`](Self::rects), [`textures`](Self::textures) and
    /// [`texts`](Self::texts).
    pub runs: Vec<BatchRun>,
}

impl<'a> QuadBatch<'a> {
    /// Build a batch from a frame's commands.
    ///
    /// `default_clear` is used when the list carries no `Clear` of its own — in
    /// the app shell that is the delegate's backbuffer color.
    pub fn from_list(list: &'a DrawList, size: [f32; 2], default_clear: Rgba) -> Self {
        let mut clear = default_clear;
        let mut clearing = false;
        let mut rects = Vec::with_capacity(list.commands.len());
        let mut textures = Vec::new();
        let mut texts = Vec::new();
        let mut runs = Vec::new();
        for command in &list.commands {
            match command {
                DrawCommand::Clear(color) => {
                    if clearing {
                        // A later Clear repaints the whole screen over prior quads.
                        push_rect(&mut rects, &mut runs, [0.0, 0.0, size[0], size[1]], *color);
                    } else {
                        clear = *color;
                        clearing = true;
                    }
                }
                DrawCommand::Rect { x, y, w, h, color } => {
                    if clearing {
                        push_rect(&mut rects, &mut runs, [*x, *y, *w, *h], *color);
                    }
                }
                DrawCommand::Texture(quad) => {
                    if clearing {
                        let index = textures.len();
                        textures.push(TextureInstance {
                            rect: [quad.x, quad.y, quad.w, quad.h],
                            uv: quad.uv.as_array(),
                            color: into_array(quad.color),
                            rotation: quad.rotation,
                            nearest: filter_flag(quad.filter.is_nearest()),
                        });
                        extend_run(&mut runs, BatchKind::Textures, index, Some(quad.texture));
                    }
                }
                DrawCommand::Text {
                    text,
                    x,
                    y,
                    size,
                    color,
                } => {
                    if clearing {
                        let index = texts.len();
                        texts.push(TextSpan {
                            text,
                            x: *x,
                            y: *y,
                            size: *size,
                            color: *color,
                        });
                        extend_run(&mut runs, BatchKind::Text, index, None);
                    }
                }
            }
        }
        Self {
            clear,
            size,
            rects,
            textures,
            texts,
            runs,
        }
    }
}

/// Append a rect instance, opening a new rect run when the last item was something
/// else.
fn push_rect(rects: &mut Vec<RectInstance>, runs: &mut Vec<BatchRun>, rect: [f32; 4], color: Rgba) {
    let index = rects.len();
    rects.push(RectInstance {
        rect,
        color: into_array(color),
    });
    extend_run(runs, BatchKind::Rects, index, None);
}

/// Grow the run that is still open, or start one when the stream just changed.
///
/// `texture` is part of the identity: two commands sampling different images are
/// two draws, so a run only continues while the texture it draws is the same one.
fn extend_run(
    runs: &mut Vec<BatchRun>,
    kind: BatchKind,
    index: usize,
    texture: Option<TextureHandle>,
) {
    match runs.last_mut() {
        Some(open) if open.kind == kind && open.texture == texture => {
            open.len = index + 1 - open.start;
        }
        _ => runs.push(BatchRun {
            kind,
            start: index,
            len: 1,
            texture,
        }),
    }
}

/// The wgpu 2D backend: clear, instanced quads and glyphs in a single pass.
pub struct WgpuRenderer2d {
    /// Held rather than passed to [`render`](Self::render) because every frame
    /// needs both: uploads go through the queue, growth and glyph rasterization
    /// go through the device. Same reason [`TextureStore`] keeps its own.
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    /// The textured pipeline: same vertex layout as [`Self::pipeline`], plus the
    /// store's bind group so it can sample.
    texture_pipeline: wgpu::RenderPipeline,
    unit_quad: wgpu::Buffer,
    unit_quad_indices: wgpu::Buffer,
    /// Instances of `DrawCommand::Rect`, sized on demand.
    instances: Instances,
    /// Instances of `DrawCommand::Texture`, in their own buffer because their
    /// vertex layout is wider.
    textures: Instances,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// One instance stream's GPU data: the buffer, and how many instances it is sized
/// for. The buffer starts as `None` and is created on the first frame that needs
/// it, so a program that never draws a sprite never allocates a texture buffer.
#[derive(Default)]
struct Instances {
    buffer: Option<wgpu::Buffer>,
    capacity: usize,
}

/// Which of the renderer's two instance streams a capacity check is about.
#[derive(Clone, Copy)]
enum InstanceBuffer {
    Rects,
    Textures,
}

const SHADER: &str = r#"
struct Screen {
    size: vec2<f32>,
    padding: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;

struct VsIn {
    @location(0) corner: vec2<f32>,
    @location(1) rect: vec4<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let world = in.rect.xy + in.corner * in.rect.zw;
    let ndc = vec2<f32>(
        2.0 * world.x / screen.size.x - 1.0,
        1.0 - 2.0 * world.y / screen.size.y,
    );
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

/// The textured variant: the same geometry, with the region walked across the
/// quad, a rotation about the quad's centre, and the sampled color multiplied by
/// the instance tint.
///
/// The region's four corners come from the instance rather than being the quad's
/// own corners, which is what sprite-sheet slicing is: a sub-rectangle maps onto
/// the same quad, and a flip is a region whose edges are swapped. The filter is
/// per instance too, so one image can be drawn sharp in a pixel-art sprite and
/// smooth in a scaled one within the same batched draw.
const TEXTURE_SHADER: &str = r#"
struct Screen {
    size: vec2<f32>,
    padding: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
// Not named `image`: that is a WGSL keyword, so a sampler called that would not
// parse.
@group(1) @binding(0) var image_sampler: sampler;
@group(1) @binding(1) var image_tex: texture_2d<f32>;
@group(1) @binding(2) var nearest_sampler: sampler;

struct VsIn {
    @location(0) corner: vec2<f32>,
    @location(1) rect: vec4<f32>,
    @location(2) uv: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) rotation: f32,
    @location(5) nearest: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // Flat: the filter is the same across the whole instance, and interpolating
    // it would only risk a flicker at the seam between two quads.
    @location(2) @interpolate(flat) nearest: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    // Turn about the rectangle's centre, scaling before rotating to match
    // `Transform`: work in the rect's local frame, then place the result.
    let local = (in.corner - vec2<f32>(0.5, 0.5)) * in.rect.zw;
    let sin_r = sin(in.rotation);
    let cos_r = cos(in.rotation);
    let turned = vec2<f32>(
        local.x * cos_r - local.y * sin_r,
        local.x * sin_r + local.y * cos_r,
    );
    let world = in.rect.xy + in.rect.zw * 0.5 + turned;
    let ndc = vec2<f32>(
        2.0 * world.x / screen.size.x - 1.0,
        1.0 - 2.0 * world.y / screen.size.y,
    );
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    // `in.uv` is the region this quad samples and `in.corner` walks across it, so
    // a whole image maps corner to corner, a sub-rectangle maps to itself, and a
    // flipped region samples its rows the other way. v is not flipped: `corner.y`
    // already grows downward like the screen transform above, and row 0 of a
    // decoded image is its top.
    out.uv = in.uv.xy + in.corner * (in.uv.zw - in.uv.xy);
    out.color = in.color;
    out.nearest = in.nearest;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // WGSL takes the texture before the sampler, which is the reverse of the
    // wgpu-level bind group order and reads backwards.
    // Level 0 explicitly: the filter is a per-instance value, and a
    // `textureSample` whose sampler differs across the draw would be
    // non-uniform control flow. These textures have one level, so nothing is
    // lost, and the two taps are chosen between by the instance's flag.
    let sampled_smooth = textureSampleLevel(image_tex, image_sampler, in.uv, 0.0);
    let sampled_nearest = textureSampleLevel(image_tex, nearest_sampler, in.uv, 0.0);
    let sampled = select(sampled_smooth, sampled_nearest, in.nearest > 0.5);
    return sampled * in.color;
}
"#;

impl WgpuRenderer2d {
    /// Create the 2D pipeline for a given present format (e.g. the sRGB view
    /// of the swapchain format). The render target passed to [`render`](Self::render)
    /// must have exactly this format.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        textures: &TextureStore,
    ) -> Self {
        let device = device.clone();
        let queue = queue.clone();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cubic-2d shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
        });
        let texture_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cubic-2d texture shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(TEXTURE_SHADER)),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cubic-2d screen size"),
            // vec2<f32> uniform, padded out to 16 bytes for alignment.
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cubic-2d uniform layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cubic-2d bind group"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cubic-2d pipeline layout"),
            bind_group_layouts: &[Some(&uniform_layout)],
            immediate_size: 0,
        });
        // The textured pipeline binds the screen uniform *and* the store's texture
        // bindings, so its layout has both groups — which is also why it is built
        // from the store's layout rather than one of its own: a pipeline layout
        // must be compatible with the bind groups actually submitted.
        let texture_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("cubic-2d texture pipeline layout"),
                bind_group_layouts: &[Some(&uniform_layout), Some(textures.bind_group_layout())],
                immediate_size: 0,
            });

        let unit_quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cubic-2d unit quad"),
            contents: bytemuck::cast_slice(&UNIT_QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let unit_quad_indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cubic-2d unit quad indices"),
            contents: bytemuck::cast_slice(&UNIT_QUAD_INDICES),
            usage: wgpu::BufferUsages::INDEX,
        });
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cubic-2d instances"),
            size: 0,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cubic-2d texture instances"),
            size: 0,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cubic-2d pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<[f32; 2]>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<RectInstance>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4],
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let texture_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cubic-2d texture pipeline"),
            layout: Some(&texture_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &texture_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<[f32; 2]>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<TextureInstance>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32, 5 => Float32],
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &texture_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            device,
            queue,
            pipeline,
            texture_pipeline,
            unit_quad,
            unit_quad_indices,
            instances: Instances {
                buffer: Some(instance_buffer),
                capacity: 0,
            },
            textures: Instances {
                buffer: Some(texture_buffer),
                capacity: 0,
            },
            uniform_buffer,
            bind_group,
        }
    }

    /// Upload the batch and draw it into `view`.
    ///
    /// `text` is the glyph-atlas pipeline the batch's text runs are drawn with.
    /// It is fed per run, so the recorded order — rects and text alike — is the
    /// order the frame's commands were pushed in. `None` draws a batch with no
    /// text in it, which is what a caller that has not built a [`TextPipeline`]
    /// yet has while its first text-free frames go by.
    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        batch: &QuadBatch<'_>,
        text: Option<&mut TextPipeline>,
        textures: &TextureStore,
    ) -> Result<(), TextError> {
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&[batch.size[0], batch.size[1], 0.0, 0.0]),
        );

        if !batch.rects.is_empty() {
            self.ensure_capacity::<RectInstance>(
                InstanceBuffer::Rects,
                batch.rects.len(),
                "cubic-2d instances",
            );
            self.queue.write_buffer(
                self.instances
                    .buffer
                    .as_ref()
                    .expect("a buffer once allocated"),
                0,
                bytemuck::cast_slice(&batch.rects),
            );
        }

        if !batch.textures.is_empty() {
            self.ensure_capacity::<TextureInstance>(
                InstanceBuffer::Textures,
                batch.textures.len(),
                "cubic-2d texture instances",
            );
            self.queue.write_buffer(
                self.textures
                    .buffer
                    .as_ref()
                    .expect("a buffer once allocated"),
                0,
                bytemuck::cast_slice(&batch.textures),
            );
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cubic-2d pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: batch.clear.r as f64,
                        g: batch.clear.g as f64,
                        b: batch.clear.b as f64,
                        a: batch.clear.a as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        let Some(text) = text else {
            debug_assert!(
                batch.texts.is_empty(),
                "text batch drawn without a text pipeline"
            );
            for run in &batch.runs {
                match run.kind {
                    BatchKind::Rects => self.draw_rect_run(&mut pass, run),
                    BatchKind::Textures => self.draw_texture_run(&mut pass, run, textures),
                    BatchKind::Text => debug_assert!(false, "text run without a text pipeline"),
                }
            }
            return Ok(());
        };

        // Glyphon reallocates its atlas and vertex buffer when a frame needs more
        // room than it has, which would invalidate draws already recorded into
        // this pass — so the frame's whole text budget is reserved up front.
        text.prepare_frame(&self.device, &self.queue, &batch.texts)?;

        for run in &batch.runs {
            match run.kind {
                BatchKind::Rects => self.draw_rect_run(&mut pass, run),
                BatchKind::Textures => self.draw_texture_run(&mut pass, run, textures),
                BatchKind::Text => {
                    let start = run.start;
                    let end = start + run.len;
                    text.draw(
                        &self.device,
                        &self.queue,
                        &mut pass,
                        start,
                        &batch.texts[start..end],
                    )?;
                }
            }
        }

        Ok(())
    }

    /// Draw one run of rect instances as a single indexed instanced call.
    fn draw_rect_run<'r>(&self, pass: &mut wgpu::RenderPass<'r>, run: &BatchRun) {
        if run.len == 0 {
            return;
        }
        let instances = self.instances.buffer.as_ref().expect("a buffer once drawn");
        let first = (run.start * size_of::<RectInstance>()) as u64;
        let bytes = (run.len * size_of::<RectInstance>()) as u64;
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_index_buffer(self.unit_quad_indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.set_vertex_buffer(0, self.unit_quad.slice(..));
        pass.set_vertex_buffer(1, instances.slice(first..first + bytes));
        pass.draw_indexed(0..UNIT_QUAD_INDICES.len() as u32, 0, 0..run.len as u32);
    }

    /// Draw one run of textured instances as a single indexed instanced call.
    ///
    /// A run whose texture has not been imported yet is skipped: the frame still
    /// draws, and the next one picks the texture up once the backend has it. That
    /// is the normal state on the first frame after a load, not an error worth
    /// failing the frame over.
    fn draw_texture_run<'r>(
        &self,
        pass: &mut wgpu::RenderPass<'r>,
        run: &BatchRun,
        textures: &TextureStore,
    ) {
        if run.len == 0 {
            return;
        }
        let Some(handle) = run.texture else {
            return;
        };
        let Some(binding) = textures.binding(handle) else {
            log::debug!("skipping an unimported texture in a draw");
            return;
        };
        let instances = self.textures.buffer.as_ref().expect("a buffer once drawn");
        let first = (run.start * size_of::<TextureInstance>()) as u64;
        let bytes = (run.len * size_of::<TextureInstance>()) as u64;
        pass.set_pipeline(&self.texture_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_bind_group(1, binding.bind_group, &[]);
        pass.set_index_buffer(self.unit_quad_indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.set_vertex_buffer(0, self.unit_quad.slice(..));
        pass.set_vertex_buffer(1, instances.slice(first..first + bytes));
        pass.draw_indexed(0..UNIT_QUAD_INDICES.len() as u32, 0, 0..run.len as u32);
    }

    /// Grow an instance buffer (power-of-two) so it always holds `needed`
    /// instances of `T`. The steady state is one allocation that lives for the
    /// whole session; only growth re-creates the buffer.
    ///
    /// A method rather than a free function because it is `self` that owns the
    /// buffers, and taking `&mut self` alongside `&mut self.instances` would
    /// borrow it twice.
    fn ensure_capacity<T: bytemuck::NoUninit>(
        &mut self,
        which: InstanceBuffer,
        needed: usize,
        label: &'static str,
    ) {
        let stride = size_of::<T>();
        let instances = match which {
            InstanceBuffer::Rects => &mut self.instances,
            InstanceBuffer::Textures => &mut self.textures,
        };
        if needed <= instances.capacity {
            return;
        }
        let capacity = needed
            .next_power_of_two()
            .max(instances.capacity.max(1) * 2);
        instances.buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (capacity * stride) as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        instances.capacity = capacity;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubic_core::render::{Renderer, TextureFilter, TexturedQuad, UvRect};
    use std::num::NonZeroU64;

    fn run(kind: BatchKind, start: usize, len: usize) -> BatchRun {
        BatchRun {
            kind,
            start,
            len,
            texture: None,
        }
    }

    fn texture_run(texture: TextureHandle, start: usize, len: usize) -> BatchRun {
        BatchRun {
            kind: BatchKind::Textures,
            start,
            len,
            texture: Some(texture),
        }
    }

    /// A handle nothing is holding, which is all a batching test needs: batching is
    /// about which commands share a draw call, not about what is on the GPU.
    fn handle(id: u64) -> TextureHandle {
        TextureHandle::new(NonZeroU64::new(id).expect("a handle id is never zero"))
    }

    #[test]
    fn unit_quad_geometry_is_a_solid_square() {
        assert_eq!(UNIT_QUAD.len(), 4);
        assert_eq!(UNIT_QUAD_INDICES.len(), 6);
        // The two triangles share exactly the quad's four corners.
        for index in UNIT_QUAD_INDICES {
            assert!(index < 4, "index {index} out of range");
        }
        let used: std::collections::HashSet<u16> = UNIT_QUAD_INDICES.into_iter().collect();
        assert_eq!(used.len(), 4, "both triangles must cover all four corners");
    }

    #[test]
    fn empty_list_uses_default_clear() {
        let list = DrawList::new();
        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.1, 0.2, 0.3));
        assert_eq!(batch.clear, Rgba::rgb(0.1, 0.2, 0.3));
        assert!(batch.rects.is_empty());
        assert!(batch.runs.is_empty());
    }

    #[test]
    fn first_clear_becomes_load_op_without_an_instance() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(1.0, 0.0, 0.0));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.clear, Rgba::rgb(1.0, 0.0, 0.0));
        assert!(batch.rects.is_empty());
    }

    #[test]
    fn rects_become_instanced_quads() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 1.0));
        list.fill_rect(1.0, 2.0, 3.0, 4.0, Rgba::new(1.0, 0.0, 0.0, 0.5));
        list.fill_rect(5.0, 6.0, 7.0, 8.0, Rgba::new(0.0, 1.0, 0.0, 1.0));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.rects.len(), 2);
        assert_eq!(
            batch.rects[0].rect,
            [1.0, 2.0, 3.0, 4.0],
            "x,y,w,h must round-trip into the instance"
        );
        assert_eq!(batch.rects[0].color, [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(batch.rects[1].rect, [5.0, 6.0, 7.0, 8.0]);
        assert_eq!(batch.rects[1].color, [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn content_before_first_clear_is_wiped() {
        // On canvas a rect painted before the Clear gets covered by it; the
        // load-op clear covers it identically, so it must not be in the batch.
        let mut list = DrawList::new();
        list.fill_rect(0.0, 0.0, 10.0, 10.0, Rgba::rgb(1.0, 0.0, 0.0));
        list.clear(Rgba::rgb(0.2, 0.2, 0.2));
        list.fill_rect(20.0, 20.0, 5.0, 5.0, Rgba::rgb(0.0, 0.0, 1.0));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.clear, Rgba::rgb(0.2, 0.2, 0.2));
        assert_eq!(batch.rects.len(), 1, "pre-clear rect is dropped");
        assert_eq!(batch.rects[0].rect, [20.0, 20.0, 5.0, 5.0]);
    }

    #[test]
    fn later_clear_repaints_the_whole_screen() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.1, 0.1, 0.1));
        list.fill_rect(0.0, 0.0, 10.0, 10.0, Rgba::rgb(1.0, 0.0, 0.0));
        list.clear(Rgba::rgb(0.9, 0.9, 0.9));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.clear, Rgba::rgb(0.1, 0.1, 0.1));
        assert_eq!(batch.rects.len(), 2);
        // The trailing Clear must cover the whole screen in its own color.
        assert_eq!(batch.rects[1].rect, [0.0, 0.0, 800.0, 600.0]);
        assert_eq!(batch.rects[1].color, [0.9, 0.9, 0.9, 1.0]);
    }

    #[test]
    fn text_commands_go_to_the_glyph_stream() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        list.text("hello", 4.0, 6.0, 12.0, Rgba::new(1.0, 0.5, 0.0, 0.25));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert!(batch.rects.is_empty(), "text is not a quad");
        assert_eq!(
            batch.texts,
            vec![TextSpan {
                text: "hello",
                x: 4.0,
                y: 6.0,
                size: 12.0,
                color: Rgba::new(1.0, 0.5, 0.0, 0.25),
            }]
        );
    }

    #[test]
    fn text_before_the_first_clear_is_wiped() {
        let mut list = DrawList::new();
        list.text("gone", 0.0, 0.0, 12.0, Rgba::rgb(1.0, 1.0, 1.0));
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(1.0, 0.0, 0.0));
        assert!(batch.texts.is_empty());
        assert!(batch.runs.is_empty());
    }

    #[test]
    fn contiguous_rects_share_one_run() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        for i in 0..5 {
            list.fill_rect(i as f32, 0.0, 1.0, 1.0, Rgba::rgb(1.0, 1.0, 1.0));
        }

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.runs, vec![run(BatchKind::Rects, 0, 5)]);
    }

    #[test]
    fn runs_interleave_text_with_rects_in_command_order() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        list.fill_rect(0.0, 0.0, 1.0, 1.0, Rgba::rgb(1.0, 0.0, 0.0));
        list.fill_rect(0.0, 0.0, 1.0, 1.0, Rgba::rgb(0.0, 1.0, 0.0));
        list.text("a", 0.0, 0.0, 12.0, Rgba::rgb(1.0, 1.0, 1.0));
        list.text("b", 0.0, 0.0, 12.0, Rgba::rgb(1.0, 1.0, 1.0));
        list.fill_rect(0.0, 0.0, 1.0, 1.0, Rgba::rgb(0.0, 0.0, 1.0));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.rects.len(), 3);
        assert_eq!(batch.texts.len(), 2);
        // Rects, then the adjacent labels in one run, then the rect that has to
        // paint over them.
        assert_eq!(
            batch.runs,
            vec![
                run(BatchKind::Rects, 0, 2),
                run(BatchKind::Text, 0, 2),
                run(BatchKind::Rects, 2, 1),
            ]
        );
        assert_eq!(batch.rects[2].color, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn runs_cover_every_item_exactly_once() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        for i in 0..20 {
            match i % 4 {
                0 => list.fill_rect(0.0, 0.0, 1.0, 1.0, Rgba::rgb(1.0, 1.0, 1.0)),
                1 => list.text("x", 0.0, 0.0, 10.0, Rgba::rgb(1.0, 1.0, 1.0)),
                2 => list.draw_texture(handle(1), 0.0, 0.0, 1.0, 1.0, Rgba::rgb(1.0, 1.0, 1.0)),
                _ => list.text("yy", 0.0, 0.0, 10.0, Rgba::rgb(1.0, 1.0, 1.0)),
            }
        }

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        let mut covered = vec![false; batch.rects.len()];
        let mut sprites = vec![false; batch.textures.len()];
        let mut labels = vec![false; batch.texts.len()];
        let mut previous: Option<(BatchKind, Option<TextureHandle>)> = None;
        for run in &batch.runs {
            if previous == Some((run.kind, run.texture)) {
                panic!("runs of the same stream and texture must merge");
            }
            previous = Some((run.kind, run.texture));
            match run.kind {
                BatchKind::Rects => {
                    for slot in &mut covered[run.start..run.start + run.len] {
                        assert!(!*slot, "a rect is drawn twice");
                        *slot = true;
                    }
                }
                BatchKind::Textures => {
                    for slot in &mut sprites[run.start..run.start + run.len] {
                        assert!(!*slot, "a sprite is drawn twice");
                        *slot = true;
                    }
                }
                BatchKind::Text => {
                    for slot in &mut labels[run.start..run.start + run.len] {
                        assert!(!*slot, "a label is drawn twice");
                        *slot = true;
                    }
                }
            }
        }
        assert!(covered.iter().all(|slot| *slot), "every rect must be drawn");
        assert!(
            sprites.iter().all(|slot| *slot),
            "every sprite must be drawn"
        );
        assert!(labels.iter().all(|slot| *slot), "every label must be drawn");
    }

    #[test]
    fn textures_become_whole_image_quads() {
        let image = handle(1);
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        list.draw_texture(image, 4.0, 6.0, 8.0, 10.0, Rgba::new(1.0, 0.5, 0.0, 0.5));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert!(batch.rects.is_empty(), "a sprite is not a rect");
        assert_eq!(batch.textures.len(), 1);
        assert_eq!(batch.textures[0].rect, [4.0, 6.0, 8.0, 10.0]);
        assert_eq!(batch.textures[0].color, [1.0, 0.5, 0.0, 0.5]);
        // A plain image draws the whole texture, unrotated and smoothly.
        assert_eq!(batch.textures[0].uv, UvRect::WHOLE.as_array());
        assert_eq!(batch.textures[0].rotation, 0.0);
        assert_eq!(batch.textures[0].nearest, 0.0);
        assert_eq!(batch.runs, vec![texture_run(image, 0, 1)]);
    }

    #[test]
    fn a_textured_quad_keeps_its_region_rotation_and_filter() {
        let image = handle(1);
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        list.draw_textured(TexturedQuad {
            texture: image,
            x: 1.0,
            y: 2.0,
            w: 3.0,
            h: 4.0,
            rotation: 0.5,
            uv: UvRect::new(0.25, 0.0, 0.75, 0.5),
            filter: TextureFilter::Nearest,
            color: Rgba::rgb(1.0, 1.0, 1.0),
        });

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.textures.len(), 1);
        let instance = batch.textures[0];
        assert_eq!(instance.rect, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(instance.uv, [0.25, 0.0, 0.75, 0.5], "the region survives");
        assert_eq!(instance.rotation, 0.5);
        assert_eq!(instance.nearest, 1.0, "nearest is the nonzero flag");
    }

    #[test]
    fn a_flipped_sprite_swaps_its_instance_uv_edges() {
        use cubic_core::components::Sprite;

        let image = handle(1);
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        let sprite = Sprite::new(image)
            .with_region(UvRect::new(0.0, 0.0, 1.0, 0.5))
            .with_flip(true, false);
        let quad = sprite.textured_quad(0.0, 0.0, 10.0, 10.0, 0.0);
        list.draw_textured(quad);

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(
            batch.textures[0].uv,
            [1.0, 0.0, 0.0, 0.5],
            "the flip is a uv swap, not extra instance state"
        );
    }

    #[test]
    fn a_sprite_before_the_first_clear_is_wiped() {
        let mut list = DrawList::new();
        list.draw_texture(handle(1), 0.0, 0.0, 4.0, 4.0, Rgba::rgb(1.0, 1.0, 1.0));
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert!(batch.textures.is_empty());
        assert!(batch.runs.is_empty());
    }

    #[test]
    fn sprites_of_one_texture_share_a_run_but_a_second_texture_splits_it() {
        let hero = handle(1);
        let rival = handle(2);
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        for _ in 0..3 {
            list.draw_texture(hero, 0.0, 0.0, 4.0, 4.0, Rgba::rgb(1.0, 1.0, 1.0));
        }
        list.draw_texture(rival, 0.0, 0.0, 4.0, 4.0, Rgba::rgb(1.0, 1.0, 1.0));
        list.draw_texture(hero, 0.0, 0.0, 4.0, 4.0, Rgba::rgb(1.0, 1.0, 1.0));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.textures.len(), 5);
        // Two images cannot be one draw call, so the middle sprite splits the run
        // even though every command is the same shape.
        assert_eq!(
            batch.runs,
            vec![
                texture_run(hero, 0, 3),
                texture_run(rival, 3, 1),
                texture_run(hero, 4, 1),
            ]
        );
    }

    #[test]
    fn sprites_and_rects_keep_their_order_around_text() {
        let image = handle(1);
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        list.fill_rect(0.0, 0.0, 1.0, 1.0, Rgba::rgb(1.0, 0.0, 0.0));
        list.draw_texture(image, 0.0, 0.0, 4.0, 4.0, Rgba::rgb(0.0, 1.0, 0.0));
        list.text("hi", 0.0, 0.0, 12.0, Rgba::rgb(1.0, 1.0, 1.0));
        list.draw_texture(image, 0.0, 0.0, 4.0, 4.0, Rgba::rgb(0.0, 0.0, 1.0));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(
            batch.runs,
            vec![
                run(BatchKind::Rects, 0, 1),
                texture_run(image, 0, 1),
                run(BatchKind::Text, 0, 1),
                texture_run(image, 1, 1),
            ]
        );
    }

    #[test]
    fn many_rects_batch_into_single_instance_list() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        for i in 0..10_000 {
            let i = i as f32;
            list.fill_rect(i, i, 1.0, 1.0, Rgba::rgb(0.1, 0.2, 0.3));
        }
        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.rects.len(), 10_000, "one instance per rect");
        assert_eq!(batch.runs, vec![run(BatchKind::Rects, 0, 10_000)]);
    }
}
