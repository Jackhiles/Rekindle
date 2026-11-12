//! Image bank (chunk 0x6666) and pixel decoding.
//!
//! Bank: `count: u32`, then per image `handle: u32, decompressed: u32, compressed: u32,
//! zlib[compressed]`. The inflated block is a 32-byte header followed by pixel data.

use super::chunk::{inflate, inflate_prefix};
use super::reader::{Blob, ByteReader};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;

const IMAGE_HEADER_SIZE: usize = 32;

pub mod flags {
    pub const RLE: u8 = 0x01;
    pub const RLEW: u8 = 0x02;
    pub const RLET: u8 = 0x04;
    pub const LZX: u8 = 0x08;
    pub const ALPHA: u8 = 0x10;
    pub const ACE: u8 = 0x20;
    pub const MAC: u8 = 0x40;
}

#[derive(Debug, Clone)]
pub struct ImageEntry {
    pub handle: u32,
    pub width: u16,
    pub height: u16,
    pub graphic_mode: u8,
    pub flags: u8,
    pub hotspot: (i16, i16),
    pub action_point: (i16, i16),
    /// R, G, B, A of the transparent colour key.
    pub transparent: [u8; 4],
    pub data_size: u32,
    decompressed_size: u32,
    compressed: Blob,
    /// Already-decompressed block (1.5 data is decompressed while loading).
    decoded: Option<Blob>,
    header_size: usize,
    /// Colours for 8-bit palette images.
    palette: Option<std::sync::Arc<Vec<[u8; 4]>>>,
}

#[derive(Debug, Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct ImageBank {
    pub images: Vec<ImageEntry>,
    by_handle: HashMap<u32, usize>,
}

impl ImageBank {
    /// `build` matters because runtimes from build 284 on store handles 1-based;
    /// they are normalised here so they match the 0-based references in objects.
    pub fn parse(bank: &Blob, build: u32) -> Result<Self> {
        Self::parse_with(bank, build, false, None)
    }

    /// `old`: Multimedia Fusion 1.5 layout. `palette`: colours for 8-bit images.
    pub fn parse_with(
        bank: &Blob,
        build: u32,
        old: bool,
        palette: Option<Vec<[u8; 4]>>,
    ) -> Result<Self> {
        let palette = palette.map(std::sync::Arc::new);
        if old {
            return Self::parse_old(bank, palette);
        }
        let mut r = ByteReader::new(bank);
        let count = r.read_u32()? as usize;
        let mut images = Vec::with_capacity(count);
        for i in 0..count {
            let mut handle = r.read_u32()?;
            if build >= 284 {
                handle = handle.saturating_sub(1);
            }
            let decompressed_size = r.read_u32()?;
            let comp = r.read_u32()? as usize;
            let start = r.pos();
            r.skip(comp)
                .with_context(|| format!("image #{i} (handle {handle}) truncated"))?;
            let compressed = bank.slice(start..start + comp)?;
            let head = inflate_prefix(&compressed, IMAGE_HEADER_SIZE)
                .with_context(|| format!("image #{i} (handle {handle}) header"))?;
            let mut h = ByteReader::new(&head);
            let _checksum = h.read_i32()?;
            let _references = h.read_i32()?;
            let data_size = h.read_u32()?;
            let width = h.read_u16()?;
            let height = h.read_u16()?;
            let graphic_mode = h.read_u8()?;
            let flags = h.read_u8()?;
            let _reserved = h.read_u16()?;
            let hotspot = (h.read_i16()?, h.read_i16()?);
            let action_point = (h.read_i16()?, h.read_i16()?);
            let transparent = h.read_color()?;
            images.push(ImageEntry {
                handle,
                width,
                height,
                graphic_mode,
                flags,
                hotspot,
                action_point,
                transparent,
                data_size,
                decompressed_size,
                compressed,
                decoded: None,
                header_size: IMAGE_HEADER_SIZE,
                palette: palette.clone(),
            });
        }
        let by_handle = images
            .iter()
            .enumerate()
            .map(|(i, e)| (e.handle, i))
            .collect();
        Ok(Self { images, by_handle })
    }

    /// 1.5 entries: `handle u32`, then a 1.5-compressed block holding a 24-byte header
    /// (`checksum u16, references i32, size u32, width, height, mode u8, flags u8,
    /// hotspot, action point`) and the pixels. The transparent colour is black.
    fn parse_old(bank: &Blob, palette: Option<std::sync::Arc<Vec<[u8; 4]>>>) -> Result<Self> {
        let mut r = ByteReader::new(bank);
        let count = r.read_u32()? as usize;
        let mut images = Vec::with_capacity(count);
        for i in 0..count {
            let handle = r.read_u32()?;
            let (raw, used) = super::old_inflate::inflate_block(&bank[r.pos()..])
                .with_context(|| format!("image #{i} (handle {handle})"))?;
            r.skip(used)?;
            let mut h = ByteReader::new(&raw);
            let _checksum = h.read_u16()?;
            let _references = h.read_i32()?;
            let data_size = h.read_u32()?;
            let width = h.read_u16()?;
            let height = h.read_u16()?;
            let graphic_mode = h.read_u8()?;
            let flags = h.read_u8()?;
            let hotspot = (h.read_i16()?, h.read_i16()?);
            let action_point = (h.read_i16()?, h.read_i16()?);
            let header_size = h.pos();
            let decompressed_size = raw.len() as u32;
            images.push(ImageEntry {
                handle,
                width,
                height,
                graphic_mode,
                flags,
                hotspot,
                action_point,
                transparent: [0, 0, 0, 0],
                data_size,
                decompressed_size,
                compressed: Blob::new(Vec::new()),
                decoded: Some(Blob::new(raw)),
                header_size,
                palette: palette.clone(),
            });
        }
        let by_handle = images
            .iter()
            .enumerate()
            .map(|(i, e)| (e.handle, i))
            .collect();
        Ok(Self { images, by_handle })
    }

    pub fn get(&self, handle: u32) -> Option<&ImageEntry> {
        self.by_handle.get(&handle).map(|&i| &self.images[i])
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Row padding in *pixels* used by the Fusion bitmap layout.
fn padding(width: usize, point_size: usize, align: usize) -> usize {
    let pad = align - (width * point_size) % align;
    if pad == align {
        0
    } else {
        pad.div_ceil(point_size)
    }
}

/// The transparent colour key as it decodes after quantisation to the image's format.
fn quantize_key(mode: u8, key: [u8; 4]) -> [u8; 3] {
    let [r, g, b, _] = key;
    let expand5 = |v: u8| ((v >> 3) as u32 * 255 / 31) as u8;
    match mode {
        // RGB565
        7 => [expand5(r), ((g >> 2) as u32 * 255 / 63) as u8, expand5(b)],
        // RGB555
        6 => [expand5(r), expand5(g), expand5(b)],
        _ => [r, g, b],
    }
}

/// Expand run-length-encoded colour data into the plain padded layout
/// (`color_len` bytes), keeping any trailing bytes (the alpha plane) intact.
///
/// Stream of commands: `0` ends; `n > 128` is followed by `n - 128` literal points;
/// otherwise `n` repeats of the single point that follows.
fn expand_rle(src: &[u8], color_len: usize, point_size: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(color_len + src.len());
    let mut i = 0;
    while out.len() < color_len {
        let Some(&cmd) = src.get(i) else {
            bail!("RLE stream ended early")
        };
        i += 1;
        if cmd == 0 {
            break;
        }
        if cmd > 128 {
            let n = (cmd - 128) as usize * point_size;
            out.extend_from_slice(src.get(i..i + n).context("RLE literal run truncated")?);
            i += n;
        } else {
            let p = src.get(i..i + point_size).context("RLE repeat truncated")?;
            for _ in 0..cmd {
                out.extend_from_slice(p);
            }
            i += point_size;
        }
    }
    // A terminating 0 may follow a stream that exactly filled the image.
    if src.get(i) == Some(&0) && out.len() >= color_len {
        i += 1;
    }
    out.resize(color_len, 0);
    out.extend_from_slice(src.get(i..).unwrap_or_default());
    Ok(out)
}

impl ImageEntry {
    pub fn is_supported(&self) -> bool {
        matches!(self.graphic_mode, 3 | 4 | 6 | 7 | 8)
    }

    pub fn decode_rgba(&self) -> Result<RgbaImage> {
        let raw = match &self.decoded {
            Some(b) => b.to_vec(),
            None => inflate(&self.compressed, self.decompressed_size as usize)?,
        };
        let hs = self.header_size;
        let mut data = raw
            .get(hs..hs + self.data_size as usize)
            .or_else(|| raw.get(hs..))
            .context("image data truncated")?
            .to_vec();
        if self.flags & flags::LZX != 0 {
            let mut r = ByteReader::new(&data);
            let size = r.read_u32()? as usize;
            data = inflate(r.rest(), size)?;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let mut px = vec![0u8; w * h * 4];
        let point_size = match self.graphic_mode {
            3 => 1,
            4 => 3,
            6 | 7 => 2,
            8 => 4,
            m => bail!("unsupported graphic mode {m}"),
        };
        let row_pad = if self.graphic_mode == 8 {
            0
        } else {
            padding(w, point_size, 2)
        };
        let row_bytes = (w + row_pad) * point_size;
        let color_len = row_bytes * h;
        if self.flags & (flags::RLE | flags::RLEW | flags::RLET) != 0 {
            data = expand_rle(&data, color_len, point_size)?;
        }
        if data.len() < color_len {
            bail!("pixel data too short: {} < {}", data.len(), color_len);
        }
        for y in 0..h {
            let row = &data[y * row_bytes..];
            for x in 0..w {
                let p = &row[x * point_size..];
                let (r, g, b, a) = match self.graphic_mode {
                    3 => {
                        let c = self
                            .palette
                            .as_ref()
                            .and_then(|pal| pal.get(p[0] as usize))
                            .copied();
                        let [r, g, b, _] = c.unwrap_or([p[0], p[0], p[0], 255]);
                        (r, g, b, 255)
                    }
                    4 => (p[2], p[1], p[0], 255),
                    8 => (p[2], p[1], p[0], p[3]),
                    6 => {
                        let v = u16::from_le_bytes([p[0], p[1]]);
                        let c = |s: u16| (((v >> s) & 0x1F) as u32 * 255 / 31) as u8;
                        (c(10), c(5), c(0), 255)
                    }
                    _ => {
                        let v = u16::from_le_bytes([p[0], p[1]]);
                        let r = ((v >> 11) & 0x1F) as u32 * 255 / 31;
                        let g = ((v >> 5) & 0x3F) as u32 * 255 / 63;
                        let b = (v & 0x1F) as u32 * 255 / 31;
                        (r as u8, g as u8, b as u8, 255)
                    }
                };
                let o = (y * w + x) * 4;
                px[o..o + 4].copy_from_slice(&[r, g, b, a]);
            }
        }

        if self.flags & flags::ALPHA != 0 && self.graphic_mode != 8 {
            let apad = padding(w, 1, 4);
            let alpha = &data[color_len..];
            let arow = w + apad;
            if alpha.len() >= arow * (h.saturating_sub(1)) + w {
                for y in 0..h {
                    for x in 0..w {
                        px[(y * w + x) * 4 + 3] = alpha[y * arow + x];
                    }
                }
            }
        } else if self.graphic_mode != 8 {
            // Compare in the image's own bit depth: a 24-bit key such as (120, 120, 120)
            // stored in a 16-bit image decodes to a slightly different colour.
            let [tr, tg, tb] = quantize_key(self.graphic_mode, self.transparent);
            for p in px.chunks_exact_mut(4) {
                if p[0] == tr && p[1] == tg && p[2] == tb {
                    p[3] = 0;
                }
            }
        }
        Ok(RgbaImage {
            width: w as u32,
            height: h as u32,
            pixels: px,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_key_matches_quantised_pixels() {
        // (120,120,120) in RGB565 is r=15, g=30, b=15, which decodes to (123, 121, 123).
        assert_eq!(quantize_key(7, [120, 120, 120, 0]), [123, 121, 123]);
        assert_eq!(quantize_key(6, [120, 120, 120, 0]), [123, 123, 123]);
        assert_eq!(quantize_key(4, [120, 120, 120, 0]), [120, 120, 120]);
    }
}
