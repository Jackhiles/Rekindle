//! Library artwork for the launcher: a portrait cover and a wide banner ("hero") per game.
//!
//! Steam's own artwork is used when the game came from Steam; otherwise the same artwork
//! Rekindle makes for Steam shortcuts is generated from the game's images. Files are cached
//! in `<data dir>\Rekindle\art\<game key>\`; a worker thread prepares them and hands back
//! small RGBA images ready to upload as textures.

use super::library;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};

/// Decoded artwork for one game.
pub struct Art {
    pub game: PathBuf,
    pub cover: Option<Rgba>,
    pub hero: Option<Rgba>,
}

pub struct Rgba {
    pub w: u16,
    pub h: u16,
    pub px: Vec<u8>,
}

/// Largest sizes kept in memory (twice the size they're drawn at, for sharp scaling).
const COVER_MAX: (usize, usize) = (360, 540);
const HERO_MAX_W: usize = 760;

pub fn dir(game: &Path) -> PathBuf {
    library::data_dir().join("art").join(library::game_key(game))
}

fn find(dir: &Path, stem: &str) -> Option<PathBuf> {
    ["png", "jpg"].iter().map(|e| dir.join(format!("{stem}.{e}"))).find(|p| p.is_file())
}

/// Make sure a game's cover and hero exist on disk; returns their files.
fn prepare(game: &Path, title: &str) -> (Option<PathBuf>, Option<PathBuf>) {
    let d = dir(game);
    if let (Some(c), Some(h)) = (find(&d, "cover"), find(&d, "hero")) {
        return (Some(c), Some(h));
    }
    let _ = std::fs::create_dir_all(&d);
    // Steam's library artwork, when the game is a Steam game.
    if let Some(app) = super::steam_shortcuts::steam_app_for(game) {
        for (suffix, src) in super::steam_shortcuts::steam_art(&app) {
            let stem = match suffix {
                "p" => "cover",
                "_hero" => "hero",
                _ => continue,
            };
            let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "jpg".into());
            let _ = std::fs::copy(&src, d.join(format!("{stem}.{ext}")));
        }
    }
    // Steam makes its own portrait for games that have none: the small store banner on a
    // blurred stretch. Rekindle's generated cover looks better, so drop that one.
    if let Some(c) = find(&d, "cover") {
        if decode(&c).is_some_and(|i| is_stretched_fallback(&i)) {
            let _ = std::fs::remove_file(&c);
        }
    }
    // Generated artwork for whatever is still missing.
    if find(&d, "cover").is_none() || find(&d, "hero").is_none() {
        if let Some(g) = super::steam_shortcuts::load_game(game) {
            let tmp = d.join("generated");
            for (suffix, p) in super::art::generate(&g, title, &tmp) {
                let stem = match suffix {
                    "p" => "cover",
                    "_hero" => "hero",
                    _ => continue,
                };
                if find(&d, stem).is_none() {
                    let _ = std::fs::rename(&p, d.join(format!("{stem}.png")));
                }
            }
            let _ = std::fs::remove_dir_all(&tmp);
        }
    }
    (find(&d, "cover"), find(&d, "hero"))
}

/// Forget a game's artwork (it is made again next time).
pub fn clear(game: &Path) {
    let _ = std::fs::remove_dir_all(dir(game));
}

/// Start the worker. Send it (game, title); it answers through `out`.
pub fn worker(out: impl Fn(Art) + Send + 'static) -> Sender<(PathBuf, String)> {
    let (tx, rx) = channel::<(PathBuf, String)>();
    std::thread::spawn(move || {
        while let Ok((game, title)) = rx.recv() {
            let (c, h) = prepare(&game, &title);
            let cover = c.and_then(|p| decode(&p)).map(|i| shrink(i, COVER_MAX.0, COVER_MAX.1));
            let hero = h.and_then(|p| decode(&p)).map(|i| shrink(i, HERO_MAX_W, usize::MAX));
            out(Art { game, cover, hero });
        }
    });
    tx
}

/// Decode a PNG or JPEG file to RGBA.
fn decode(path: &Path) -> Option<Rgba> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(&bytes));
        let px = d.decode().ok()?;
        let info = d.info()?;
        let rgba: Vec<u8> = match info.pixel_format {
            jpeg_decoder::PixelFormat::RGB24 => px.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
            jpeg_decoder::PixelFormat::L8 => px.iter().flat_map(|&l| [l, l, l, 255]).collect(),
            _ => return None,
        };
        return Some(Rgba { w: info.width, h: info.height, px: rgba });
    }
    let mut dec = png::Decoder::new(std::io::Cursor::new(&bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    let buf = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf.to_vec(),
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&l| [l, l, l, 255]).collect(),
        _ => return None,
    };
    Some(Rgba { w: info.width as u16, h: info.height as u16, px: rgba })
}

/// Top and bottom tenth almost without detail (a blur), the middle with detail.
fn is_stretched_fallback(img: &Rgba) -> bool {
    let (w, h) = (img.w as usize, img.h as usize);
    if w < 8 || h < 40 {
        return false;
    }
    let lum = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        (img.px[i] as f32 * 0.3 + img.px[i + 1] as f32 * 0.59 + img.px[i + 2] as f32 * 0.11) as f32
    };
    let energy = |y0: usize, y1: usize| {
        let mut sum: f32 = 0.0;
        let mut n: f32 = 0.0;
        for y in (y0..y1).step_by(2) {
            for x in 1..w {
                sum += (lum(x, y) - lum(x - 1, y)).abs();
                n += 1.0;
            }
        }
        sum / n.max(1.0)
    };
    let (top, bottom, middle) = (energy(0, h / 10), energy(h - h / 10, h), energy(h * 3 / 10, h * 45 / 100));
    top < 2.5 && bottom < 2.5 && middle > 3.0 * top.max(bottom).max(0.5)
}

/// Box-filter `img` down to fit within max_w x max_h (never enlarged).
fn shrink(img: Rgba, max_w: usize, max_h: usize) -> Rgba {
    let (w, h) = (img.w as usize, img.h as usize);
    let scale = (max_w as f32 / w as f32).min(max_h as f32 / h as f32);
    if scale >= 1.0 || w == 0 || h == 0 {
        return img;
    }
    let (nw, nh) = (((w as f32 * scale).round() as usize).max(1), ((h as f32 * scale).round() as usize).max(1));
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        let (y0, y1) = (y * h / nh, ((y + 1) * h / nh).max(y * h / nh + 1));
        for x in 0..nw {
            let (x0, x1) = (x * w / nw, ((x + 1) * w / nw).max(x * w / nw + 1));
            let mut acc = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy * w + sx) * 4;
                    for c in 0..4 {
                        acc[c] += img.px[i + c] as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (y * nw + x) * 4;
            for c in 0..4 {
                out[o + c] = (acc[c] / n) as u8;
            }
        }
    }
    Rgba { w: nw as u16, h: nh as u16, px: out }
}
