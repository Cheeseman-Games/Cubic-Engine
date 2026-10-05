//! Shared wgpu test harness: an adapter, a render target, and a way to read the
//! pixels back.
//!
//! Every GPU test in this crate needs the same three things, and the pixel
//! assertions are worth comparing across tests — so they live here rather than
//! being copied per file.

#![allow(dead_code)]

use cubic_core::render::Rgba;

/// A device and its queue, or `None` when the machine has no adapter — a headless
/// CI box, for instance. A GPU test that gets `None` passes rather than failing,
/// so the suite is green without a GPU and meaningful with one.
pub fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// An off-screen render target in `format`, which every test here draws into.
pub fn target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cubic-2d test target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// One row of `Pixels`, addressable as `pixels[y][x]`.
pub type Pixels = Vec<Vec<[u8; 4]>>;

/// Read a target back after everything submitted so far has landed.
pub fn read_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Pixels {
    let padded = (width * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cubic-2d test readback"),
        size: u64::from(padded) * u64::from(height),
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
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
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
    let mut pixels = Pixels::with_capacity(height as usize);
    for y in 0..height as usize {
        let row = (0..width as usize)
            .map(|x| {
                let i = y * stride + x * 4;
                [data[i], data[i + 1], data[i + 2], data[i + 3]]
            })
            .collect();
        pixels.push(row);
    }
    pixels
}

/// The pixel at `(x, y)`, for the "did this draw *there*" assertions that are
/// most of what a GPU test checks.
pub fn pixel(pixels: &Pixels, x: u32, y: u32) -> [u8; 4] {
    pixels[y as usize][x as usize]
}

/// How many pixels in the `x`/`y` ranges pass `wanted`.
pub fn count(
    pixels: &Pixels,
    x: std::ops::Range<u32>,
    y: std::ops::Range<u32>,
    wanted: impl Fn([u8; 4]) -> bool,
) -> usize {
    let mut hits = 0;
    for row in y {
        for column in x.clone() {
            if wanted(pixels[row as usize][column as usize]) {
                hits += 1;
            }
        }
    }
    hits
}

/// Whether any pixel in the `x`/`y` ranges passes `wanted`.
pub fn any(
    pixels: &Pixels,
    x: std::ops::Range<u32>,
    y: std::ops::Range<u32>,
    wanted: impl Fn([u8; 4]) -> bool,
) -> bool {
    count(pixels, x, y, wanted) > 0
}

/// Whether any pixel anywhere passes `wanted`.
pub fn matches(pixels: &Pixels, wanted: impl Fn([u8; 4]) -> bool) -> bool {
    pixels.iter().flatten().copied().any(wanted)
}

/// Whether `color` is close enough to `p` to count as having drawn, given that a
/// blended and sampled pixel is not the byte you asked for.
pub fn near(p: [u8; 4], color: Rgba, tolerance: i32) -> bool {
    let want = [
        (color.r * 255.0) as i32,
        (color.g * 255.0) as i32,
        (color.b * 255.0) as i32,
    ];
    want.iter()
        .zip(p.iter().take(3))
        .all(|(w, got)| (*got as i32 - w).abs() <= tolerance)
}
