//! The application icon from a Windows executable's resources (RT_ICON), so the
//! window can show the game's own icon.

/// An RGBA image, row-major, top row first.
#[derive(Debug, Clone)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

const RT_ICON: u32 = 3;
const RESOURCE_DIR_INDEX: usize = 2;

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn i32_at(d: &[u8], o: usize) -> Option<i32> {
    u32_at(d, o).map(|v| v as i32)
}

/// Largest, deepest icon image in the executable, if it has any.
pub fn from_pe(data: &[u8]) -> Option<IconImage> {
    let pe = u32_at(data, 0x3C)? as usize;
    if data.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = u16_at(data, pe + 6)? as usize;
    let opt_size = u16_at(data, pe + 20)? as usize;
    let opt = pe + 24;
    let magic = u16_at(data, opt)?;
    let dirs = opt + if magic == 0x20B { 112 } else { 96 };
    let rsrc_rva = u32_at(data, dirs + RESOURCE_DIR_INDEX * 8)? as usize;
    if rsrc_rva == 0 {
        return None;
    }
    // Map RVAs to file offsets through the section table.
    let table = opt + opt_size;
    let section_of = |rva: usize| -> Option<usize> {
        (0..sections).find_map(|i| {
            let s = table + 40 * i;
            let vsize = u32_at(data, s + 8)? as usize;
            let va = u32_at(data, s + 12)? as usize;
            let raw_size = u32_at(data, s + 16)? as usize;
            let raw = u32_at(data, s + 20)? as usize;
            (rva >= va && rva < va + vsize.max(raw_size)).then(|| raw + (rva - va))
        })
    };
    let root = section_of(rsrc_rva)?;

    // Resource directory: entries follow a 16-byte header.
    let entries = |dir: usize| -> Vec<(u32, u32)> {
        let named = u16_at(data, dir + 12).unwrap_or(0) as usize;
        let ids = u16_at(data, dir + 14).unwrap_or(0) as usize;
        (0..(named + ids).min(4096))
            .filter_map(|i| Some((u32_at(data, dir + 16 + 8 * i)?, u32_at(data, dir + 20 + 8 * i)?)))
            .collect()
    };
    let (_, icons) = entries(root).into_iter().find(|&(id, off)| id == RT_ICON && off & 0x8000_0000 != 0)?;
    let mut best: Option<(u64, IconImage)> = None;
    for (_, name) in entries(root + (icons & 0x7FFF_FFFF) as usize) {
        if name & 0x8000_0000 == 0 {
            continue;
        }
        for (_, lang) in entries(root + (name & 0x7FFF_FFFF) as usize) {
            if lang & 0x8000_0000 != 0 {
                continue;
            }
            let entry = root + lang as usize;
            let (Some(rva), Some(size)) = (u32_at(data, entry), u32_at(data, entry + 4)) else { continue };
            let Some(start) = section_of(rva as usize) else { continue };
            let Some(bytes) = data.get(start..start + size as usize) else { continue };
            if let Some((depth, img)) = decode(bytes) {
                let score = (img.width as u64 * img.height as u64).min(256 * 256) * 64 + depth as u64;
                if best.as_ref().is_none_or(|(s, _)| score > *s) {
                    best = Some((score, img));
                }
            }
        }
    }
    best.map(|(_, img)| img)
}

/// Decode one icon image (PNG, or a BMP DIB with an AND mask). Returns (bit depth, image).
fn decode(b: &[u8]) -> Option<(u16, IconImage)> {
    if b.starts_with(b"\x89PNG") {
        let dec = png::Decoder::new(std::io::Cursor::new(b));
        let mut reader = dec.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).ok()?;
        let (w, h) = (info.width, info.height);
        let px = (w * h) as usize;
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf[..px * 4].to_vec(),
            png::ColorType::Rgb => buf[..px * 3].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
            _ => return None,
        };
        return Some((32, IconImage { width: w, height: h, rgba }));
    }
    let header = u32_at(b, 0)? as usize;
    let w = i32_at(b, 4)?.unsigned_abs();
    let h = i32_at(b, 8)?.unsigned_abs() / 2; // the height includes the AND mask
    let bpp = u16_at(b, 14)?;
    if w == 0 || h == 0 || w > 512 || h > 512 || u32_at(b, 16)? != 0 {
        return None;
    }
    let colors = match u32_at(b, 32)? {
        0 if bpp <= 8 => 1usize << bpp,
        n => n as usize,
    };
    let palette_at = header;
    let pixels_at = palette_at + if bpp <= 8 { colors * 4 } else { 0 };
    let stride = (w as usize * bpp as usize).div_ceil(32) * 4;
    let mask_at = pixels_at + stride * h as usize;
    let mask_stride = (w as usize).div_ceil(32) * 4;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let mut any_alpha = false;
    for y in 0..h as usize {
        let row = pixels_at + (h as usize - 1 - y) * stride;
        for x in 0..w as usize {
            let (r, g, bl, a) = match bpp {
                32 => {
                    let p = b.get(row + x * 4..row + x * 4 + 4)?;
                    (p[2], p[1], p[0], p[3])
                }
                24 => {
                    let p = b.get(row + x * 3..row + x * 3 + 3)?;
                    (p[2], p[1], p[0], 255)
                }
                1 | 4 | 8 => {
                    let bit = x * bpp as usize;
                    let byte = *b.get(row + bit / 8)?;
                    let idx = (byte >> (8 - bpp as usize - bit % 8)) & ((1u16 << bpp) - 1) as u8;
                    let p = b.get(palette_at + idx as usize * 4..palette_at + idx as usize * 4 + 4)?;
                    (p[2], p[1], p[0], 255)
                }
                _ => return None,
            };
            any_alpha |= bpp == 32 && a != 0;
            let o = (y * w as usize + x) * 4;
            rgba[o..o + 4].copy_from_slice(&[r, g, bl, a]);
        }
    }
    // Without per-pixel alpha, the 1-bit AND mask marks transparent pixels.
    if !any_alpha {
        for y in 0..h as usize {
            let row = mask_at + (h as usize - 1 - y) * mask_stride;
            for x in 0..w as usize {
                let transparent = b.get(row + x / 8).is_some_and(|m| m & (0x80 >> (x % 8)) != 0);
                rgba[(y * w as usize + x) * 4 + 3] = if transparent { 0 } else { 255 };
            }
        }
    }
    Some((bpp, IconImage { width: w, height: h, rgba }))
}

impl IconImage {
    /// A `size` x `size` copy (area-averaged when shrinking, nearest when growing).
    pub fn resized(&self, size: u32) -> Vec<u8> {
        let mut out = vec![0u8; (size * size * 4) as usize];
        let (sx, sy) = (self.width as f32 / size as f32, self.height as f32 / size as f32);
        for y in 0..size {
            for x in 0..size {
                let (x0, y0) = ((x as f32 * sx) as u32, (y as f32 * sy) as u32);
                let x1 = (((x + 1) as f32 * sx).ceil() as u32).clamp(x0 + 1, self.width);
                let y1 = (((y + 1) as f32 * sy).ceil() as u32).clamp(y0 + 1, self.height);
                let mut acc = [0u32; 4];
                let mut n = 0;
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        let p = ((yy * self.width + xx) * 4) as usize;
                        let a = self.rgba[p + 3] as u32;
                        // Premultiply so transparent pixels don't bleed their colour.
                        for c in 0..3 {
                            acc[c] += self.rgba[p + c] as u32 * a;
                        }
                        acc[3] += a;
                        n += 1;
                    }
                }
                let o = ((y * size + x) * 4) as usize;
                if acc[3] > 0 {
                    for c in 0..3 {
                        out[o + c] = (acc[c] / acc[3]) as u8;
                    }
                }
                out[o + 3] = (acc[3] / n.max(1)) as u8;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_32bit_dib_and_resizes() {
        // 2x2, 32-bit, bottom-up: bottom row red, top row blue (opaque).
        let mut b = vec![0u8; 40];
        b[0..4].copy_from_slice(&40u32.to_le_bytes());
        b[4..8].copy_from_slice(&2i32.to_le_bytes());
        b[8..12].copy_from_slice(&4i32.to_le_bytes());
        b[12..14].copy_from_slice(&1u16.to_le_bytes());
        b[14..16].copy_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(&[0, 0, 255, 255, 0, 0, 255, 255]); // bottom row: red (BGRA)
        b.extend_from_slice(&[255, 0, 0, 255, 255, 0, 0, 255]); // top row: blue
        b.extend_from_slice(&[0; 8]); // AND mask
        let (_, img) = decode(&b).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(&img.rgba[0..4], &[0, 0, 255, 255], "top-left is blue");
        assert_eq!(&img.rgba[8..12], &[255, 0, 0, 255], "bottom-left is red");
        let small = img.resized(4);
        assert_eq!(small.len(), 64);
        assert_eq!(&small[0..4], &[0, 0, 255, 255]);
    }
}
