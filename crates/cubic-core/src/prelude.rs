//! Public prelude for `cubic-core`.
//!
//! Re-exports the whole gameplay surface in one glob so game crates do not
//! have to know which module a type lives in. Everything gated behind the
//! `rendering` feature is re-exported behind the same gate, so this module
//! resolves in the headless (`--no-default-features`) build too.

pub use crate::components::Transform;
pub use crate::input::{FrameInput, InputState, KeyCode, MouseButton};
pub use crate::math::{Mat4, Quat, Rect, Vec2, Vec3, Vec4};
#[cfg(feature = "rendering")]
pub use crate::render::{DrawCommand, DrawList, Renderer, Rgba};
pub use crate::world::{EntityId, World};
#[cfg(feature = "rendering")]
pub use crate::{Game, GameDriver};
pub use crate::{System, TickContext};
