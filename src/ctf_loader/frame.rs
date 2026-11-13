//! Frames (chunk 0x3333) and their sub-chunks.

use super::chunk::{id, read_chunk_list, ChunkDecoder};
use super::events::{parse_events, Events};
use super::reader::{decode_cstr, Blob, ByteReader};
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct Layer {
    pub flags: u32,
    pub x_coefficient: f32,
    pub y_coefficient: f32,
    pub background_count: u32,
    pub background_index: u32,
    pub name: String,
}

impl Layer {
    pub const VISIBLE: u32 = 0x10;
    pub const X_COEF: u32 = 0x01;
    pub const Y_COEF: u32 = 0x02;

    /// How much of the frame's scroll this layer follows (1 = all, 0 = fixed on screen).
    pub fn scroll_coefficients(&self) -> (f32, f32) {
        let cx = if self.flags & Self::X_COEF != 0 { self.x_coefficient } else { 1.0 };
        let cy = if self.flags & Self::Y_COEF != 0 { self.y_coefficient } else { 1.0 };
        (cx, cy)
    }
}

#[derive(Debug, Clone)]
pub struct ObjectInstance {
    /// Unique instance handle within the frame.
    pub handle: u16,
    /// Handle of the object definition (`ObjectInfo::handle`).
    pub object_info: u16,
    pub x: i32,
    pub y: i32,
    pub parent_type: i16,
    pub parent_handle: i16,
    pub layer: i16,
}

/// A frame fade-in / fade-out transition (chunks 0x333B / 0x333C).
#[derive(Debug, Clone)]
pub struct Transition {
    /// Transition DLL id, e.g. "STDT".
    pub module: String,
    /// Effect id within the module, e.g. "FADE".
    pub name: String,
    pub duration_ms: u32,
    pub flags: u32,
    /// Colour to fade from / to when `flags & 1`.
    pub color: [u8; 4],
    pub module_file: String,
}

impl Transition {
    pub const FLAG_COLOR: u32 = 1;

    fn parse(b: &[u8], unicode: bool) -> Result<Self> {
        let mut r = ByteReader::new(b);
        let module = super::reader::latin1(r.read_bytes(4)?);
        let name = super::reader::latin1(r.read_bytes(4)?);
        let duration_ms = r.read_i32()?.max(0) as u32;
        let flags = r.read_u32()?;
        let color = r.read_color()?;
        let name_offset = r.read_u32()? as usize;
        let module_file = if name_offset > 0 && name_offset < b.len() {
            r.seek(name_offset)?;
            r.read_cstring(unicode).unwrap_or_default()
        } else {
            String::new()
        };
        Ok(Self {
            module,
            name,
            duration_ms,
            flags,
            color,
            module_file,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub name: String,
    pub width: i32,
    pub height: i32,
    /// R, G, B, A
    pub background_color: [u8; 4],
    pub flags: u32,
    pub password: Option<String>,
    pub virtual_rect: Option<[i32; 4]>,
    pub layers: Vec<Layer>,
    pub instances: Vec<ObjectInstance>,
    pub fade_in: Option<Transition>,
    pub fade_out: Option<Transition>,
    pub random_seed: Option<i16>,
    pub movement_timer_base: Option<u32>,
    /// Raw event block.
    pub events: Option<Blob>,
    /// Parsed event sheet.
    pub event_sheet: Events,
    /// 256-colour palette (palette-based games).
    pub palette: Option<Vec<[u8; 4]>>,
}

impl Frame {
    pub fn parse(
        data: &Blob,
        index: usize,
        unicode: bool,
        old: bool,
        dec: &ChunkDecoder,
        warnings: &mut Vec<String>,
    ) -> Result<Self> {
        let (chunks, _) = read_chunk_list(data, 0)?;
        let mut f = Frame::default();
        for c in &chunks {
            let body = match dec.decode(c) {
                Ok(b) => b,
                Err(e) => {
                    warnings.push(format!("frame #{index}: {e:#}"));
                    continue;
                }
            };
            let res: Result<()> = (|| {
                let mut r = ByteReader::new(&body);
                match c.id {
                    id::FRAME_HEADER if old => {
                        f.width = r.read_u16()? as i32;
                        f.height = r.read_u16()? as i32;
                        f.background_color = r.read_color()?;
                        f.flags = r.read_u16()? as u32;
                    }
                    id::FRAME_HEADER => {
                        f.width = r.read_i32()?;
                        f.height = r.read_i32()?;
                        f.background_color = r.read_color()?;
                        f.flags = r.read_u32()?;
                    }
                    id::FRAME_PALETTE => {
                        let _version = r.read_u16()?;
                        let n = r.read_u16()?.min(256);
                        f.palette = Some((0..n).map(|_| r.read_color()).collect::<Result<_>>()?);
                    }
                    id::FRAME_ITEM_INSTANCES if old => {
                        let n = r.read_u32()?;
                        for _ in 0..n {
                            f.instances.push(ObjectInstance {
                                handle: r.read_u16()?,
                                object_info: r.read_u16()?,
                                x: r.read_i16()? as i32,
                                y: r.read_i16()? as i32,
                                parent_type: r.read_i16()?,
                                parent_handle: r.read_i16()?,
                                layer: 0,
                            });
                        }
                    }
                    id::FRAME_NAME => f.name = decode_cstr(&body, unicode),
                    id::FRAME_PASSWORD => f.password = Some(decode_cstr(&body, unicode)),
                    id::FRAME_VIRTUAL_RECT => {
                        f.virtual_rect =
                            Some([r.read_i32()?, r.read_i32()?, r.read_i32()?, r.read_i32()?]);
                    }
                    id::FRAME_LAYERS => {
                        let n = r.read_u32()?;
                        for _ in 0..n {
                            f.layers.push(Layer {
                                flags: r.read_u32()?,
                                x_coefficient: r.read_f32()?,
                                y_coefficient: r.read_f32()?,
                                background_count: r.read_u32()?,
                                background_index: r.read_u32()?,
                                name: r.read_cstring(unicode)?,
                            });
                        }
                    }
                    id::FRAME_ITEM_INSTANCES => {
                        let n = r.read_u32()?;
                        for _ in 0..n {
                            let inst = ObjectInstance {
                                handle: r.read_u16()?,
                                object_info: r.read_u16()?,
                                x: r.read_i32()?,
                                y: r.read_i32()?,
                                parent_type: r.read_i16()?,
                                parent_handle: r.read_i16()?,
                                layer: r.read_i16()?,
                            };
                            r.skip(2)?;
                            f.instances.push(inst);
                        }
                    }
                    id::FRAME_FADE_IN => f.fade_in = Some(Transition::parse(&body, unicode)?),
                    id::FRAME_FADE_OUT => f.fade_out = Some(Transition::parse(&body, unicode)?),
                    id::FRAME_RANDOM_SEED => f.random_seed = Some(r.read_i16()?),
                    id::FRAME_MOVE_TIMER => f.movement_timer_base = Some(r.read_u32()?),
                    id::FRAME_EVENTS => {
                        f.event_sheet = parse_events(&body, unicode, old)?;
                        f.events = Some(body.clone());
                    }
                    _ => {}
                }
                Ok(())
            })();
            if let Err(e) = res {
                warnings.push(format!("frame #{index} chunk {:#06x}: {e:#}", c.id));
            }
        }
        if f.layers.is_empty() {
            // Multimedia Fusion 1.5 frames have a single implicit layer.
            f.layers.push(Layer {
                flags: 0x10,
                x_coefficient: 1.0,
                y_coefficient: 1.0,
                background_count: 0,
                background_index: 0,
                name: "Layer 1".into(),
            });
        }
        Ok(f)
    }
}
