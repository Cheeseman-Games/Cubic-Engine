//! Headless benchmark of the 2D command pipeline: the wgpu quad-batch build
//! vs the legacy canvas-path dispatch, for the same `DrawList`.
//!
//! Both timings are pure CPU (the GPU upload/draw and the wasm JS boundary are
//! not measurable headlessly), so this compares the per-frame command work the
//! two backends do. Reported in microseconds; warns when the wgpu batch build
//! p95 exceeds the 16.7 ms (60 fps) frame budget.
//!
//! Run (release matters for representative timings):
//! `cargo run -p cubic-render --example bench --release`

use cubic_core::render::{DrawCommand, DrawList, Renderer, Rgba};
use cubic_render::render2d::QuadBatch;
use std::time::Instant;

const RECTS: usize = 20_000;
const ITERS: usize = 2_000;

fn main() {
    let mut list = DrawList::new();
    list.clear(Rgba::rgb(0.05, 0.08, 0.11));
    for i in 0..RECTS {
        let i = i as f32;
        list.fill_rect(
            (i * 9.7) % 640.0,
            (i * 3.1) % 480.0,
            24.0,
            24.0,
            Rgba::new(0.2 + (i % 7.0) / 10.0, 0.3, 0.9, 0.5 + (i % 5.0) / 10.0),
        );
    }

    let size = [960.0, 540.0];

    let wgpu_samples = sample(ITERS, || {
        let batch = QuadBatch::from_list(&list, size, Rgba::rgb(0.0, 0.0, 0.0));
        std::hint::black_box(&batch.instances.len());
        std::hint::black_box(batch.clear);
    });

    let canvas_samples = sample(ITERS, || {
        let mut sink = CanvasSink::new(size[0], size[1]);
        dispatch_canvas(&list, &mut sink);
        std::hint::black_box(sink.ops);
        std::hint::black_box(sink.fill_style_len);
    });

    println!("draw list: 1 Clear + {RECTS} Rect commands ({RECTS} instances / quad vertices)");
    println!(
        "{:<22} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "path", "min", "avg", "p95", "p99", "max"
    );
    print_row("wgpu batch build (us)", &wgpu_samples);
    print_row("canvas dispatch (us)", &canvas_samples);

    let wgpu_p95 = pct(&wgpu_samples, 0.95);
    let canvas_p95 = pct(&canvas_samples, 0.95);
    println!(
        "wgpu batch build p95 is {:.2}x the canvas dispatch cost",
        wgpu_p95 / canvas_p95.max(f64::EPSILON)
    );
    if wgpu_p95 > 16_667.0 {
        println!(
            "WARN: wgpu batch build p95 ({wgpu_p95:.1}us) exceeds the 16.7ms (60 fps) frame budget at {RECTS} rects"
        );
    }
}

fn print_row(label: &str, samples: &[f64]) {
    let n = samples.len() as f64;
    let sum: f64 = samples.iter().sum();
    println!(
        "{label:<22} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1}",
        samples[0],
        sum / n,
        pct(samples, 0.95),
        pct(samples, 0.99),
        samples[n as usize - 1],
    );
}

/// Time `f` for `iters` runs; returns sorted per-run microseconds.
fn sample(iters: usize, mut f: impl FnMut()) -> Vec<f64> {
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t0 = Instant::now();
        f();
        samples.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    sorted[((n as f64 * p) as usize).min(n - 1)]
}

/// Opaque stand-in for a `CanvasRenderingContext2d` fill op — counts calls and
/// tracks the css-string traffic the real backend does per rect.
struct CanvasSink {
    width: f32,
    height: f32,
    ops: u64,
    fill_style_len: usize,
}

impl CanvasSink {
    fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            ops: 0,
            fill_style_len: 0,
        }
    }

    fn set_fill_style_str(&mut self, style: &str) {
        self.fill_style_len += style.len();
    }

    fn fill_rect(&mut self, _x: f64, _y: f64, _w: f64, _h: f64) {
        self.ops += 1;
    }
}

/// This is `CanvasRenderer::render` replayed without the JS boundary: the same
/// per-command loop and css string building, writing into [`CanvasSink`]
/// instead of a real 2d context.
fn dispatch_canvas(list: &DrawList, sink: &mut CanvasSink) {
    for command in &list.commands {
        match command {
            DrawCommand::Clear(color) => {
                sink.set_fill_style_str(&css_color(*color));
                sink.fill_rect(0.0, 0.0, sink.width.into(), sink.height.into());
            }
            DrawCommand::Rect { x, y, w, h, color } => {
                sink.set_fill_style_str(&css_color(*color));
                sink.fill_rect((*x).into(), (*y).into(), (*w).into(), (*h).into());
            }
            DrawCommand::Text { .. } => {}
        }
    }
}

/// Mirrors `CanvasRenderer::css_color` so the comparison measures identical
/// work on both sides.
fn css_color(color: Rgba) -> String {
    format!(
        "rgba({},{},{},{:.3})",
        (color.r * 255.0) as u32,
        (color.g * 255.0) as u32,
        (color.b * 255.0) as u32,
        color.a
    )
}
