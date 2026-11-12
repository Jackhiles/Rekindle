//! Object definitions ("frame items", chunk 0x2229) and the extension list (0x2234).

use super::chunk::{id, read_chunk_list, ChunkDecoder};
use super::properties::{parse_properties, ObjectProperties};
use super::reader::{decode_cstr, Blob, ByteReader};
use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObjectKind {
    QuickBackdrop,
    Backdrop,
    Active,
    Text,
    Question,
    Score,
    Lives,
    Counter,
    Rtf,
    SubApplication,
    /// Extension object; type id >= 32, extension index = type id - 32.
    Extension(i16),
    Unknown(i16),
}

impl ObjectKind {
    pub fn from_type_id(t: i16) -> Self {
        match t {
            0 => Self::QuickBackdrop,
            1 => Self::Backdrop,
            2 => Self::Active,
            3 => Self::Text,
            4 => Self::Question,
            5 => Self::Score,
            6 => Self::Lives,
            7 => Self::Counter,
            8 => Self::Rtf,
            9 => Self::SubApplication,
            t if t >= 32 => Self::Extension(t),
            t => Self::Unknown(t),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::QuickBackdrop => "Quick Backdrop".into(),
            Self::Backdrop => "Backdrop".into(),
            Self::Active => "Active".into(),
            Self::Text => "String".into(),
            Self::Question => "Question & Answer".into(),
            Self::Score => "Score".into(),
            Self::Lives => "Lives".into(),
            Self::Counter => "Counter".into(),
            Self::Rtf => "Formatted Text".into(),
            Self::SubApplication => "Sub-Application".into(),
            Self::Extension(t) => format!("Extension #{}", t - 32),
            Self::Unknown(t) => format!("Unknown type {t}"),
        }
    }
}

/// Name of a built-in "system" object type used by events (negative type ids).
pub fn system_type_name(t: i16) -> Option<&'static str> {
    Some(match t {
        -7 => "Player",
        -6 => "Keyboard",
        -5 => "Create",
        -4 => "Timer",
        -3 => "Game",
        -2 => "Speaker",
        -1 => "System",
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub struct ObjectInfo {
    pub handle: u16,
    pub type_id: i16,
    pub kind: ObjectKind,
    pub name: String,
    pub flags: u16,
    pub ink_effect: u32,
    pub ink_effect_param: u32,
    /// Raw type-specific property block.
    pub properties: Option<Blob>,
    /// Decoded properties (shape, backdrop image, animations, ...).
    pub props: ObjectProperties,
}

/// Decoded ink effect of an object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ink {
    /// 0 none, 1 semi-transparent, 2 inverted, 3 XOR, 4 AND, 5 OR, 9 add, 11 subtract, 13 shader...
    pub effect: u16,
    /// Tint multiplied into the image (R, G, B) and opacity, all 0..=255.
    pub rgba: [u8; 4],
}

pub const INK_RGB_COEFFICIENT: u32 = 0x1000;

impl ObjectInfo {
    pub fn ink(&self) -> Ink {
        let effect = (self.ink_effect & 0x0FFF) as u16;
        let p = self.ink_effect_param;
        let mut rgba = [255u8; 4];
        if self.ink_effect & INK_RGB_COEFFICIENT != 0 {
            // Parameter bytes are B, G, R, A.
            let b = p.to_le_bytes();
            rgba = [b[2], b[1], b[0], b[3]];
        } else if effect == 1 {
            // Semi-transparent: 0 = opaque .. 128 = invisible.
            let t = p.min(128);
            rgba[3] = (255 * (128 - t) / 128) as u8;
        }
        Ink { effect, rgba }
    }
}

pub fn parse_frame_items(
    data: &Blob,
    unicode: bool,
    build: u32,
    old: bool,
    dec: &ChunkDecoder,
    warnings: &mut Vec<String>,
) -> Result<Vec<ObjectInfo>> {
    let count = ByteReader::new(data).read_u32()? as usize;
    let mut pos = 4;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (chunks, next) =
            read_chunk_list(data, pos).with_context(|| format!("object definition #{i}"))?;
        pos = next;
        let mut obj = ObjectInfo {
            handle: 0,
            type_id: 0,
            kind: ObjectKind::Unknown(0),
            name: String::new(),
            flags: 0,
            ink_effect: 0,
            ink_effect_param: 0,
            properties: None,
            props: ObjectProperties::None,
        };
        let mut have_header = false;
        for c in &chunks {
            let body = match dec.decode(c) {
                Ok(b) => b,
                Err(e) => {
                    warnings.push(format!("object #{i}: {e:#}"));
                    continue;
                }
            };
            match c.id {
                id::OBJECT_HEADER => {
                    let mut r = ByteReader::new(&body);
                    obj.handle = r.read_u16()?;
                    obj.type_id = r.read_i16()?;
                    obj.kind = ObjectKind::from_type_id(obj.type_id);
                    obj.flags = r.read_u16()?;
                    let _reserved = r.read_u16()?;
                    obj.ink_effect = r.read_u32().unwrap_or(0);
                    obj.ink_effect_param = r.read_u32().unwrap_or(0);
                    have_header = true;
                }
                id::OBJECT_NAME => obj.name = decode_cstr(&body, unicode),
                id::OBJECT_PROPERTIES => obj.properties = Some(body),
                _ => {}
            }
        }
        if have_header {
            if let Some(p) = &obj.properties {
                match parse_properties(obj.type_id, p, build, unicode, old) {
                    Ok(props) => obj.props = props,
                    Err(e) => warnings.push(format!("object '{}' properties: {e:#}", obj.name)),
                }
            }
            out.push(obj);
        } else {
            warnings.push(format!("object #{i} has no header chunk; skipped"));
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct ExtensionInfo {
    pub handle: i16,
    pub magic: i32,
    pub version_ls: i32,
    pub version_ms: i32,
    /// Extension file name without ".mfx", e.g. "kcini".
    pub name: String,
    pub sub_type: String,
}

pub fn parse_extensions(data: &[u8], unicode: bool) -> Result<Vec<ExtensionInfo>> {
    let mut r = ByteReader::new(data);
    let count = r.read_u16()?;
    let _preload = r.read_u16()?;
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let start = r.pos();
        let size = (r.read_i16()? as i32).unsigned_abs() as usize;
        let handle = r.read_i16()?;
        let magic = r.read_i32()?;
        let version_ls = r.read_i32()?;
        let version_ms = r.read_i32()?;
        let name = r.read_cstring(unicode)?;
        let sub_type = r.read_cstring(unicode).unwrap_or_default();
        let name = name.split('.').next().unwrap_or_default().to_string();
        out.push(ExtensionInfo {
            handle,
            magic,
            version_ls,
            version_ms,
            name,
            sub_type,
        });
        r.seek(start + size)?;
    }
    Ok(out)
}
