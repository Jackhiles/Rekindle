//! Draw-list types shared by the runtime (which produces them) and the renderer.

use crate::ctf_loader::properties::Shape;

#[derive(Debug, Clone)]
pub enum Visual {
    /// An image from the bank, drawn with its hotspot at the item position.
    Image { handle: u32 },
    /// A Quick Backdrop shape filling `w` x `h`.
    Shape { w: f32, h: f32, shape: Shape },
    /// Text laid out in a `w` x `h` box (top-left at the item position).
    Text {
        w: f32,
        h: f32,
        text: String,
        font: Option<u32>,
        color: [u8; 4],
        flags: u16,
        size: Option<f32>,
    },
    /// A row of images (counter digits, lives icons) whose bottom-right corner is the item position.
    Glyphs { handles: Vec<u32> },
    /// A screen-space distortion of everything drawn before it (the Perspective object).
    Effect {
        w: f32,
        h: f32,
        params: EffectParams,
    },
    /// An edit box, list, button or combo box.
    Widget(Box<crate::runtime::widgets::Widget>),
    /// Something we can't draw (extensions); shown only in debug view.
    Placeholder { w: f32, h: f32 },
}

/// Settings of a Perspective-style distortion.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EffectParams {
    /// 0 panorama, 1 perspective, 2 sine wave, 3 sine offset.
    pub effect: u8,
    pub vertical: bool,
    pub zoom: f32,
    pub offset: f32,
    pub waves: f32,
}

impl EffectParams {
    /// Parse the Perspective object's settings block:
    /// `u32 ?, u16 width, u16 height, u8 effect, u8 direction, u16 pad, i32 zoom, i32 offset, i32 waves, ...`
    pub fn from_perspective_data(d: &[u8]) -> Option<(Self, f32, f32)> {
        let u16_at = |o: usize| d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        let i32_at = |o: usize| {
            d.get(o..o + 4)
                .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let w = u16_at(4)? as f32;
        let h = u16_at(6)? as f32;
        let p = Self {
            effect: *d.get(8)?,
            vertical: *d.get(9)? != 0,
            zoom: i32_at(12)? as f32,
            offset: i32_at(16)? as f32,
            waves: i32_at(20)? as f32,
        };
        Some((p, w, h))
    }
}

#[derive(Debug, Clone)]
pub struct SceneItem {
    pub layer: usize,
    pub x: f32,
    pub y: f32,
    pub visual: Visual,
    /// Multiplied into the image: RGB tint + opacity (0..=255).
    pub tint: [u8; 4],
    pub ink_effect: u16,
    pub visible: bool,
    /// Backdrop-type objects sit behind sprites within their layer.
    pub background: bool,
    pub label: String,
    pub scale: (f32, f32),
    /// Degrees, counter-clockwise around the hotspot.
    pub angle: f32,
    /// Ignores scrolling (drawn at screen coordinates).
    pub fixed: bool,
}
