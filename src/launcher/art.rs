//! Library artwork for games that have none from Steam, made from the game itself: its
//! most detailed full-screen picture as the background, its icon and its title.
//!
//! Produces Steam's grid images: portrait capsule (600 x 900), wide capsule (920 x 430),
//! hero banner (1920 x 620) and a transparent title logo.

use crate::ctf_loader::{GameData, RgbaImage};
use std::path::{Path, PathBuf};

#[derive(Clone)]
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize, rgba: [u8; 4]) -> Self {
        Self { w, h, px: rgba.repeat(w * h) }
    }

    fn from_image(img: &RgbaImage) -> Self {
        Self { w: img.width as usize, h: img.height as usize, px: img.pixels.clone() }
    }

    fn get(&self, x: usize, y: usize) -> [f32; 4] {
        let i = (y.min(self.h - 1) * self.w + x.min(self.w - 1)) * 4;
        [self.px[i] as f32, self.px[i + 1] as f32, self.px[i + 2] as f32, self.px[i + 3] as f32]
    }

    fn bilinear(&self, x: f32, y: f32) -> [f32; 4] {
        let (x, y) = ((x - 0.5).max(0.0), (y - 0.5).max(0.0));
        let (x0, y0) = (x as usize, y as usize);
        let (tx, ty) = (x.fract(), y.fract());
        let (a, b, c, d) = (self.get(x0, y0), self.get(x0 + 1, y0), self.get(x0, y0 + 1), self.get(x0 + 1, y0 + 1));
        std::array::from_fn(|k| (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty)
    }

    /// Scale the region (sx, sy, sw, sh) of self to w x h (area average when shrinking).
    fn resample(&self, sx: f32, sy: f32, sw: f32, sh: f32, w: usize, h: usize) -> Canvas {
        let mut out = Canvas::new(w, h, [0; 4]);
        let (fx, fy) = (sw / w as f32, sh / h as f32);
        for y in 0..h {
            for x in 0..w {
                let c = if fx > 1.0 || fy > 1.0 {
                    let (x0, x1) = ((sx + x as f32 * fx) as usize, (sx + (x + 1) as f32 * fx).ceil() as usize);
                    let (y0, y1) = ((sy + y as f32 * fy) as usize, (sy + (y + 1) as f32 * fy).ceil() as usize);
                    let mut acc = [0f32; 4];
                    let mut n = 0.0;
                    for yy in y0..y1.max(y0 + 1) {
                        for xx in x0..x1.max(x0 + 1) {
                            let p = self.get(xx, yy);
                            for k in 0..4 {
                                acc[k] += p[k];
                            }
                            n += 1.0;
                        }
                    }
                    acc.map(|v| v / n)
                } else {
                    self.bilinear(sx + (x as f32 + 0.5) * fx, sy + (y as f32 + 0.5) * fy)
                };
                let i = (y * w + x) * 4;
                for k in 0..4 {
                    out.px[i + k] = c[k].round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        out
    }

    /// Scale to fill w x h, cropping the overflow around the centre.
    fn cover(&self, w: usize, h: usize) -> Canvas {
        let s = (w as f32 / self.w as f32).max(h as f32 / self.h as f32);
        let (cw, ch) = (w as f32 / s, h as f32 / s);
        self.resample((self.w as f32 - cw) / 2.0, (self.h as f32 - ch) / 2.0, cw, ch, w, h)
    }

    fn scaled(&self, w: usize, h: usize) -> Canvas {
        self.resample(0.0, 0.0, self.w as f32, self.h as f32, w, h)
    }

    /// Blend `c` (alpha 0..1) over pixel (x, y).
    fn blend(&mut self, x: i64, y: i64, c: [f32; 3], a: f32) {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 || a <= 0.0 {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        let da = self.px[i + 3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        for k in 0..3 {
            let d = self.px[i + k] as f32;
            let v = if oa > 0.0 { (c[k] * a + d * da * (1.0 - a)) / oa } else { 0.0 };
            self.px[i + k] = v.round().clamp(0.0, 255.0) as u8;
        }
        self.px[i + 3] = (oa * 255.0).round() as u8;
    }

    fn draw(&mut self, img: &Canvas, x: i64, y: i64) {
        for yy in 0..img.h {
            for xx in 0..img.w {
                let p = img.get(xx, yy);
                self.blend(x + xx as i64, y + yy as i64, [p[0], p[1], p[2]], p[3] / 255.0);
            }
        }
    }

    /// Darken towards black: `amount(x, y)` in 0..1.
    fn shade(&mut self, amount: impl Fn(f32, f32) -> f32) {
        for y in 0..self.h {
            for x in 0..self.w {
                let a = amount(x as f32 / self.w as f32, y as f32 / self.h as f32).clamp(0.0, 1.0);
                self.blend(x as i64, y as i64, [0.0; 3], a);
            }
        }
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut enc = png::Encoder::new(file, self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(std::io::Error::other)?;
        w.write_image_data(&self.px).map_err(std::io::Error::other)
    }
}

struct Text {
    font: fontdue::Font,
}

impl Text {
    fn new() -> Option<Self> {
        fontdue::Font::from_bytes(super::FONT_SEMIBOLD, fontdue::FontSettings::default()).ok().map(|font| Self { font })
    }

    fn width(&self, s: &str, px: f32) -> f32 {
        s.chars().map(|c| self.font.metrics(c, px).advance_width).sum()
    }

    /// Greedy word wrap to `max_w`.
    fn wrap(&self, s: &str, px: f32, max_w: f32) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        for word in s.split_whitespace() {
            match lines.last_mut() {
                Some(l) if self.width(&format!("{l} {word}"), px) <= max_w => {
                    l.push(' ');
                    l.push_str(word);
                }
                _ => lines.push(word.to_string()),
            }
        }
        lines
    }

    /// A size and wrapping for `s` within `lines` lines of `max_w`: as few lines as possible
    /// while the text stays at least 70% of `px`, then the largest size that fits.
    fn fit(&self, s: &str, px: f32, min: f32, max_w: f32, lines: usize) -> (f32, Vec<String>) {
        let fits = |size: f32, n: usize| {
            let l = self.wrap(s, size, max_w);
            (l.len() <= n && l.iter().all(|x| self.width(x, size) <= max_w)).then_some(l)
        };
        for n in 1..=lines {
            let mut size = px;
            while size >= (px * 0.7).max(min) {
                if let Some(l) = fits(size, n) {
                    return (size, l);
                }
                size -= 2.0;
            }
        }
        let mut size = px * 0.7;
        while size > min {
            if let Some(l) = fits(size, lines) {
                return (size, l);
            }
            size -= 2.0;
        }
        (min, self.wrap(s, min, max_w))
    }

    /// Draw a line with its baseline at `y`, horizontally centred on `cx` (or from `x` when
    /// `cx` is None), with a soft shadow.
    fn draw(&self, c: &mut Canvas, s: &str, px: f32, x: f32, cx: Option<f32>, y: f32) {
        let x0 = cx.map_or(x, |cx| cx - self.width(s, px) / 2.0);
        for (dx, dy, col, alpha) in [(3.0, 3.0, [0.0; 3], 0.6f32), (0.0, 0.0, [255.0; 3], 1.0)] {
            let mut pen = x0 + dx;
            for ch in s.chars() {
                let (m, bmp) = self.font.rasterize(ch, px);
                let top = (y + dy) as i64 - (m.height as i64 + m.ymin as i64);
                for gy in 0..m.height {
                    for gx in 0..m.width {
                        let a = bmp[gy * m.width + gx] as f32 / 255.0;
                        c.blend(pen as i64 + m.xmin as i64 + gx as i64, top + gy as i64, col, a * alpha);
                    }
                }
                pen += m.advance_width;
            }
        }
    }
}

/// The game's best background picture: the largest images near full-screen size, scored
/// by detail (contrast) and brightness, so title screens beat black fills.
fn pick_background(g: &GameData) -> Option<Canvas> {
    let (ww, wh) = (g.header.window_width.max(1) as u32, g.header.window_height.max(1) as u32);
    let mut cands: Vec<_> = g
        .images
        .images
        .iter()
        .filter(|e| e.width as u32 * e.height as u32 >= ww * wh * 2 / 5 && e.width as u32 >= ww * 3 / 5)
        .collect();
    cands.sort_by_key(|e| std::cmp::Reverse(e.width as u32 * e.height as u32));
    let mut best: Option<(f32, Canvas)> = None;
    for e in cands.into_iter().take(16) {
        let Ok(img) = e.decode_rgba() else { continue };
        let n = (img.width * img.height) as usize;
        if n == 0 {
            continue;
        }
        let (mut sum, mut sq, mut opaque) = (0f64, 0f64, 0usize);
        for p in img.pixels.chunks_exact(4).step_by(7) {
            if p[3] > 200 {
                let l = 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
                sum += l;
                sq += l * l;
                opaque += 1;
            }
        }
        let sampled = n.div_ceil(7);
        if opaque * 10 < sampled * 9 {
            continue; // mostly transparent: a sprite, not a background
        }
        let mean = sum / opaque as f64;
        let sd = (sq / opaque as f64 - mean * mean).max(0.0).sqrt();
        let score = (sd + 0.35 * mean) as f32;
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, Canvas::from_image(&img)));
        }
    }
    best.map(|(_, c)| c)
}

/// Make artwork for a game into `dir`: (Steam grid suffix, file) pairs.
pub fn generate(g: &GameData, title: &str, dir: &Path) -> Vec<(&'static str, PathBuf)> {
    let _ = std::fs::create_dir_all(dir);
    let bg = pick_background(g).unwrap_or_else(|| Canvas::new(16, 16, [40, 28, 22, 255]));
    let icon = g.icon.as_ref().map(|i| Canvas { w: 256, h: 256, px: i.resized(256) });
    let text = Text::new();
    let mut out = Vec::new();
    let mut save = |suffix: &'static str, name: &str, c: &Canvas| {
        let p = dir.join(name);
        if c.save(&p).is_ok() {
            out.push((suffix, p));
        }
    };

    // Portrait capsule: picture, shaded towards the bottom, icon and title.
    let mut p = bg.cover(600, 900);
    p.shade(|_, y| 0.25 + 0.65 * ((y - 0.35) / 0.5).clamp(0.0, 1.0));
    if let Some(i) = &icon {
        p.draw(&i.scaled(200, 200), 200, 330);
    }
    if let Some(t) = &text {
        let (px, lines) = t.fit(title, 64.0, 30.0, 540.0, 3);
        for (k, l) in lines.iter().enumerate() {
            t.draw(&mut p, l, px, 0.0, Some(300.0), 620.0 + px * 0.8 + k as f32 * px * 1.15);
        }
    }
    save("p", "portrait.png", &p);

    // Wide capsule: picture shaded on the left, icon and title.
    let mut w = bg.cover(920, 430);
    w.shade(|x, _| 0.75 - 0.55 * x);
    let text_x = if let Some(i) = &icon {
        w.draw(&i.scaled(150, 150), 48, 140);
        230.0
    } else {
        56.0
    };
    if let Some(t) = &text {
        let (px, lines) = t.fit(title, 58.0, 26.0, 900.0 - text_x - 30.0, 3);
        let block = lines.len() as f32 * px * 1.15;
        for (k, l) in lines.iter().enumerate() {
            t.draw(&mut w, l, px, text_x, None, 215.0 - block / 2.0 + px * 0.85 + k as f32 * px * 1.15);
        }
    }
    save("", "wide.png", &w);

    // Hero: the picture alone (Steam puts the logo over it).
    let mut h = bg.cover(1920, 620);
    h.shade(|_, _| 0.15);
    save("_hero", "hero.png", &h);

    // Logo: the title on transparent background.
    if let Some(t) = &text {
        let (px, lines) = t.fit(title, 110.0, 40.0, 1100.0, 2);
        let width = lines.iter().map(|l| t.width(l, px)).fold(0.0, f32::max).ceil() as usize + 24;
        let height = (lines.len() as f32 * px * 1.2 + px * 0.4).ceil() as usize;
        let mut l = Canvas::new(width.max(1), height.max(1), [0; 4]);
        for (k, line) in lines.iter().enumerate() {
            t.draw(&mut l, line, px, 0.0, Some(width as f32 / 2.0), px * 0.95 + k as f32 * px * 1.2);
        }
        save("_logo", "logo.png", &l);
    }
    out
}
