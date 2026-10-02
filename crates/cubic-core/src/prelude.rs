//! Game-facing prelude for `cubic-core`.
//!
//! One glob import covers the whole gameplay surface — ECS, math, input,
//! transforms, the render command model and the lifecycle traits — so a game
//! never has to know which module a type lives in. Backends add their own
//! layer on top (`cubic_render::prelude`), which re-exports everything here.
//!
//! Everything gated behind the `rendering` feature is re-exported behind the
//! same gate, so this module still resolves in the headless
//! (`--no-default-features`) build.

#[cfg(feature = "rendering")]
pub use crate::Game;
pub use crate::components::{Transform, transform_direction, transform_point};
pub use crate::input::{FrameInput, Gamepad, InputState, KeyCode, MouseButton};
pub use crate::math::{Mat4, Quat, Rect, Vec2, Vec3, Vec4};
#[cfg(feature = "rendering")]
pub use crate::render::{DrawCommand, DrawList, NullRenderer, Renderer, Rgba};
pub use crate::world::{EntityId, World};
pub use crate::{System, TickContext};
