//! Type-specific object properties (chunk 0x4446 inside each object definition).

use super::reader::ByteReader;
use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    Line,
    Rectangle,
    Ellipse,
    Other(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    None,
    Solid([u8; 4]),
    Gradient {
        from: [u8; 4],
        to: [u8; 4],
        vertical: bool,
    },
    /// Tiled image.
    Motif(u16),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub border_size: u16,
    pub border_color: [u8; 4],
    pub kind: ShapeKind,
    pub fill: Fill,
    /// Line direction flags (Line shapes only).
    pub line_flags: u16,
}

#[derive(Debug, Clone)]
pub struct QuickBackdrop {
    pub obstacle: u16,
    pub collision: u16,
    pub width: u32,
    pub height: u32,
    pub shape: Shape,
}

#[derive(Debug, Clone)]
pub struct Backdrop {
    pub obstacle: u16,
    pub collision: u16,
    pub width: u32,
    pub height: u32,
    pub image: u16,
}

#[derive(Debug, Clone)]
pub struct AnimationDirection {
    pub min_speed: u8,
    pub max_speed: u8,
    /// 0 = loop forever.
    pub repeat: u16,
    pub back_to: u16,
    /// Image handles.
    pub frames: Vec<u16>,
}

/// One animation: up to 32 directions (0 = right, 8 = up, 16 = left, 24 = down).
#[derive(Debug, Clone)]
pub struct Animation {
    pub directions: Vec<Option<AnimationDirection>>,
}

impl Animation {
    /// The defined direction closest to `dir` (Fusion falls back to the nearest one).
    pub fn nearest_direction(&self, dir: usize) -> Option<&AnimationDirection> {
        (0..=16usize).find_map(|d| {
            let a = self
                .directions
                .get((dir + d) % 32)?
                .as_ref()
                .filter(|x| !x.frames.is_empty());
            a.or_else(|| {
                self.directions
                    .get((dir + 32 - d) % 32)?
                    .as_ref()
                    .filter(|x| !x.frames.is_empty())
            })
        })
    }
}

pub mod anim {
    pub const STOPPED: usize = 0;
    pub const WALKING: usize = 1;
    pub const RUNNING: usize = 2;
    pub const APPEARING: usize = 3;
    pub const DISAPPEARING: usize = 4;
    pub const JUMPING: usize = 7;
    pub const FALLING: usize = 8;
}

/// Object-common flags (`ObjectCommon::flags`).
pub mod oc_flags {
    pub const DISPLAY_IN_FRONT: u32 = 0x0001;
    pub const BACKGROUND: u32 = 0x0002;
    pub const MOVEMENTS: u32 = 0x0010;
    pub const ANIMATIONS: u32 = 0x0020;
    pub const VALUES: u32 = 0x0100;
    pub const SPRITES: u32 = 0x0200;
    pub const SCROLLING_INDEPENDENT: u32 = 0x0800;
    pub const TEXT: u32 = 0x10000;
}

/// Object-common "new" flags (`ObjectCommon::new_flags`).
pub mod oc_new_flags {
    pub const COLLISION_BOX: u16 = 0x0004;
    pub const VISIBLE_AT_START: u16 = 0x0008;
}

#[derive(Debug, Clone, PartialEq)]
pub enum MovementKind {
    Static,
    Mouse {
        x1: i16,
        x2: i16,
        y1: i16,
        y2: i16,
    },
    Race {
        speed: i16,
        acceleration: i16,
        deceleration: i16,
        rotation_speed: i16,
        bounce: i16,
        angles: i16,
        reverse: i16,
    },
    EightDirections {
        speed: i16,
        acceleration: i16,
        deceleration: i16,
        bounce: i16,
        directions: u32,
    },
    Ball {
        speed: i16,
        randomizer: i16,
        angles: i16,
        security: i16,
        deceleration: i16,
    },
    Path {
        min_speed: i16,
        max_speed: i16,
        looping: bool,
        reposition_at_end: bool,
        reverse_at_end: bool,
        steps: Vec<PathStep>,
    },
    Platform {
        speed: i16,
        acceleration: i16,
        deceleration: i16,
        control: i16,
        gravity: i16,
        jump_strength: i16,
    },
    Extension {
        name: String,
    },
    Other(i16),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathStep {
    pub speed: u8,
    pub direction: u8,
    /// Movement of this step, relative to the end of the previous one.
    pub dx: i16,
    pub dy: i16,
    pub length: i16,
    pub pause: i16,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Movement {
    pub player: i16,
    pub kind: MovementKind,
    pub moving_at_start: bool,
    /// Bitmask of allowed initial directions (bit n = direction n of 32).
    pub direction_at_start: u32,
}

#[derive(Debug, Clone, Default)]
pub struct CounterSettings {
    pub initial: i32,
    pub minimum: i32,
    pub maximum: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CounterDisplayKind {
    Hidden,
    /// Digit images: 0-9, then '-', '+', '.', 'e'.
    Numbers(Vec<u16>),
    VerticalBar(Shape),
    HorizontalBar(Shape),
    /// One image per value step between min and max (Lives: an icon per life).
    Animation(Vec<u16>),
    /// Text in the given font, coloured by the shape's fill.
    Text(Shape),
    Unknown(u16),
}

#[derive(Debug, Clone)]
pub struct CounterDisplay {
    pub width: u32,
    pub height: u32,
    /// 1-based player number (Score / Lives).
    pub player: u16,
    pub flags: u16,
    pub font: u16,
    pub kind: CounterDisplayKind,
}

pub mod paragraph_flags {
    pub const H_CENTER: u16 = 0x1;
    pub const RIGHT: u16 = 0x2;
    pub const V_CENTER: u16 = 0x4;
    pub const BOTTOM: u16 = 0x8;
}

#[derive(Debug, Clone)]
pub struct Paragraph {
    pub font: u16,
    pub flags: u16,
    pub color: [u8; 4],
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct TextBlock {
    pub width: u32,
    pub height: u32,
    pub paragraphs: Vec<Paragraph>,
}

/// Shared header of Actives, Counters, Strings, extensions etc.
#[derive(Debug, Clone, Default)]
pub struct ObjectCommon {
    pub flags: u32,
    pub new_flags: u16,
    pub preferences: u16,
    pub identifier: [u8; 4],
    pub back_color: [u8; 4],
    pub qualifiers: Vec<i16>,
    pub animations: Vec<Option<Animation>>,
    pub movements: Vec<Movement>,
    pub alterable_values: Vec<i32>,
    pub alterable_strings: Vec<String>,
    pub counter: Option<CounterSettings>,
    /// How a Counter / Score / Lives object draws itself.
    pub counter_display: Option<CounterDisplay>,
    /// Paragraphs of a String object.
    pub text: Option<TextBlock>,
    /// Extension objects: their private settings block (after the 20-byte header).
    pub extension_data: Option<Vec<u8>>,
    pub extension_version: u32,
    // Offsets of sub-blocks relative to the start of the property data (0 = absent).
    pub movements_offset: u16,
    pub animations_offset: u16,
    pub counter_offset: u16,
    pub system_offset: u16,
    pub extension_offset: u16,
    pub values_offset: u16,
    pub strings_offset: u16,
    pub fade_in_offset: u32,
    pub fade_out_offset: u32,
}

impl ObjectCommon {
    /// Multimedia Fusion 1.5 layout.
    pub fn parse_old(data: &[u8], type_id: i16, unicode: bool) -> Result<Self> {
        let mut r = ByteReader::new(data);
        let _size = r.read_u16()?;
        let _checksum = r.read_u16()?;
        let mut c = ObjectCommon {
            movements_offset: r.read_u16()?,
            animations_offset: r.read_u16()?,
            ..Default::default()
        };
        let _version = r.read_u16()?;
        c.counter_offset = r.read_u16()?;
        c.system_offset = r.read_u16()?;
        let _variable = r.read_u32()?;
        c.flags = r.read_u16()? as u32;
        c.qualifiers = read_qualifiers(&mut r)?;
        c.extension_offset = r.read_u16()?;
        c.values_offset = r.read_u16()?;
        c.new_flags = r.read_u16()?;
        c.preferences = r.read_u16()?;
        c.identifier = r.read_bytes(4)?.try_into().expect("4 bytes");
        c.back_color = r.read_color()?;
        c.fade_in_offset = r.read_u32()?;
        c.fade_out_offset = r.read_u32()?;
        if c.animations_offset != 0 {
            c.animations = parse_animations(data, c.animations_offset as usize)?;
        }
        if c.movements_offset != 0 {
            // A single movement: player, type, moving at start, 3 pad, direction mask, data.
            let mut r = ByteReader::new(data);
            r.seek(c.movements_offset as usize)?;
            let player = r.read_i16()?;
            let type_id = r.read_i16()?;
            let moving_at_start = r.read_u8()? != 0;
            r.skip(3)?;
            let direction_at_start = r.read_u32()?;
            if let Ok(kind) = read_movement_kind(&mut r, type_id, unicode) {
                c.movements = vec![Movement {
                    player,
                    kind,
                    moving_at_start,
                    direction_at_start,
                }];
            }
        }
        if c.counter_offset != 0 {
            let mut r = ByteReader::new(data);
            r.seek(c.counter_offset as usize)?;
            let _size = r.read_u16()?;
            c.counter = Some(CounterSettings {
                initial: r.read_i32()?,
                minimum: r.read_i32()?,
                maximum: r.read_i32()?,
            });
        }
        if c.extension_offset != 0 && type_id >= 32 {
            let mut r = ByteReader::new(data);
            r.seek(c.extension_offset as usize)?;
            let n = (r.read_u16()? as usize).saturating_sub(8);
            let _max = r.read_u16()?;
            let _old_flags = r.read_u16()?;
            c.extension_version = r.read_u16()? as u32;
            c.extension_data = Some(r.read_bytes(n.min(r.remaining()))?.to_vec());
        }
        if c.system_offset != 0 {
            let base = c.system_offset as usize;
            match type_id {
                5..=7 => c.counter_display = parse_counter_display_old(data, base).ok(),
                3 | 4 => c.text = parse_text_block_old(data, base, unicode).ok(),
                _ => {}
            }
        }
        Ok(c)
    }

    pub fn parse(data: &[u8], type_id: i16, build: u32, unicode: bool) -> Result<Self> {
        let mut r = ByteReader::new(data);
        let _size = r.read_u32()?;
        let mut c = ObjectCommon::default();
        if build >= 284 {
            // Newer runtimes reordered the offset table.
            c.animations_offset = r.read_u16()?;
            c.movements_offset = r.read_u16()?;
            let _version = r.read_u16()?;
            let _unknown = r.read_u16()?;
            c.extension_offset = r.read_u16()?;
            c.counter_offset = r.read_u16()?;
            c.flags = r.read_u32()?;
            c.qualifiers = read_qualifiers(&mut r)?;
            c.system_offset = r.read_u16()?;
        } else {
            c.movements_offset = r.read_u16()?;
            c.animations_offset = r.read_u16()?;
            let _version = r.read_u16()?;
            c.counter_offset = r.read_u16()?;
            c.system_offset = r.read_u16()?;
            r.skip(2)?;
            c.flags = r.read_u32()?;
            c.qualifiers = read_qualifiers(&mut r)?;
            c.extension_offset = r.read_u16()?;
        }
        c.values_offset = r.read_u16()?;
        c.strings_offset = r.read_u16()?;
        c.new_flags = r.read_u16()?;
        c.preferences = r.read_u16()?;
        c.identifier = r.read_bytes(4)?.try_into().expect("4 bytes");
        c.back_color = r.read_color()?;
        c.fade_in_offset = r.read_u32()?;
        c.fade_out_offset = r.read_u32()?;
        if c.animations_offset != 0 {
            c.animations = parse_animations(data, c.animations_offset as usize)?;
        }
        if c.movements_offset != 0 {
            c.movements =
                parse_movements(data, c.movements_offset as usize, unicode).unwrap_or_default();
        }
        if c.values_offset != 0 {
            let mut r = ByteReader::new(data);
            r.seek(c.values_offset as usize)?;
            let n = r.read_u16()?;
            c.alterable_values = (0..n).map(|_| r.read_i32()).collect::<Result<_>>()?;
        }
        if c.strings_offset != 0 {
            let mut r = ByteReader::new(data);
            r.seek(c.strings_offset as usize)?;
            let n = r.read_u16()?;
            c.alterable_strings = (0..n)
                .map(|_| r.read_cstring(unicode))
                .collect::<Result<_>>()?;
        }
        if c.counter_offset != 0 {
            let mut r = ByteReader::new(data);
            r.seek(c.counter_offset as usize)?;
            let _size = r.read_u16()?;
            c.counter = Some(CounterSettings {
                initial: r.read_i32()?,
                minimum: r.read_i32()?,
                maximum: r.read_i32()?,
            });
        }
        if c.extension_offset != 0 && type_id >= 32 {
            // size u32 (header included), max_size u32, version u32, id u32, private u32, data
            let mut r = ByteReader::new(data);
            r.seek(c.extension_offset as usize)?;
            let size = r.read_u32()? as usize;
            let _max = r.read_u32()?;
            c.extension_version = r.read_u32()?;
            let _id = r.read_u32()?;
            let _private = r.read_u32()?;
            let n = size.saturating_sub(20).min(r.remaining());
            c.extension_data = Some(r.read_bytes(n)?.to_vec());
        }
        if c.system_offset != 0 {
            let base = c.system_offset as usize;
            match type_id {
                // Counter, Score, Lives share the counter display block.
                5..=7 => c.counter_display = parse_counter_display(data, base).ok(),
                3 => c.text = parse_text_block(data, base, unicode).ok(),
                _ => {}
            }
        }
        Ok(c)
    }

    pub fn visible_at_start(&self) -> bool {
        self.new_flags & oc_new_flags::VISIBLE_AT_START != 0
    }

    pub fn identifier_str(&self) -> String {
        self.identifier.iter().map(|&b| b as char).collect()
    }
}

/// Border / shape / fill block shared by Quick Backdrops and counter bars.
fn read_shape(r: &mut ByteReader) -> Result<Shape> {
    read_shape_v(r, false)
}

fn read_shape_v(r: &mut ByteReader, old: bool) -> Result<Shape> {
    let border_size = r.read_u16()?;
    let border_color = r.read_color()?;
    let kind_id = r.read_u16()?;
    let fill_id = r.read_u16()?;
    let kind = match kind_id {
        1 => ShapeKind::Line,
        2 => ShapeKind::Rectangle,
        3 => ShapeKind::Ellipse,
        k => ShapeKind::Other(k),
    };
    let mut line_flags = 0;
    let fill = if kind == ShapeKind::Line {
        line_flags = r.read_u16()?;
        Fill::None
    } else {
        match fill_id {
            1 => Fill::Solid(r.read_color()?),
            2 => Fill::Gradient {
                from: r.read_color()?,
                to: r.read_color()?,
                vertical: if old {
                    r.read_u16()? as u32
                } else {
                    r.read_u32()?
                } != 0,
            },
            3 => Fill::Motif(r.read_u16()?),
            _ => Fill::None,
        }
    };
    Ok(Shape {
        border_size,
        border_color,
        kind,
        fill,
        line_flags,
    })
}

fn parse_counter_display(data: &[u8], base: usize) -> Result<CounterDisplay> {
    let mut r = ByteReader::new(data);
    r.seek(base)?;
    let _size = r.read_u32()?;
    let width = r.read_u32()?;
    let height = r.read_u32()?;
    let player = r.read_u16()?;
    let display = r.read_u16()?;
    let flags = r.read_u16()?;
    let font = r.read_u16()?;
    let images = |r: &mut ByteReader| -> Result<Vec<u16>> {
        let n = r.read_u16()?;
        (0..n).map(|_| r.read_u16()).collect()
    };
    let kind = match display {
        0 => CounterDisplayKind::Hidden,
        1 => CounterDisplayKind::Numbers(images(&mut r)?),
        2 => CounterDisplayKind::VerticalBar(read_shape(&mut r)?),
        3 => CounterDisplayKind::HorizontalBar(read_shape(&mut r)?),
        4 => CounterDisplayKind::Animation(images(&mut r)?),
        5 => CounterDisplayKind::Text(read_shape(&mut r)?),
        k => CounterDisplayKind::Unknown(k),
    };
    Ok(CounterDisplay {
        width,
        height,
        player,
        flags,
        font,
        kind,
    })
}

fn parse_counter_display_old(data: &[u8], base: usize) -> Result<CounterDisplay> {
    let mut r = ByteReader::new(data);
    r.seek(base)?;
    let _size = r.read_u32()?;
    let width = r.read_u16()? as u32;
    let height = r.read_u16()? as u32;
    let player = r.read_u16()?;
    let display = r.read_u16()?;
    let flags = r.read_u16()?;
    let images = |r: &mut ByteReader| -> Result<Vec<u16>> {
        let n = r.read_u16()?;
        (0..n).map(|_| r.read_u16()).collect()
    };
    let kind = match display {
        0 => CounterDisplayKind::Hidden,
        1 => CounterDisplayKind::Numbers(images(&mut r)?),
        2 => CounterDisplayKind::VerticalBar(read_shape_v(&mut r, true)?),
        3 => CounterDisplayKind::HorizontalBar(read_shape_v(&mut r, true)?),
        4 => CounterDisplayKind::Animation(images(&mut r)?),
        k => CounterDisplayKind::Unknown(k),
    };
    Ok(CounterDisplay {
        width,
        height,
        player,
        flags,
        font: 0,
        kind,
    })
}

fn parse_text_block_old(data: &[u8], base: usize, unicode: bool) -> Result<TextBlock> {
    let mut r = ByteReader::new(data);
    r.seek(base)?;
    let _size = r.read_u32()?;
    let width = r.read_u16()? as u32;
    let height = r.read_u16()? as u32;
    let n = r.read_u16()? as usize;
    let offsets: Vec<u16> = (0..n).map(|_| r.read_u16()).collect::<Result<_>>()?;
    let mut paragraphs = Vec::with_capacity(n);
    for off in offsets {
        r.seek(base + off as usize)?;
        let _size = r.read_u16()?;
        let font = r.read_u16()?;
        let color = r.read_color()?;
        let flags = r.read_u16()?;
        let text = r.read_cstring(unicode)?;
        paragraphs.push(Paragraph {
            font,
            flags,
            color,
            text,
        });
    }
    Ok(TextBlock {
        width,
        height,
        paragraphs,
    })
}

fn parse_text_block(data: &[u8], base: usize, unicode: bool) -> Result<TextBlock> {
    let mut r = ByteReader::new(data);
    r.seek(base)?;
    let _size = r.read_u32()?;
    let width = r.read_u32()?;
    let height = r.read_u32()?;
    let n = r.read_u32()? as usize;
    if n > 10_000 {
        anyhow::bail!("implausible paragraph count {n}");
    }
    let offsets: Vec<u32> = (0..n).map(|_| r.read_u32()).collect::<Result<_>>()?;
    let mut paragraphs = Vec::with_capacity(n);
    for off in offsets {
        r.seek(base + off as usize)?;
        paragraphs.push(Paragraph {
            font: r.read_u16()?,
            flags: r.read_u16()?,
            color: r.read_color()?,
            text: r.read_cstring(unicode)?,
        });
    }
    Ok(TextBlock {
        width,
        height,
        paragraphs,
    })
}

fn parse_movements(data: &[u8], base: usize, unicode: bool) -> Result<Vec<Movement>> {
    let mut r = ByteReader::new(data);
    r.seek(base)?;
    let count = r.read_u32()? as usize;
    if count > 64 {
        anyhow::bail!("implausible movement count {count}");
    }
    let table = r.pos();
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        r.seek(table + 16 * i)?;
        let name_offset = r.read_u32()? as usize;
        let _id = r.read_u32()?;
        let new_offset = r.read_u32()? as usize;
        let _data_size = r.read_u32()?;
        r.seek(base + new_offset)?;
        let player = r.read_i16()?;
        let type_id = r.read_i16()?;
        let moving_at_start = r.read_u8()? != 0;
        r.skip(3)?;
        let direction_at_start = r.read_u32()?;
        let mut kind = read_movement_kind(&mut r, type_id, unicode)?;
        if let MovementKind::Extension { name } = &mut kind {
            r.seek(base + name_offset)?;
            *name = r.read_cstring(unicode).unwrap_or_default();
        }
        out.push(Movement {
            player,
            kind,
            moving_at_start,
            direction_at_start,
        });
    }
    Ok(out)
}

/// Movement-type-specific data (shared by 1.5 and 2.x).
fn read_movement_kind(r: &mut ByteReader, type_id: i16, unicode: bool) -> Result<MovementKind> {
    Ok(match type_id {
        0 => MovementKind::Static,
        1 => MovementKind::Mouse {
            x1: r.read_i16()?,
            x2: r.read_i16()?,
            y1: r.read_i16()?,
            y2: r.read_i16()?,
        },
        2 => MovementKind::Race {
            speed: r.read_i16()?,
            acceleration: r.read_i16()?,
            deceleration: r.read_i16()?,
            rotation_speed: r.read_i16()?,
            bounce: r.read_i16()?,
            angles: r.read_i16()?,
            reverse: r.read_i16()?,
        },
        3 => MovementKind::EightDirections {
            speed: r.read_i16()?,
            acceleration: r.read_i16()?,
            deceleration: r.read_i16()?,
            bounce: r.read_i16()?,
            directions: r.read_u32()?,
        },
        4 => MovementKind::Ball {
            speed: r.read_i16()?,
            randomizer: r.read_i16()?,
            angles: r.read_i16()?,
            security: r.read_i16()?,
            deceleration: r.read_i16()?,
        },
        5 => {
            let n = r.read_i16()?.max(0) as usize;
            let min_speed = r.read_i16()?;
            let max_speed = r.read_i16()?;
            let looping = r.read_u8()? != 0;
            let reposition_at_end = r.read_u8()? != 0;
            let reverse_at_end = r.read_u8()? != 0;
            r.skip(1)?;
            let mut steps = Vec::with_capacity(n);
            for _ in 0..n {
                let start = r.pos();
                r.skip(1)?;
                let size = r.read_u8()? as usize;
                let speed = r.read_u8()?;
                let direction = r.read_u8()?;
                let dx = r.read_i16()?;
                let dy = r.read_i16()?;
                r.skip(4)?; // cosine, sine (x16384)
                let length = r.read_i16()?;
                let pause = r.read_i16()?;
                let name = if r.pos() < start + size {
                    r.read_cstring(unicode).unwrap_or_default()
                } else {
                    String::new()
                };
                steps.push(PathStep {
                    speed,
                    direction,
                    dx,
                    dy,
                    length,
                    pause,
                    name,
                });
                r.seek(start + size.max(2))?;
            }
            MovementKind::Path {
                min_speed,
                max_speed,
                looping,
                reposition_at_end,
                reverse_at_end,
                steps,
            }
        }
        9 => MovementKind::Platform {
            speed: r.read_i16()?,
            acceleration: r.read_i16()?,
            deceleration: r.read_i16()?,
            control: r.read_i16()?,
            gravity: r.read_i16()?,
            jump_strength: r.read_i16()?,
        },
        14 => MovementKind::Extension {
            name: String::new(),
        },
        t => MovementKind::Other(t),
    })
}

fn read_qualifiers(r: &mut ByteReader) -> Result<Vec<i16>> {
    let mut q = Vec::new();
    for _ in 0..8 {
        let v = r.read_i16()?;
        if v != -1 {
            q.push(v);
        }
    }
    Ok(q)
}

fn parse_animations(data: &[u8], base: usize) -> Result<Vec<Option<Animation>>> {
    let mut r = ByteReader::new(data);
    r.seek(base)?;
    let _size = r.read_u16()?;
    let count = r.read_u16()? as usize;
    let offsets: Vec<i16> = (0..count).map(|_| r.read_i16()).collect::<Result<_>>()?;
    let mut anims = Vec::with_capacity(count);
    for off in offsets {
        if off <= 0 {
            anims.push(None);
            continue;
        }
        let abase = base + off as usize;
        r.seek(abase)?;
        let dir_offsets: Vec<i16> = (0..32).map(|_| r.read_i16()).collect::<Result<_>>()?;
        let mut directions = Vec::with_capacity(32);
        for doff in dir_offsets {
            if doff <= 0 {
                directions.push(None);
                continue;
            }
            r.seek(abase + doff as usize)?;
            let min_speed = r.read_u8()?;
            let max_speed = r.read_u8()?;
            let repeat = r.read_u16()?;
            let back_to = r.read_u16()?;
            let n = r.read_u16()? as usize;
            let frames = (0..n).map(|_| r.read_u16()).collect::<Result<_>>()?;
            directions.push(Some(AnimationDirection {
                min_speed,
                max_speed,
                repeat,
                back_to,
                frames,
            }));
        }
        anims.push(Some(Animation { directions }));
    }
    Ok(anims)
}

#[derive(Debug, Clone)]
pub enum ObjectProperties {
    QuickBackdrop(QuickBackdrop),
    Backdrop(Backdrop),
    Common(Box<ObjectCommon>),
    None,
}

pub fn parse_properties(
    type_id: i16,
    data: &[u8],
    build: u32,
    unicode: bool,
    old: bool,
) -> Result<ObjectProperties> {
    let mut r = ByteReader::new(data);
    if old {
        return Ok(match type_id {
            0 => {
                let _size = r.read_u32()?;
                let obstacle = r.read_u16()?;
                let collision = r.read_u16()?;
                let width = r.read_u16()? as u32;
                let height = r.read_u16()? as u32;
                let shape = read_shape_v(&mut r, true)?;
                ObjectProperties::QuickBackdrop(QuickBackdrop {
                    obstacle,
                    collision,
                    width,
                    height,
                    shape,
                })
            }
            1 => {
                let _size = r.read_u32()?;
                ObjectProperties::Backdrop(Backdrop {
                    obstacle: r.read_u16()?,
                    collision: r.read_u16()?,
                    width: 0,
                    height: 0,
                    image: r.read_u16()?,
                })
            }
            t if t >= 2 => {
                ObjectProperties::Common(Box::new(ObjectCommon::parse_old(data, t, unicode)?))
            }
            _ => ObjectProperties::None,
        });
    }
    Ok(match type_id {
        0 => {
            let _size = r.read_u32()?;
            let obstacle = r.read_u16()?;
            let collision = r.read_u16()?;
            let width = r.read_u32()?;
            let height = r.read_u32()?;
            let shape = read_shape(&mut r)?;
            ObjectProperties::QuickBackdrop(QuickBackdrop {
                obstacle,
                collision,
                width,
                height,
                shape,
            })
        }
        1 => {
            let _size = r.read_u32()?;
            ObjectProperties::Backdrop(Backdrop {
                obstacle: r.read_u16()?,
                collision: r.read_u16()?,
                width: r.read_u32()?,
                height: r.read_u32()?,
                image: r.read_u16()?,
            })
        }
        t if t >= 2 => {
            ObjectProperties::Common(Box::new(ObjectCommon::parse(data, t, build, unicode)?))
        }
        _ => ObjectProperties::None,
    })
}
