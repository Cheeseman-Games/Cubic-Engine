//! GPU tests for the wgpu 2D renderer: they need a real adapter, so they are
//! `#[ignore]`d and run with
//! `cargo test -p cubic-render --features wgpu --test text_render -- --ignored`.

#![cfg(feature = "wgpu")]

mod common;

use common::{any, matches, near, pixel, read_pixels, target};
use cubic_core::assets::{AssetKind, AssetServer, PendingAsset, TextureHandle};
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

/// A sprite drawn over a label must cover it, which is the one thing the
/// textured pipeline has to get right about ordering: it is a separate draw from
/// the glyphs, and the record order is the command order.
#[test]
#[ignore = "needs a wgpu adapter"]
fn a_sprite_covers_the_text_under_it() {
    let (device, queue) = match common::device() {
        Some(device) => device,
        None => return,
    };
    let target = target(&device, W, H, FORMAT);
    let view = target.create_view(&Default::default());
    let mut textures = TextureStore::new(&device, &queue);
    // A white image, so what the sprite shows is its tint and nothing else.
    let white = solid(8, 8, [255, 255, 255, 255]);
    let Some(handle) = import_bytes(&mut textures, &white, "white.png") else {
        return;
    };
    let mut list = DrawList::new();
    list.clear(Rgba::rgb(0.0, 0.0, 0.0));
    list.text("MMM", 20.0, 20.0, 48.0, Rgba::rgb(1.0, 1.0, 1.0));
    list.draw_texture(handle, 20.0, 20.0, 120.0, 60.0, Rgba::rgb(0.0, 1.0, 0.0));

    let mut text = TextPipeline::new(&device, &queue, FORMAT);
    text.set_resolution(&queue, W, H);
    let mut renderer = WgpuRenderer2d::new(&device, &queue, FORMAT, &textures);
    let batch = QuadBatch::from_list(&list, [W as f32, H as f32], Rgba::rgb(0.0, 0.0, 0.0));
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer
        .render(&mut encoder, &view, &batch, Some(&mut text), &textures)
        .expect("render sprite over text");
    queue.submit([encoder.finish()]);

    let pixels = read_pixels(&device, &queue, &target, W, H);
    // The sprite's whole rectangle, including where the label's glyphs were.
    assert!(
        any(&pixels, 30..130, 30..70, |p| p[1] > 200
            && p[0] < 60
            && p[2] < 60),
        "the green sprite drew over the label"
    );
}

/// A handle nothing has imported must be skipped rather than crash the frame —
/// this is what happens on the first frame after a load.
#[test]
#[ignore = "needs a wgpu adapter"]
fn an_unimported_texture_is_skipped_not_fatal() {
    let (device, queue) = match common::device() {
        Some(device) => device,
        None => return,
    };
    let target = target(&device, W, H, FORMAT);
    let view = target.create_view(&Default::default());
    let textures = TextureStore::new(&device, &queue);
    let handle = TextureHandle::new(std::num::NonZeroU64::new(1).expect("one is not zero"));
    assert!(
        textures.binding(handle).is_none(),
        "nothing has imported this handle"
    );

    let mut list = DrawList::new();
    list.clear(Rgba::rgb(0.0, 0.0, 0.1));
    list.draw_texture(
        handle,
        0.0,
        0.0,
        W as f32,
        H as f32,
        Rgba::rgb(1.0, 0.0, 0.0),
    );
    // A rect drawn *after* the skipped sprite, so the frame has something in it.
    list.fill_rect(10.0, 10.0, 40.0, 40.0, Rgba::rgb(1.0, 1.0, 0.0));

    let mut renderer = WgpuRenderer2d::new(&device, &queue, FORMAT, &textures);
    let batch = QuadBatch::from_list(&list, [W as f32, H as f32], Rgba::rgb(0.0, 0.0, 0.0));
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer
        .render(&mut encoder, &view, &batch, None, &textures)
        .expect("a missing texture is not a draw error");
    queue.submit([encoder.finish()]);

    let pixels = read_pixels(&device, &queue, &target, W, H);
    assert!(
        !matches(&pixels, |p| p[0] > 200 && p[1] < 60 && p[2] < 60),
        "the unimported sprite drew nothing"
    );
    assert_eq!(
        pixel(&pixels, 20, 20),
        [255, 255, 0, 255],
        "the later rect drew"
    );
}

/// A PNG whose four quadrants are red, green, blue and white, drawn where the
/// quadrants should land — which is what proves the image is neither flipped nor
/// channel-swapped on the way to the screen.
#[test]
#[ignore = "needs a wgpu adapter"]
fn an_imported_png_draws_the_way_round() {
    let (device, queue) = match common::device() {
        Some(device) => device,
        None => return,
    };
    let target = target(&device, W, H, FORMAT);
    let view = target.create_view(&Default::default());
    let mut textures = TextureStore::new(&device, &queue);
    let Some(handle) = import(&mut textures) else {
        return;
    };

    let size = textures.size(handle).expect("the logo has a size");
    assert_eq!(
        (size.width, size.height),
        (64, 64),
        "the committed logo is 64x64"
    );

    let mut list = DrawList::new();
    list.clear(Rgba::rgb(0.0, 0.0, 0.0));
    // Drawn 1:1, so a pixel of the image lands on a pixel of the target.
    list.draw_texture(handle, 100.0, 50.0, 64.0, 64.0, Rgba::rgb(1.0, 1.0, 1.0));

    let mut renderer = WgpuRenderer2d::new(&device, &queue, FORMAT, &textures);
    let batch = QuadBatch::from_list(&list, [W as f32, H as f32], Rgba::rgb(0.0, 0.0, 0.0));
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer
        .render(&mut encoder, &view, &batch, None, &textures)
        .expect("render the logo");
    queue.submit([encoder.finish()]);

    let pixels = read_pixels(&device, &queue, &target, W, H);
    // Sampled with linear filtering and multiplied by the tint, so each is checked
    // as "dominantly" its own color rather than exactly.
    let at = |x: u32, y: u32| pixel(&pixels, 100 + x, 50 + y);
    assert!(
        near(at(8, 8), Rgba::rgb(1.0, 0.0, 0.0), 40),
        "top left is red"
    );
    assert!(
        near(at(56, 8), Rgba::rgb(0.0, 1.0, 0.0), 40),
        "top right is green"
    );
    assert!(
        near(at(8, 56), Rgba::rgb(0.0, 0.0, 1.0), 40),
        "bottom left is blue"
    );
    assert!(
        near(at(56, 56), Rgba::rgb(1.0, 1.0, 1.0), 40),
        "bottom right is white"
    );
}

/// Re-importing the same handle at the same size must replace the pixels, which
/// is what makes editing a texture in a running game visible without a reload.
#[test]
#[ignore = "needs a wgpu adapter"]
fn re_importing_the_same_size_replaces_the_pixels() {
    let (device, queue) = match common::device() {
        Some(device) => device,
        None => return,
    };
    let target = target(&device, W, H, FORMAT);
    let view = target.create_view(&Default::default());
    let mut textures = TextureStore::new(&device, &queue);

    // One handle, two different images: the path and version differ, the handle
    // does not, which is exactly a file being edited.
    let Some(handle) = import(&mut textures) else {
        return;
    };
    assert_eq!(
        textures.version(handle),
        Some(1),
        "the first import is version 1"
    );

    let mut list = DrawList::new();
    list.clear(Rgba::rgb(0.0, 0.0, 0.0));
    list.draw_texture(handle, 100.0, 50.0, 64.0, 64.0, Rgba::rgb(1.0, 1.0, 1.0));
    let mut renderer = WgpuRenderer2d::new(&device, &queue, FORMAT, &textures);

    // Frame one: the logo, so the top left is red.
    let batch = QuadBatch::from_list(&list, [W as f32, H as f32], Rgba::rgb(0.0, 0.0, 0.0));
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer
        .render(&mut encoder, &view, &batch, None, &textures)
        .expect("first frame");
    queue.submit([encoder.finish()]);

    // The same 64x64, but with the quadrants swapped left to right, handed to the
    // *same* handle at version 2 — which is the shape a watcher produces.
    let swapped = swap_quadrants(include_bytes!("../assets/logo.png"));
    textures
        .import(&reloaded(handle, "logo.png", &swapped))
        .expect("the re-import was accepted");
    assert_eq!(
        textures.version(handle),
        Some(2),
        "the store is now showing version 2"
    );
    assert_eq!(textures.len(), 1, "and still holds exactly one texture");

    // Frame two: the same command, and the pixels under it have changed.
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer
        .render(&mut encoder, &view, &batch, None, &textures)
        .expect("second frame");
    queue.submit([encoder.finish()]);

    let pixels = read_pixels(&device, &queue, &target, W, H);
    let at = |x: u32, y: u32| pixel(&pixels, 100 + x, 50 + y);
    assert!(
        near(at(8, 8), Rgba::rgb(0.0, 1.0, 0.0), 40),
        "the top left is now the green that took its place"
    );
    assert!(
        near(at(56, 8), Rgba::rgb(1.0, 0.0, 0.0), 40),
        "and the top right is red"
    );
}

/// A resized image needs a new texture, and the store must say so.
#[test]
#[ignore = "needs a wgpu adapter"]
fn a_resized_image_gets_a_new_texture_at_the_new_size() {
    let (device, queue) = match common::device() {
        Some(device) => device,
        None => return,
    };
    let mut textures = TextureStore::new(&device, &queue);
    let Some(handle) = import(&mut textures) else {
        return;
    };
    assert_eq!(textures.size(handle).unwrap().width, 64);

    let bigger = scale_quadrants(include_bytes!("../assets/logo.png"), 2);
    textures
        .import(&reloaded(handle, "logo.png", &bigger))
        .expect("the larger image imported");

    let size = textures.size(handle).expect("still sized");
    assert_eq!((size.width, size.height), (128, 128), "the size followed");
    assert_eq!(textures.len(), 1, "the old texture was replaced, not kept");
    assert_eq!(textures.version(handle), Some(2));
}

/// Releasing an asset hands its memory back.
#[test]
#[ignore = "needs a wgpu adapter"]
fn forgetting_a_handle_drops_its_texture() {
    let (device, queue) = match common::device() {
        Some(device) => device,
        None => return,
    };
    let mut textures = TextureStore::new(&device, &queue);
    let Some(handle) = import(&mut textures) else {
        return;
    };
    assert_eq!(textures.len(), 1);
    assert!(
        textures.forget(handle.asset()),
        "there was something to drop"
    );
    assert!(textures.is_empty(), "and it is gone");
    assert!(
        !textures.forget(handle.asset()),
        "forgetting twice is not an error"
    );
    assert!(textures.binding(handle).is_none(), "no binding survives");
}

/// Bytes that are not an image are refused, and the store is left untouched —
/// a file caught mid-save must not blank the sprite that was on screen.
#[test]
#[ignore = "needs a wgpu adapter"]
fn undecodable_bytes_keep_the_previous_texture() {
    let (device, queue) = match common::device() {
        Some(device) => device,
        None => return,
    };
    let mut textures = TextureStore::new(&device, &queue);
    let Some(handle) = import(&mut textures) else {
        return;
    };

    let error = textures
        .import(&reloaded(handle, "logo.png", b"half a file"))
        .expect_err("junk is not an image");

    assert!(error.to_string().contains("is not a usable image"));
    assert_eq!(textures.len(), 1, "the previous texture is still there");
    assert_eq!(
        textures.version(handle),
        Some(1),
        "and still shows the last good version"
    );
}

/// The engine's own test image, imported through a real [`AssetServer`] so the
/// store is fed exactly what a game's asset server would produce.
fn import(store: &mut TextureStore) -> Option<TextureHandle> {
    import_bytes(store, include_bytes!("../assets/logo.png"), "logo.png")
}

/// Import `bytes` as a texture file named `name`, returning the handle.
///
/// The round trip through a file on disk is the point: a `PendingAsset` built by
/// hand would test the store against a test's idea of the server, not the server.
fn import_bytes(store: &mut TextureStore, bytes: &[u8], name: &str) -> Option<TextureHandle> {
    let dir = std::env::temp_dir().join("cubic-asset-render-test");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(dir.join(name), bytes).ok()?;

    let mut assets = AssetServer::from_dir(&dir);
    let handle = assets.load_texture(name).ok()?;
    let pending = assets.drain_pending().into_iter().next()?;
    store.import(&pending).ok()?;
    Some(handle)
}

/// `bytes` as the second version of `name`, for the handle that already holds it.
///
/// This is the shape a watcher produces: the same handle, a bumped version, and
/// whatever the file now says. Built by hand because a test cannot make the
/// operating system deliver an edit on cue.
fn reloaded(handle: TextureHandle, name: &str, bytes: &[u8]) -> PendingAsset {
    PendingAsset {
        handle: handle.asset(),
        path: name.to_string(),
        kind: AssetKind::Texture,
        version: 2,
        bytes: bytes.to_vec().into(),
    }
}

/// A `width` × `height` PNG of one flat color, for the tests that want a sprite
/// whose every pixel is known.
fn solid(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    encode(image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba(color),
    ))
}

/// The logo with its left and right halves exchanged, same size, same handle —
/// an "edit" that must be visible.
fn swap_quadrants(png: &[u8]) -> Vec<u8> {
    let image = image::load_from_memory(png)
        .expect("the logo is a png")
        .to_rgba8();
    let (width, height) = image.dimensions();
    let mut out = image::RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            // x < half maps to the source's right side and vice versa.
            let source_x = if x < width / 2 {
                x + width / 2
            } else {
                x - width / 2
            };
            out.put_pixel(x, y, *image.get_pixel(source_x, y));
        }
    }
    encode(out)
}

/// The logo at twice its size, so a re-import has a different extent.
fn scale_quadrants(png: &[u8], factor: u32) -> Vec<u8> {
    let image = image::load_from_memory(png)
        .expect("the logo is a png")
        .to_rgba8();
    let (width, height) = image.dimensions();
    let mut out = image::RgbaImage::new(width * factor, height * factor);
    for y in 0..height * factor {
        for x in 0..width * factor {
            out.put_pixel(x, y, *image.get_pixel(x / factor, y / factor));
        }
    }
    encode(out)
}

fn encode(image: image::RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode a png");
    bytes
}
