//! Font bank (chunk 0x6667). Entries hold a Windows LOGFONT description.

use super::chunk::inflate;
use super::reader::{Blob, ByteReader};
use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct FontEntry {
    pub handle: u32,
    pub height: i32,
    pub width: i32,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike_out: bool,
    pub face_name: String,
}

#[derive(Debug, Default)]
pub struct FontBank {
    pub fonts: Vec<FontEntry>,
}

impl FontBank {
    /// Multimedia Fusion 1.5 font bank: `handle u32` + 1.5-compressed `checksum u16,
    /// references, size, then a 16-bit LOGFONT` (ANSI face name).
    pub fn parse_old(bank: &Blob) -> Result<Self> {
        let mut r = ByteReader::new(bank);
        let count = r.read_u32()? as usize;
        let mut fonts = Vec::with_capacity(count);
        for i in 0..count {
            let handle = r.read_u32()?;
            let (raw, used) = super::old_inflate::inflate_block(&bank[r.pos()..])
                .with_context(|| format!("font #{i} (handle {handle})"))?;
            r.skip(used)?;
            // 14-byte header, then a 16-bit LOGFONT.
            let mut f = ByteReader::new(&raw);
            f.skip(14)?;
            let height = f.read_i16()? as i32;
            let width = f.read_i16()? as i32;
            f.skip(4)?; // escapement, orientation
            let weight = f.read_i16()? as i32;
            let italic = f.read_u8()? != 0;
            let underline = f.read_u8()? != 0;
            let strike_out = f.read_u8()? != 0;
            f.skip(5)?;
            let face_name = f.read_fixed_string(32, false).unwrap_or_default();
            fonts.push(FontEntry {
                handle,
                height,
                width,
                weight,
                italic,
                underline,
                strike_out,
                face_name,
            });
        }
        Ok(Self { fonts })
    }

    /// Builds >= 284 store handles 1-based; they are normalised to the 0-based references used in events.
    pub fn parse(bank: &Blob, unicode: bool, build: u32) -> Result<Self> {
        let mut r = ByteReader::new(bank);
        let count = r.read_u32()? as usize;
        let mut fonts = Vec::with_capacity(count);
        for i in 0..count {
            let handle = r.read_u32()?.saturating_sub((build >= 284) as u32);
            let decomp = r.read_u32()? as usize;
            let comp = r.read_u32()? as usize;
            let raw = inflate(r.read_bytes(comp)?, decomp)
                .with_context(|| format!("font #{i} (handle {handle})"))?;
            let mut f = ByteReader::new(&raw);
            let _checksum = f.read_i32()?;
            let _references = f.read_i32()?;
            let _size = f.read_i32()?;
            let height = f.read_i32()?;
            let width = f.read_i32()?;
            let _escapement = f.read_i32()?;
            let _orientation = f.read_i32()?;
            let weight = f.read_i32()?;
            let italic = f.read_u8()? != 0;
            let underline = f.read_u8()? != 0;
            let strike_out = f.read_u8()? != 0;
            f.skip(5)?; // charset, precision, clip precision, quality, pitch & family
            let face_name = f.read_fixed_string(32, unicode).unwrap_or_default();
            fonts.push(FontEntry {
                handle,
                height,
                width,
                weight,
                italic,
                underline,
                strike_out,
                face_name,
            });
        }
        Ok(Self { fonts })
    }
}
