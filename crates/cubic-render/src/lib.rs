//! Concrete render backends for `cubic-core`'s command model.
//!
//! The command model itself (`Renderer`, `DrawList`) lives in `cubic-core`;
//! this crate flushes it to a real target. Today that is the wasm 2d-canvas
//! backend behind the `web` feature; the wgpu desktop backend lands here
//! next.

#[cfg(all(target_arch = "wasm32", feature = "web"))]
pub mod canvas;
