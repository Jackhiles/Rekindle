//! Text rendering for String objects and text counters.
//!
//! Fusion games reference fonts by Windows LOGFONT (face name, height, weight); the
//! font files themselves are not in the game data. We look the face up among the
//! operating system's installed fonts and fall back to Macroquad's built-in font.

use crate::ctf_loader::fonts::FontEntry;
use crate::ctf_loader::properties::paragraph_flags;
use crate::ctf_loader::GameData;
use macroquad::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(w) = std::env::var("WINDIR") {
        dirs.push(PathBuf::from(w).join("Fonts"));
    }
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        dirs.push(
            PathBuf::from(l)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    dirs.push(PathBuf::from("/usr/share/fonts/truetype"));
    dirs.push(PathBuf::from("/usr/share/fonts/TTF"));
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs.push(PathBuf::from("/Library/Fonts"));
    dirs.push(PathBuf::from("/System/Library/Fonts/Supplemental"));
    dirs
}

/// Likely file names (lowercase, without extension) for a face name.
fn candidates(face: &str, bold: bool, italic: bool) -> Vec<String> {
    let f = face.to_lowercase();
    // Common faces whose file names don't follow the face name.
    let base: &[&str] = match f.as_str() {
        "times new roman" => &["times"],
        "arial" => &["arial"],
        "arial narrow" => &["arialn", "arial"],
        "arial black" => &["ariblk", "arial"],
        "courier new" => &["cour"],
        "consolas" => &["consola", "cour"],
        "trebuchet ms" => &["trebuc"],
        "comic sans ms" => &["comic"],
        "segoe ui" => &["segoeui"],
        "lucida console" => &["lucon"],
        "palatino linotype" => &["pala"],
        "georgia" => &["georgia"],
        "verdana" => &["verdana"],
        "tahoma" => &["tahoma"],
        "impact" => &["impact"],
        "corbel" => &["corbel"],
        // Bitmap faces have no TrueType file; use a monospace stand-in.
        "terminal" | "system" | "fixedsys" | "courier" => &["consola", "cour", "lucon"],
        _ => &[],
    };
    let squashed = f.replace(' ', "");
    let mut out = Vec::new();
    for b in base
        .iter()
        .map(|s| s.to_string())
        .chain([squashed.clone(), f.clone()])
    {
        let style = match (bold, italic) {
            (true, true) => vec![format!("{b}bi"), format!("{b}z")],
            (true, false) => vec![format!("{b}bd"), format!("{b}b"), format!("{b}-bold")],
            (false, true) => vec![format!("{b}i"), format!("{b}-italic")],
            _ => vec![],
        };
        out.extend(style);
        out.push(b);
    }
    out
}

/// The installed font file for a face name, if any.
pub fn find_font_file(face: &str, bold: bool, italic: bool) -> Option<PathBuf> {
    let dirs = font_dirs();
    for name in candidates(face, bold, italic) {
        for d in &dirs {
            for ext in ["ttf", "otf", "TTF"] {
                let p = d.join(format!("{name}.{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

#[derive(Default)]
pub struct Fonts {
    /// Bank font handle -> loaded font (None = use the default font).
    loaded: HashMap<u32, Option<Font>>,
    by_file: HashMap<PathBuf, Font>,
}

impl Fonts {
    fn entry(g: &GameData, handle: u32) -> Option<&FontEntry> {
        g.fonts.fonts.iter().find(|f| f.handle == handle)
    }

    /// A system font by face name (falls back to Arial, then the built-in font).
    pub fn by_face(&mut self, face: &str, bold: bool) -> Option<Font> {
        let path = find_font_file(face, bold, false)
            .or_else(|| find_font_file(face, false, false))
            .or_else(|| find_font_file("arial", bold, false))?;
        if let Some(f) = self.by_file.get(&path) {
            return Some(f.clone());
        }
        let f = load_ttf_font_from_bytes(&std::fs::read(&path).ok()?).ok()?;
        self.by_file.insert(path, f.clone());
        Some(f)
    }

    /// A font by face name only if it is installed (no substitute).
    pub fn installed(&mut self, face: &str) -> Option<Font> {
        find_font_file(face, false, false)?;
        self.by_face(face, false)
    }

    /// (font, pixel height) for a bank font handle.
    pub fn get(&mut self, g: &GameData, handle: Option<u32>) -> (Option<Font>, f32) {
        let Some(h) = handle else { return (None, 16.0) };
        let entry = Self::entry(g, h);
        let size = entry.map_or(16.0, |e| (e.height.unsigned_abs() as f32).max(6.0));
        if !self.loaded.contains_key(&h) {
            let font = entry.and_then(|e| {
                let path = find_font_file(&e.face_name, e.weight >= 600, e.italic)
                    .or_else(|| find_font_file(&e.face_name, false, false))
                    .or_else(|| find_font_file("arial", false, false))?;
                if let Some(f) = self.by_file.get(&path) {
                    return Some(f.clone());
                }
                let bytes = std::fs::read(&path).ok()?;
                let f = load_ttf_font_from_bytes(&bytes).ok()?;
                self.by_file.insert(path, f.clone());
                Some(f)
            });
            self.loaded.insert(h, font);
        }
        (self.loaded[&h].clone(), size)
    }
}

/// Greedy word wrap of `text` to `max_w` pixels; explicit newlines are kept.
fn wrap(text: &str, font: Option<&Font>, size: u16, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.replace("\r\n", "\n").split(['\n', '\r']) {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && measure_text(&candidate, font, size, 1.0).width > max_w {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines
}

/// Draw text inside the screen-space box `(x, y, w, h)`; `scale` maps game pixels to screen pixels.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_box(
    text: &str,
    font: Option<&Font>,
    px: f32,
    color: Color,
    flags: u16,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) {
    let size = px.round().clamp(4.0, 400.0) as u16;
    let lines = wrap(text, font, size, w.max(1.0));
    let line_h = px * 1.15;
    let total_h = line_h * lines.len() as f32;
    let top = if flags & paragraph_flags::BOTTOM != 0 {
        y + h - total_h
    } else if flags & paragraph_flags::V_CENTER != 0 {
        y + (h - total_h) / 2.0
    } else {
        y
    };
    let ascent = px * 0.85;
    for (i, line) in lines.iter().enumerate() {
        let lw = measure_text(line, font, size, 1.0).width;
        let lx = if flags & paragraph_flags::RIGHT != 0 {
            x + w - lw
        } else if flags & paragraph_flags::H_CENTER != 0 {
            x + (w - lw) / 2.0
        } else {
            x
        };
        let params = TextParams {
            font,
            font_size: size,
            font_scale: 1.0,
            color,
            ..Default::default()
        };
        draw_text_ex(line, lx, top + ascent + line_h * i as f32, params);
    }
}
