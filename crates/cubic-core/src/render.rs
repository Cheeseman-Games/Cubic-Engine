use std::borrow::Cow;

use crate::assets::TextureHandle;

/// RGBA color, components normalized to 0.0..=1.0.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

/// How a textured quad samples its image.
///
/// Chosen per quad rather than per texture, so one image can be drawn sharp in a
/// pixel-art sprite and smooth in a scaled backdrop. The default is
/// [`Linear`](Self::Linear), which is what an image drawn at a size it was not
/// authored at wants; [`Nearest`](Self::Nearest) keeps hard texel edges.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextureFilter {
    /// Pick the nearest texel: sharp and blocky when scaled up.
    Nearest,
    /// Blend neighbouring texels: smooth when scaled.
    #[default]
    Linear,
}

impl TextureFilter {
    /// Whether this is [`Nearest`](Self::Nearest).
    pub const fn is_nearest(self) -> bool {
        matches!(self, Self::Nearest)
    }
}

// The reflection vocabulary lives behind `reflect`; the type itself is part of
// the render model, so this is where its selector is defined rather than in
// `components`, where the sprite that uses it lives.
#[cfg(feature = "reflect")]
impl crate::reflect::Selectable for TextureFilter {
    fn index(self) -> usize {
        match self {
            Self::Nearest => 0,
            Self::Linear => 1,
        }
    }

    fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Nearest,
            _ => Self::Linear,
        }
    }
}

/// A region of a texture, in normalized UV coordinates.
///
/// `(0, 0)` is the image's top-left and `(1, 1)` its bottom-right — the row order
/// the decoder hands over — so the whole image is [`WHOLE`](Self::WHOLE). A
/// sprite sheet slices an image into these, and a flip is a region with its
/// `u`s or `v`s swapped, so a mirrored sprite is still one instance with no
/// extra state to carry.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvRect {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

impl UvRect {
    /// The whole image.
    pub const WHOLE: Self = Self {
        u0: 0.0,
        v0: 0.0,
        u1: 1.0,
        v1: 1.0,
    };

    /// A region from its two opposite corners.
    pub const fn new(u0: f32, v0: f32, u1: f32, v1: f32) -> Self {
        Self { u0, v0, u1, v1 }
    }

    /// This region mirrored horizontally and/or vertically.
    ///
    /// Mirroring swaps the region's edges: the sample then runs the other way
    /// across the same texels, which is exactly what a flipped sprite draws.
    pub fn flipped(mut self, flip_x: bool, flip_y: bool) -> Self {
        if flip_x {
            core::mem::swap(&mut self.u0, &mut self.u1);
        }
        if flip_y {
            core::mem::swap(&mut self.v0, &mut self.v1);
        }
        self
    }

    /// The region as the `u0, v0, u1, v1` array the instance data and the shader
    /// speak.
    pub const fn as_array(self) -> [f32; 4] {
        [self.u0, self.v0, self.u1, self.v1]
    }
}

impl Default for UvRect {
    fn default() -> Self {
        Self::WHOLE
    }
}

/// A textured quad: a whole image or a region of one, tinted, placed and
/// rotated.
///
/// The richer form of a texture draw, and the value a [`Sprite`](crate::components::Sprite)
/// becomes once a backend can place it. Every field a backend needs to draw the
/// quad is here — the destination rectangle, the source [`UvRect`], a rotation
/// about the rectangle's centre, and the [`TextureFilter`] — so a rotated,
/// flipped, region-sliced sprite is still one instanced quad and the batching
/// stays a single draw per texture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexturedQuad {
    /// The image to sample.
    pub texture: TextureHandle,
    /// Left edge of the destination rectangle, in the caller's space.
    pub x: f32,
    /// Top edge of the destination rectangle.
    pub y: f32,
    /// Width of the destination rectangle.
    pub w: f32,
    /// Height of the destination rectangle.
    pub h: f32,
    /// Rotation about the rectangle's centre, in radians, counter-clockwise in
    /// the world's right-handed sense — which is clockwise on a y-down screen.
    pub rotation: f32,
    /// The region of the texture to sample; a flip is a swapped [`UvRect`].
    pub uv: UvRect,
    /// How the region is sampled.
    pub filter: TextureFilter,
    /// Multiplied into the sampled color; white leaves the image as it is.
    pub color: Rgba,
}

impl TexturedQuad {
    /// The whole image, unrotated and linearly filtered, tinted by `color`:
    /// what [`Renderer::draw_texture`] draws.
    pub const fn whole(
        texture: TextureHandle,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgba,
    ) -> Self {
        Self {
            texture,
            x,
            y,
            w,
            h,
            rotation: 0.0,
            uv: UvRect::WHOLE,
            filter: TextureFilter::Linear,
            color,
        }
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
    /// A textured quad: a whole image or a region of one, tinted, placed and
    /// optionally rotated.
    ///
    /// The handle, not a path: a backend that has not imported it yet cannot draw
    /// it and must skip the command, and one whose bytes have since been replaced
    /// draws the new bytes without the game saying anything. A plain image is a
    /// [`TexturedQuad::whole`], which is what [`Renderer::draw_texture`]
    /// produces.
    Texture(TexturedQuad),
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

    /// Draw a whole texture, stretched into `x, y, w, h` and tinted by `color`.
    ///
    /// The shorthand for a plain image: the same as
    /// [`draw_textured`](Self::draw_textured) with a whole-region, unrotated,
    /// linearly filtered quad.
    fn draw_texture(
        &mut self,
        texture: TextureHandle,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgba,
    ) {
        self.draw_textured(TexturedQuad::whole(texture, x, y, w, h, color));
    }

    /// Draw a textured quad: a region of an image, tinted, rotated and filtered.
    ///
    /// The general form of [`draw_texture`](Self::draw_texture): it carries the
    /// source [`UvRect`], a rotation about the quad's centre and the
    /// [`TextureFilter`], so a sprite can slice a sheet, flip, spin and pick its
    /// sampling without a new command per combination.
    fn draw_textured(&mut self, quad: TexturedQuad);
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

    fn draw_textured(&mut self, quad: TexturedQuad) {
        self.commands.push(DrawCommand::Texture(quad));
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
            [DrawCommand::Texture(quad)] => {
                assert_eq!(quad.texture, texture);
                assert_eq!((quad.x, quad.y, quad.w, quad.h), (4.0, 5.0, 6.0, 7.0));
                assert_eq!(quad.color, Rgba::rgb(1.0, 0.5, 0.0));
                // The shorthand draws the whole image, unrotated and smooth.
                assert_eq!(quad.uv, UvRect::WHOLE);
                assert_eq!(quad.rotation, 0.0);
                assert_eq!(quad.filter, TextureFilter::Linear);
            }
            _ => panic!("a texture draws as one texture command"),
        }
    }

    /// The general draw keeps a region, rotation and filter intact, which is
    /// everything S4.1's richer sprite needs.
    #[test]
    fn a_textured_quad_carries_its_region_rotation_and_filter() {
        let texture = TextureHandle::new(std::num::NonZeroU64::new(5).unwrap());
        let quad = TexturedQuad {
            texture,
            x: 1.0,
            y: 2.0,
            w: 3.0,
            h: 4.0,
            rotation: 0.5,
            uv: UvRect::new(0.25, 0.0, 0.75, 0.5),
            filter: TextureFilter::Nearest,
            color: Rgba::rgb(0.5, 1.0, 0.5),
        };
        let mut list = DrawList::new();

        list.draw_textured(quad);

        match &list.commands[..] {
            [DrawCommand::Texture(drawn)] => assert_eq!(*drawn, quad),
            _ => panic!("a textured quad draws as one texture command"),
        }
    }

    #[test]
    fn flipping_a_region_swaps_its_edges() {
        let region = UvRect::new(0.25, 0.1, 0.75, 0.6);
        assert_eq!(region, region.flipped(false, false), "no flip is a no-op");
        assert_eq!(
            region.flipped(true, false),
            UvRect::new(0.75, 0.1, 0.25, 0.6),
            "a horizontal flip swaps the u edges"
        );
        assert_eq!(
            region.flipped(false, true),
            UvRect::new(0.25, 0.6, 0.75, 0.1),
            "a vertical flip swaps the v edges"
        );
        assert_eq!(
            region.flipped(true, true),
            UvRect::new(0.75, 0.6, 0.25, 0.1),
            "both flips swap both pairs"
        );
    }

    #[test]
    fn a_region_reports_the_array_the_shader_reads() {
        assert_eq!(UvRect::WHOLE.as_array(), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(
            UvRect::new(0.1, 0.2, 0.3, 0.4).as_array(),
            [0.1, 0.2, 0.3, 0.4]
        );
        assert_eq!(UvRect::default(), UvRect::WHOLE);
    }

    #[test]
    fn whichever_filter_is_the_default_only_nearest_is_nearest() {
        assert_eq!(TextureFilter::default(), TextureFilter::Linear);
        assert!(!TextureFilter::Linear.is_nearest());
        assert!(TextureFilter::Nearest.is_nearest());
    }
}
