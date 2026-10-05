//! GPU tests for the wgpu 2D renderer: they need a real adapter, so they are
//! `#[ignore]`d and run with
//! `cargo test -p cubic-render --features wgpu --test text_render -- --ignored`.

#![cfg(feature = "wgpu")]

mod common;

use common::{any, matches, read_pixels, target};
use cubic_core::render::{DrawList, Renderer, Rgba};
use cubic_render::assets::TextureStore;
use cubic_render::render2d::{QuadBatch, WgpuRenderer2d};
use cubic_render::text::TextPipeline;

const W: u32 = 480;
const H: u32 = 240;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

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
    let (device, queue) = match common::device() {
        Some(device) => device,
        None => return,
    };
    let target = target(&device, W, H, FORMAT);
    let view = target.create_view(&Default::default());
    // Empty, and made first: the textured pipeline's layout comes from the store,
    // even for a frame that draws no sprites.
    let textures = TextureStore::new(&device, &queue);

    let list = interleaved_list();
    let mut text = TextPipeline::new(&device, &queue, FORMAT);
    text.set_resolution(&queue, W, H);
    let mut renderer = WgpuRenderer2d::new(&device, &queue, FORMAT, &textures);
    let batch = QuadBatch::from_list(&list, [W as f32, H as f32], Rgba::rgb(0.0, 0.0, 0.0));

    // Two frames: the second one reads the atlas the first one left behind, so
    // trimming between frames cannot quietly drop glyphs that are still in use.
    for _ in 0..2 {
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer
            .render(&mut encoder, &view, &batch, Some(&mut text), &textures)
            .expect("render interleaved frame");
        queue.submit([encoder.finish()]);
        text.trim();
    }

    let pixels = read_pixels(&device, &queue, &target, W, H);

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
    let (device, queue) = match common::device() {
        Some(device) => device,
        None => return,
    };
    let target = target(&device, W, H, FORMAT);
    let view = target.create_view(&Default::default());
    let textures = TextureStore::new(&device, &queue);

    let mut list = DrawList::new();
    list.clear(Rgba::rgb(0.0, 0.0, 0.1));
    list.fill_rect(10.0, 10.0, 100.0, 50.0, Rgba::rgb(1.0, 1.0, 0.0));

    let mut renderer = WgpuRenderer2d::new(&device, &queue, FORMAT, &textures);
    let batch = QuadBatch::from_list(&list, [W as f32, H as f32], Rgba::rgb(0.0, 0.0, 0.0));
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer
        .render(&mut encoder, &view, &batch, None, &textures)
        .expect("render text-free frame");
    queue.submit([encoder.finish()]);

    let pixels = read_pixels(&device, &queue, &target, W, H);
    assert!(
        matches(&pixels, |p| p[0] > 200 && p[1] > 200 && p[2] < 90),
        "rect drew"
    );
    assert!(matches(&pixels, |p| p[2] > 30 && p[0] < 32), "clear drew");
}
