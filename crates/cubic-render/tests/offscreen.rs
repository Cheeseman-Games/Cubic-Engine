//! GPU tests for the offscreen target: they need a real adapter, so they are
//! `#[ignore]`d and run with
//! `cargo test -p cubic-render --features wgpu --test offscreen -- --ignored`.

#![cfg(feature = "wgpu")]

mod common;

use cubic_core::render::{DrawList, Renderer, Rgba};
use cubic_render::offscreen::Offscreen2d;

/// The clear and a rect land in the target the caller asked for: green
/// backdrop, red block at a known spot, read back through the same texture the
/// host would sample.
#[test]
#[ignore = "needs a wgpu adapter"]
fn offscreen_clears_and_draws_into_its_own_texture() {
    let Some((device, queue)) = common::device() else {
        return;
    };
    let mut offscreen = Offscreen2d::new(&device, &queue, 32, 16);
    assert_eq!(offscreen.size(), [32, 16]);

    let mut list = DrawList::new();
    list.clear(Rgba::rgb(0.0, 1.0, 0.0));
    list.fill_rect(8.0, 4.0, 8.0, 8.0, Rgba::rgb(1.0, 0.0, 0.0));
    offscreen
        .render(&list, Rgba::rgb(0.0, 0.0, 0.0))
        .expect("render offscreen frame");

    let pixels = common::read_pixels(&device, &queue, offscreen.texture(), 32, 16);
    assert!(
        common::near(common::pixel(&pixels, 1, 1), Rgba::rgb(0.0, 1.0, 0.0), 4),
        "the list's clear color fills the target"
    );
    assert!(
        common::near(common::pixel(&pixels, 12, 8), Rgba::rgb(1.0, 0.0, 0.0), 4),
        "the rect drew where the list put it"
    );
    assert!(
        common::near(common::pixel(&pixels, 24, 8), Rgba::rgb(0.0, 1.0, 0.0), 4),
        "outside the rect the clear color shows"
    );
}

/// A same-size resize is a no-op; a different size replaces the target and the
/// next frame draws into the new one at its new dimensions.
#[test]
#[ignore = "needs a wgpu adapter"]
fn resize_swaps_the_target_only_when_the_size_changes() {
    let Some((device, queue)) = common::device() else {
        return;
    };
    let mut offscreen = Offscreen2d::new(&device, &queue, 32, 32);
    assert!(!offscreen.resize(32, 32), "same size changes nothing");
    assert!(offscreen.resize(16, 8), "a new size recreates the target");
    assert_eq!(offscreen.size(), [16, 8]);

    let mut list = DrawList::new();
    list.clear(Rgba::rgb(0.0, 0.0, 1.0));
    offscreen
        .render(&list, Rgba::rgb(0.0, 0.0, 0.0))
        .expect("render after resize");

    let pixels = common::read_pixels(&device, &queue, offscreen.texture(), 16, 8);
    assert!(
        common::matches(&pixels, |p| {
            common::near(p, Rgba::rgb(0.0, 0.0, 1.0), 4)
        }),
        "the resized target holds the new frame"
    );
    assert_eq!(pixels.len(), 8, "read back at the resized height");
}
