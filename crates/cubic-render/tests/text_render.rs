//! GPU tests for the wgpu 2D renderer: they need a real adapter, so they are
//! `#[ignore]`d and run with
//! `cargo test -p cubic-render --features wgpu --test text_render -- --ignored`.

#![cfg(feature = "wgpu")]

use cubic_core::render::{DrawList, Renderer, Rgba};
use cubic_render::render2d::{QuadBatch, WgpuRenderer2d};
use cubic_render::text::TextPipeline;

const W: u32 = 480;
const H: u32 = 240;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

type Pixels = Vec<[u8; 4]>;

/// Text and rects interleaved, so the runs are `text, text, rect, text` and the
/// batching has to split them without losing or reordering any of them.
fn interleaved_list() -> DrawList {
    let mut list = DrawList::new();
    list.clear(Rgba::rgb(0.0, 0.0, 0.0));
    list.text("Hxg", 20.0, 20.0, 48.0, Rgba::rgb(1.0, 1.0, 1.0));
    list.text("baseline", 20.0, 100.0, 16.0, Rgba::rgb(0.5, 1.0, 0.5));
    list.fill_rect(0.0, 120.0, 480.0, 4.0, Rgba::rgb(0.2, 0.2, 0.4));
    list.text("after rect", 20.0, 160.0, 24.0, Rgba::rgb(1.0, 0.6, 0.2));
    // Painted last, so it must cover the left part of the 48 px label.
    list.fill_rect(20.0, 20.0, 40.0, 40.0, Rgba::rgb(1.0, 0.0, 0.0));
    list
}

#[test]
#[ignore = "needs a wgpu adapter"]
fn text_runs_survive_interleaved_rects() {
    let (device, queue) = match device() {
        Some(device) => device,
        None => return,
    };
    let target = target(&device);
    let view = target.create_view(&Default::default());

    let list = interleaved_list();
    let mut text = TextPipeline::new(&device, &queue, FORMAT);
    text.set_resolution(&queue, W, H);
    let mut renderer = WgpuRenderer2d::new(&device, FORMAT);
    let batch = QuadBatch::from_list(&list, [W as f32, H as f32], Rgba::rgb(0.0, 0.0, 0.0));

    // Two frames: the second one reads the atlas the first one left behind, so
    // trimming between frames cannot quietly drop glyphs that are still in use.
    for _ in 0..2 {
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer
            .render(
                &device,
                &queue,
                &mut encoder,
                &view,
                &batch,
                Some(&mut text),
            )
            .expect("render interleaved frame");
        queue.submit([encoder.finish()]);
        text.trim();
    }

    let pixels = read_pixels(&device, &queue, &target);

    assert!(
        matches(&pixels, |p| p[0] < 32 && p[1] < 32 && p[2] < 32),
        "clear color kept"
    );
    // The 48 px label, to the right of the red rect painted over its left half.
    assert!(
        any(&pixels, 70..115, 20..72, |p| p[0] > 200
            && p[1] > 200
            && p[2] > 200),
        "48 px label drew its glyphs"
    );
    assert!(
        any(&pixels, 20..60, 20..60, |p| p[0] > 200
            && p[1] < 90
            && p[2] < 90),
        "the later rect covers the label it overlaps"
    );
    // The 16 px label, in the run before the rect.
    assert!(
        any(&pixels, 20..120, 100..118, |p| p[1] > 180 && p[0] < 200),
        "16 px label drew its glyphs"
    );
    // The 4 px full-width rect.
    assert!(
        any(&pixels, 0..W, 120..124, |p| p[2] > p[0] && p[0] < 200),
        "the rect between the text runs drew"
    );
    // The 24 px label, in the run after the rect.
    assert!(
        any(&pixels, 20..150, 160..190, |p| p[0] > 180
            && p[1] < 200
            && p[2] < 120),
        "24 px label drew its glyphs after the rect"
    );
}

/// A batch with no text in it must draw with no pipeline at all, which is the
/// state a window sits in until its first label.
#[test]
#[ignore = "needs a wgpu adapter"]
fn text_free_batches_draw_without_a_pipeline() {
    let (device, queue) = match device() {
        Some(device) => device,
        None => return,
    };
    let target = target(&device);
    let view = target.create_view(&Default::default());

    let mut list = DrawList::new();
    list.clear(Rgba::rgb(0.0, 0.0, 0.1));
    list.fill_rect(10.0, 10.0, 100.0, 50.0, Rgba::rgb(1.0, 1.0, 0.0));

    let mut renderer = WgpuRenderer2d::new(&device, FORMAT);
    let batch = QuadBatch::from_list(&list, [W as f32, H as f32], Rgba::rgb(0.0, 0.0, 0.0));
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer
        .render(&device, &queue, &mut encoder, &view, &batch, None)
        .expect("render text-free frame");
    queue.submit([encoder.finish()]);

    let pixels = read_pixels(&device, &queue, &target);
    assert!(
        matches(&pixels, |p| p[0] > 200 && p[1] > 200 && p[2] < 90),
        "rect drew"
    );
    assert!(matches(&pixels, |p| p[2] > 30 && p[0] < 32), "clear drew");
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let device = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        .expect("request device");
    Some(device)
}

fn target(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cubic-2d test target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn read_pixels(device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::Texture) -> Pixels {
    let row = W * 4;
    let padded: u32 = row.div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cubic-2d test readback"),
        size: u64::from(padded) * u64::from(H),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map callback").expect("map");
    let data = slice.get_mapped_range().expect("mapped range");
    let stride = padded as usize;
    let mut pixels = Pixels::with_capacity((W * H) as usize);
    for y in 0..H as usize {
        for x in 0..W as usize {
            let i = y * stride + x * 4;
            pixels.push([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        }
    }
    pixels
}

/// How many pixels in the `x`/`y` ranges pass `wanted`, used for both "this
/// drew" and "this is exactly the color I asked for" checks.
fn count(
    pixels: &Pixels,
    x: std::ops::Range<u32>,
    y: std::ops::Range<u32>,
    wanted: impl Fn([u8; 4]) -> bool,
) -> usize {
    let mut hits = 0;
    for py in y {
        for px in x.clone() {
            if wanted(pixels[py as usize * W as usize + px as usize]) {
                hits += 1;
            }
        }
    }
    hits
}

fn any(
    pixels: &Pixels,
    x: std::ops::Range<u32>,
    y: std::ops::Range<u32>,
    wanted: impl Fn([u8; 4]) -> bool,
) -> bool {
    count(pixels, x, y, wanted) > 0
}

fn matches(pixels: &Pixels, wanted: impl Fn([u8; 4]) -> bool) -> bool {
    pixels.iter().copied().any(wanted)
}
