use std::borrow::Cow;

use crate::assets::TextureHandle;

/// RGBA color, components normalized to 0.0..=1.0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

/// One retained draw command. The game emits commands into a `DrawList` each
/// frame — pure Rust, zero JS — and backends flush the whole list in one shot.
/// That gives WebGPU a single vertex upload + draw call per frame.
#[derive(Clone, Debug)]
pub enum DrawCommand {
    Clear(Rgba),
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgba,
    },
    Text {
        text: Cow<'static, str>,
        x: f32,
        y: f32,
        size: f32,
        color: Rgba,
    },
    /// A whole texture, stretched into the given rectangle.
    ///
    /// The handle, not a path: a backend that has not imported it yet cannot draw
    /// it and must skip the command, and one whose bytes have since been replaced
    /// draws the new bytes without the game saying anything.
    Texture {
        texture: TextureHandle,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        /// Multiplied into the sampled color; white leaves the image as it is.
        color: Rgba,
    },
}

/// Frame-local command buffer. Persistent so each frame only pushes commands
/// and one `reset()` — no steady-state allocation.
#[derive(Default)]
pub struct DrawList {
    pub commands: Vec<DrawCommand>,
}

impl DrawList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.commands.clear();
    }
}

/// Immediate-mode render API. `DrawList` implements it to capture commands;
/// a backend (canvas / WebGPU) consumes a `&DrawList`.
pub trait Renderer {
    fn clear(&mut self, color: Rgba);
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgba);
    fn text(&mut self, text: &str, x: f32, y: f32, size: f32, color: Rgba);

    /// Draw a texture, stretched into `x, y, w, h`.
    ///
    /// The whole image, tinted by `color`. Sub-rectangles, rotation and flipping
    /// are a sprite's business rather than this command's: a command that could
    /// express them would carry a transform every backend has to interpret.
    fn draw_texture(&mut self, texture: TextureHandle, x: f32, y: f32, w: f32, h: f32, color: Rgba);
}

impl Renderer for DrawList {
    fn clear(&mut self, color: Rgba) {
        self.commands.push(DrawCommand::Clear(color));
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgba) {
        self.commands.push(DrawCommand::Rect { x, y, w, h, color });
    }

    fn text(&mut self, text: &str, x: f32, y: f32, size: f32, color: Rgba) {
        self.commands.push(DrawCommand::Text {
            text: Cow::Owned(text.to_owned()),
            x,
            y,
            size,
            color,
        });
    }

    fn draw_texture(
        &mut self,
        texture: TextureHandle,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgba,
    ) {
        self.commands.push(DrawCommand::Texture {
            texture,
            x,
            y,
            w,
            h,
            color,
        });
    }
}

/// Headless backend for native simulation runs.
pub struct NullRenderer;

impl NullRenderer {
    pub fn render(&mut self, _list: &DrawList) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_list_resets_between_frames() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.1, 0.2, 0.3));
        list.fill_rect(1.0, 2.0, 3.0, 4.0, Rgba::new(1.0, 0.0, 0.0, 0.5));
        assert_eq!(list.commands.len(), 2);

        list.reset();
        assert!(list.commands.is_empty());
    }

    #[test]
    fn renderer_captures_commands_in_order() {
        let mut list = DrawList::new();
        list.clear(Rgba::rgb(0.0, 0.0, 0.0));
        list.fill_rect(0.0, 0.0, 1.0, 1.0, Rgba::rgb(1.0, 1.0, 1.0));
        list.text("hi", 8.0, 9.0, 12.0, Rgba::rgb(0.5, 0.5, 0.5));

        match &list.commands[..] {
            [
                DrawCommand::Clear(_),
                DrawCommand::Rect { x, y, w, h, .. },
                DrawCommand::Text { text, size, .. },
            ] => {
                assert_eq!(*x, 0.0);
                assert_eq!(*y, 0.0);
                assert_eq!(*w, 1.0);
                assert_eq!(*h, 1.0);
                assert_eq!(text, "hi");
                assert_eq!(*size, 12.0);
            }
            _ => panic!("unexpected command sequence"),
        }
    }

    #[test]
    fn null_renderer_swallows_any_list() {
        let mut list = DrawList::new();
        list.fill_rect(0.0, 0.0, 1.0, 1.0, Rgba::rgb(1.0, 1.0, 1.0));
        list.draw_texture(
            TextureHandle::new(std::num::NonZeroU64::new(1).unwrap()),
            0.0,
            0.0,
            1.0,
            1.0,
            Rgba::rgb(1.0, 1.0, 1.0),
        );
        NullRenderer.render(&list);
    }

    /// A texture command names a handle, so it stays a name in the command stream:
    /// copying it never copies the image.
    #[test]
    fn a_texture_command_carries_a_handle_not_a_path() {
        let texture = TextureHandle::new(std::num::NonZeroU64::new(3).unwrap());
        let mut list = DrawList::new();

        list.draw_texture(texture, 4.0, 5.0, 6.0, 7.0, Rgba::rgb(1.0, 0.5, 0.0));

        match &list.commands[..] {
            [
                DrawCommand::Texture {
                    texture: drawn,
                    x,
                    y,
                    w,
                    h,
                    color,
                },
            ] => {
                assert_eq!(*drawn, texture);
                assert_eq!((*x, *y, *w, *h), (4.0, 5.0, 6.0, 7.0));
                assert_eq!(*color, Rgba::rgb(1.0, 0.5, 0.0));
            }
            _ => panic!("a texture draws as one texture command"),
        }
    }
}
