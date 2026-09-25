//! Immediate-mode 2D renderer over wgpu.
//!
//! [`WgpuRenderer2d`] flushes a `cubic_core::render::DrawList` — the same
//! command model the wasm canvas and headless backends consume — into GPU
//! memory. Every `DrawCommand::Rect` becomes one instanced quad, all batched
//! into a single indexed draw call, so gameplay code stays backend-agnostic:
//! games emit commands into a `DrawList` and the flush target is invisible to
//! them.
//!
//! [`QuadBatch`] is the CPU-side translation of a `DrawList` into instance
//! data (plus the load-op clear color). It is written to be unit-testable
//! without a GPU device; only [`WgpuRenderer2d::render`] talks to wgpu.
//!
//! `DrawCommand::Text` is intentionally skipped here (the glyph-atlas text
//! pipeline is a separate milestone): text commands simply do not produce
//! quads in this pass.

use std::borrow::Cow;

use cubic_core::render::{DrawCommand, DrawList, Rgba};
use wgpu::util::DeviceExt;

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

fn into_array(color: Rgba) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}

/// The CPU translation of a `DrawList` into one batch of instanced quads.
///
/// Clear semantics mirror the canvas backend: the *first* `Clear` becomes the
/// render pass load-op color (and wipes everything drawn before it), any later
/// `Clear` becomes a full-screen rect instance that repaints over prior quads.
#[derive(Clone, Debug, PartialEq)]
pub struct QuadBatch {
    /// Color to clear the backbuffer with for this frame.
    pub clear: Rgba,
    /// Screen size in physical pixels.
    pub size: [f32; 2],
    /// Instances: one per `Rect`, plus full-screen rects for extra `Clear`s.
    pub instances: Vec<RectInstance>,
}

impl QuadBatch {
    /// Build a batch from a frame's commands.
    ///
    /// `default_clear` is used when the list carries no `Clear` of its own — in
    /// the app shell that is the delegate's backbuffer color.
    pub fn from_list(list: &DrawList, size: [f32; 2], default_clear: Rgba) -> Self {
        let mut clear = default_clear;
        let mut clearing = false;
        let mut instances = Vec::with_capacity(list.commands.len());
        for command in &list.commands {
            match command {
                DrawCommand::Clear(color) => {
                    if clearing {
                        // A later Clear repaints the whole screen over prior quads.
                        instances.push(RectInstance {
                            rect: [0.0, 0.0, size[0], size[1]],
                            color: into_array(*color),
                        });
                    } else {
                        clear = *color;
                        clearing = true;
                    }
                }
                DrawCommand::Rect { x, y, w, h, color } => {
                    if clearing {
                        instances.push(RectInstance {
                            rect: [*x, *y, *w, *h],
                            color: into_array(*color),
                        });
                    }
                }
                DrawCommand::Text { .. } => {}
            }
        }
        Self {
            clear,
            size,
            instances,
        }
    }
}

/// The wgpu 2D backend: clear + instanced quads in a single pass.
pub struct WgpuRenderer2d {
    pipeline: wgpu::RenderPipeline,
    unit_quad: wgpu::Buffer,
    unit_quad_indices: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
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

impl WgpuRenderer2d {
    /// Create the 2D pipeline for a given present format (e.g. the sRGB view
    /// of the swapchain format). The render target passed to [`render`]
    /// (`Self::render`) must have exactly this format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cubic-2d shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
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

        Self {
            pipeline,
            unit_quad,
            unit_quad_indices,
            instance_buffer,
            instance_capacity: 0,
            uniform_buffer,
            bind_group,
        }
    }

    /// Upload the batch and draw it into `view`.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        batch: &QuadBatch,
    ) {
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&[batch.size[0], batch.size[1], 0.0, 0.0]),
        );

        let instance_count = batch.instances.len();
        if instance_count > 0 {
            self.ensure_instance_capacity(device, instance_count);
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&batch.instances),
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

        if instance_count > 0 {
            let bytes = (instance_count * size_of::<RectInstance>()) as wgpu::BufferAddress;
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_index_buffer(self.unit_quad_indices.slice(..), wgpu::IndexFormat::Uint16);
            pass.set_vertex_buffer(0, self.unit_quad.slice(..));
            pass.set_vertex_buffer(1, self.instance_buffer.slice(..bytes));
            pass.draw_indexed(
                0..UNIT_QUAD_INDICES.len() as u32,
                0,
                0..instance_count as u32,
            );
        }
    }

    /// Grow the instance buffer (power-of-two) so it always holds `needed`
    /// instances. The steady state is one allocation that lives for the whole
    /// session; only growth re-creates the buffer.
    fn ensure_instance_capacity(&mut self, device: &wgpu::Device, needed: usize) {
        if needed <= self.instance_capacity {
            return;
        }
        let capacity = needed
            .next_power_of_two()
            .max(self.instance_capacity.max(1) * 2);
        self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cubic-2d instances"),
            size: (capacity * size_of::<RectInstance>()) as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = capacity;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubic_core::render::Renderer;

    fn list_with(default_clear: Rgba, size: [f32; 2]) -> QuadBatch {
        QuadBatch::from_list(&DrawList::new(), size, default_clear)
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
        let batch = list_with(Rgba::rgb(0.1, 0.2, 0.3), [800.0, 600.0]);
        assert_eq!(batch.clear, Rgba::rgb(0.1, 0.2, 0.3));
        assert!(batch.instances.is_empty());
    }

    #[test]
    fn first_clear_becomes_load_op_without_an_instance() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(1.0, 0.0, 0.0));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.clear, Rgba::rgb(1.0, 0.0, 0.0));
        assert!(batch.instances.is_empty());
    }

    #[test]
    fn rects_become_instanced_quads() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 1.0));
        list.fill_rect(1.0, 2.0, 3.0, 4.0, Rgba::new(1.0, 0.0, 0.0, 0.5));
        list.fill_rect(5.0, 6.0, 7.0, 8.0, Rgba::new(0.0, 1.0, 0.0, 1.0));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.instances.len(), 2);
        assert_eq!(
            batch.instances[0].rect,
            [1.0, 2.0, 3.0, 4.0],
            "x,y,w,h must round-trip into the instance"
        );
        assert_eq!(batch.instances[0].color, [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(batch.instances[1].rect, [5.0, 6.0, 7.0, 8.0]);
        assert_eq!(batch.instances[1].color, [0.0, 1.0, 0.0, 1.0]);
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
        assert_eq!(batch.instances.len(), 1, "pre-clear rect is dropped");
        assert_eq!(batch.instances[0].rect, [20.0, 20.0, 5.0, 5.0]);
    }

    #[test]
    fn later_clear_repaints_the_whole_screen() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.1, 0.1, 0.1));
        list.fill_rect(0.0, 0.0, 10.0, 10.0, Rgba::rgb(1.0, 0.0, 0.0));
        list.clear(Rgba::rgb(0.9, 0.9, 0.9));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.clear, Rgba::rgb(0.1, 0.1, 0.1));
        assert_eq!(batch.instances.len(), 2);
        // The trailing Clear must cover the whole screen in its own color.
        assert_eq!(batch.instances[1].rect, [0.0, 0.0, 800.0, 600.0]);
        assert_eq!(batch.instances[1].color, [0.9, 0.9, 0.9, 1.0]);
    }

    #[test]
    fn text_commands_produce_no_quads() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        list.text("hello", 4.0, 4.0, 12.0, Rgba::rgb(1.0, 1.0, 1.0));
        list.fill_rect(1.0, 1.0, 2.0, 2.0, Rgba::rgb(0.5, 0.5, 0.5));

        let batch = QuadBatch::from_list(&list, [800.0, 600.0], Rgba::rgb(0.0, 0.0, 0.0));
        assert_eq!(batch.instances.len(), 1);
        assert_eq!(batch.instances[0].rect, [1.0, 1.0, 2.0, 2.0]);
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
        assert_eq!(batch.instances.len(), 10_000, "one instance per rect");
    }
}
