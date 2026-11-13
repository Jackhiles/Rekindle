//! The standard Windows-control extensions, reimplemented as in-game widgets: Edit box
//! (`kcedit`), List (`kclist`), Button (`kcbutton`) and Combo box (`kccombo`).
//!
//! Their condition / action / expression indices follow the objects' menu order (verified
//! against how games use them; e.g. Edit action 4 "set text", 23 "limit text size",
//! condition 3 "text just modified", List action 6 "add line", expression 7 "number of
//! lines"). Lines are numbered from 1; 0 means "no line".
//!
//! Settings block: `width u16, height u16`, then a LOGFONT (16-bit in 1.5 data, 32-bit
//! later), colours, flags, and for buttons the label text at the end.

use super::files::SaveFs;
use super::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetKind {
    Edit,
    List,
    Button,
    Combo,
}

impl WidgetKind {
    pub fn from_ext_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "kcedit" => Some(Self::Edit),
            "kclist" => Some(Self::List),
            "kcbutton" => Some(Self::Button),
            "kccombo" => Some(Self::Combo),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Widget {
    pub kind: WidgetKind,
    pub w: f32,
    pub h: f32,
    pub font_face: String,
    pub font_px: f32,
    pub bold: bool,
    pub fg: [u8; 4],
    pub bg: [u8; 4],
    /// Edit text, button label, or the combo box's edit field.
    pub text: String,
    /// List / combo lines.
    pub lines: Vec<String>,
    /// Selected line, 1-based (0 = none).
    pub current: usize,
    /// First visible line (0-based).
    pub top: usize,
    pub visible: bool,
    pub enabled: bool,
    pub read_only: bool,
    pub focused: bool,
    pub max_len: Option<usize>,
    pub checked: i32,
    // One-tick flags, cleared after the event pass.
    pub modified: bool,
    pub clicked: bool,
    pub selection_changed: bool,
    pub double_clicked: bool,
}

fn printable_run(data: &[u8], from: usize) -> Option<(usize, String)> {
    // First ASCII run of 3+ letters (an 8-bit face name) or the same as UTF-16.
    let mut i = from;
    while i + 3 <= data.len() {
        if data[i..i + 3].iter().all(|b| b.is_ascii_alphabetic()) {
            let end = data[i..].iter().position(|&b| b == 0).map_or(data.len(), |p| i + p);
            return Some((i, String::from_utf8_lossy(&data[i..end]).into_owned()));
        }
        if i + 6 <= data.len()
            && data[i].is_ascii_alphabetic()
            && data[i + 1] == 0
            && data[i + 2].is_ascii_alphabetic()
            && data[i + 3] == 0
            && data[i + 4].is_ascii_alphabetic()
            && data[i + 5] == 0
        {
            let units: Vec<u16> = data[i..]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&u| u != 0)
                .collect();
            return Some((i, String::from_utf16_lossy(&units)));
        }
        i += 1;
    }
    None
}

impl Widget {
    /// Build from the object's settings block (see the module docs).
    pub fn from_settings(kind: WidgetKind, data: &[u8]) -> Self {
        let u16_at = |o: usize| data.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
        let (mut w, mut h) = (u16_at(0) as f32, u16_at(2) as f32);
        if w <= 0.0 {
            w = 120.0;
        }
        if h <= 0.0 {
            h = 20.0;
        }
        let mut font_face = String::from("Arial");
        let mut font_px = (h - 6.0).clamp(10.0, 14.0);
        let mut bold = false;
        let mut face_at = None;
        if let Some((at, face)) = printable_run(data, 4).filter(|(at, _)| *at < 120) {
            face_at = Some(at);
            font_face = face;
            // The LOGFONT starts 18 (16-bit) or 28 (32-bit) bytes before the face name.
            for (start, wide) in [(at.wrapping_sub(18), false), (at.wrapping_sub(28), true)] {
                if start >= data.len() {
                    continue;
                }
                let height = if wide {
                    data.get(start..start + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()))
                } else {
                    data.get(start..start + 2).map(|b| i16::from_le_bytes([b[0], b[1]]) as i32)
                };
                let weight_at = if wide { start + 16 } else { start + 8 };
                if let Some(hgt) = height.filter(|v| v.unsigned_abs() >= 6 && v.unsigned_abs() <= 72) {
                    font_px = hgt.unsigned_abs() as f32;
                    bold = u16_at(weight_at) >= 600;
                    break;
                }
            }
        }
        let mut text = String::new();
        if kind == WidgetKind::Button {
            // Label(s) at the end of the block.
            let end = data.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
            let start = data[..end].iter().rposition(|&b| b == 0).map_or(0, |p| p + 1);
            if face_at.is_none_or(|f| start > f + 8) {
                text = String::from_utf8_lossy(&data[start..end]).into_owned();
            }
        }
        let (fg, bg) = match kind {
            WidgetKind::Button => ([0, 0, 0, 255], [212, 208, 200, 255]),
            _ => ([0, 0, 0, 255], [255, 255, 255, 255]),
        };
        Self {
            kind,
            w,
            h,
            font_face,
            font_px,
            bold,
            fg,
            bg,
            text,
            lines: Vec::new(),
            current: 0,
            top: 0,
            visible: true,
            enabled: true,
            read_only: false,
            focused: false,
            max_len: None,
            checked: 0,
            modified: false,
            clicked: false,
            selection_changed: false,
            double_clicked: false,
        }
    }

    pub fn line_height(&self) -> f32 {
        (self.font_px * 1.2).max(10.0)
    }

    pub fn clear_events(&mut self) {
        self.modified = false;
        self.clicked = false;
        self.selection_changed = false;
        self.double_clicked = false;
    }

    /// A left click at `(x, y)` relative to the widget's top-left corner.
    pub fn click(&mut self, x: f32, y: f32, double: bool) {
        if !self.enabled {
            return;
        }
        match self.kind {
            WidgetKind::Button => {
                self.clicked = true;
                self.checked = 1 - self.checked.clamp(0, 1);
            }
            WidgetKind::Edit => self.focused = true,
            WidgetKind::List | WidgetKind::Combo => {
                self.focused = true;
                let first = if self.kind == WidgetKind::Combo { 1 } else { 0 };
                let row = (y / self.line_height()).floor() as isize - first;
                let _ = x;
                if row >= 0 {
                    let line = self.top + row as usize + 1;
                    if line <= self.lines.len() {
                        if line != self.current {
                            self.selection_changed = true;
                        }
                        self.current = line;
                        if self.kind == WidgetKind::Combo {
                            self.text = self.lines[line - 1].clone();
                        }
                        self.double_clicked |= double;
                    }
                }
            }
        }
    }

    /// Typed characters (and backspace) for a focused edit box.
    pub fn type_text(&mut self, chars: &[char], backspace: usize) {
        if self.kind != WidgetKind::Edit || !self.focused || self.read_only || !self.enabled {
            return;
        }
        for _ in 0..backspace {
            if self.text.pop().is_some() {
                self.modified = true;
            }
        }
        for &c in chars {
            if c.is_control() {
                continue;
            }
            if self.max_len.is_some_and(|m| self.text.chars().count() >= m) {
                break;
            }
            self.text.push(c);
            self.modified = true;
        }
    }

    fn visible_rows(&self) -> usize {
        ((self.h / self.line_height()).floor() as usize).max(1)
    }

    fn line_index(&self, v: &Value) -> Option<usize> {
        let i = v.as_i32();
        (i >= 1 && (i as usize) <= self.lines.len()).then(|| i as usize - 1)
    }

    /// Condition `idx` (0 = the first in the object's menu).
    pub fn condition(&self, idx: usize) -> Option<bool> {
        Some(match (self.kind, idx) {
            (WidgetKind::Edit, 0) => self.visible,
            (WidgetKind::Edit, 1) => self.enabled,
            (WidgetKind::Edit, 2) => false,
            (WidgetKind::Edit, 3) => self.modified,
            (WidgetKind::Edit, 4) => self.focused,
            (WidgetKind::Edit, 5) => !self.text.trim().is_empty() && self.text.trim().parse::<f64>().is_ok(),
            (WidgetKind::Edit, 6) => false,
            (WidgetKind::Button, 0) => self.checked != 0,
            (WidgetKind::Button, 1) => self.clicked,
            (WidgetKind::Button, 2) => self.visible,
            (WidgetKind::Button, 3) => self.enabled,
            (WidgetKind::List | WidgetKind::Combo, 0) => self.visible,
            (WidgetKind::List | WidgetKind::Combo, 1) => self.enabled,
            (WidgetKind::List | WidgetKind::Combo, 2) => self.double_clicked,
            (WidgetKind::List | WidgetKind::Combo, 3) => self.selection_changed,
            (WidgetKind::List | WidgetKind::Combo, 4) => self.focused,
            _ => return None,
        })
    }

    /// Action `idx`; returns false if it isn't supported.
    pub fn action(&mut self, idx: usize, args: &[Value], fs: &SaveFs) -> bool {
        let s = |i: usize| args.get(i).map(Value::as_string).unwrap_or_default();
        let n = |i: usize| args.get(i).map_or(0, Value::as_i32);
        match self.kind {
            WidgetKind::Edit => match idx {
                0 => {
                    if let Some(b) = fs.read(&s(0)) {
                        self.text = String::from_utf8_lossy(&b).into_owned();
                    }
                }
                1 | 3 | 6..=8 | 10 | 11 | 14 | 15 | 21 | 22 | 31..=33 => {}
                2 => {
                    let _ = fs.write(&s(0), self.text.as_bytes());
                }
                4 => self.text = s(0),
                5 => self.text.push_str(&s(0)),
                9 => self.text.clear(),
                12 => self.visible = true,
                13 => self.visible = false,
                16 => self.focused = true,
                17 => self.enabled = true,
                18 => self.enabled = false,
                19 => self.read_only = true,
                20 => self.read_only = false,
                23 => self.max_len = Some(n(0).max(0) as usize),
                25 | 30 => self.focused = false,
                _ => return false,
            },
            WidgetKind::Button => match idx {
                0 => self.text = s(0),
                1 => self.visible = true,
                2 => self.visible = false,
                3 => self.enabled = true,
                4 => self.enabled = false,
                11 => self.checked = n(0),
                12 => self.checked = 0,
                _ => return false,
            },
            WidgetKind::List | WidgetKind::Combo => match idx {
                0 => {
                    if let Some(b) = fs.read(&s(0)) {
                        let text = String::from_utf8_lossy(&b).into_owned();
                        self.lines = text.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
                        self.current = 0;
                        self.top = 0;
                    }
                }
                1..=3 => {}
                4 => {
                    let mut out = self.lines.join("\r\n");
                    if !self.lines.is_empty() {
                        out.push_str("\r\n");
                    }
                    let _ = fs.write(&s(0), out.as_bytes());
                }
                5 => {
                    self.lines.clear();
                    self.current = 0;
                    self.top = 0;
                    if self.kind == WidgetKind::Combo {
                        self.text.clear();
                    }
                }
                6 => self.lines.push(s(0)),
                7 => {
                    let at = (n(0).max(1) as usize - 1).min(self.lines.len());
                    self.lines.insert(at, s(1));
                }
                8 => {
                    if let Some(i) = args.first().and_then(|v| self.line_index(v)) {
                        self.lines.remove(i);
                        if self.current > self.lines.len() {
                            self.current = self.lines.len();
                        }
                    }
                }
                9 => {
                    let i = n(0).max(0) as usize;
                    self.current = i.min(self.lines.len());
                    if self.kind == WidgetKind::Combo && self.current > 0 {
                        self.text = self.lines[self.current - 1].clone();
                    }
                }
                10 => self.visible = true,
                11 => self.visible = false,
                12 => self.focused = true,
                13 => self.enabled = true,
                14 => self.enabled = false,
                21 => self.focused = false,
                22 => self.top = 0,
                23 => self.top = (n(0).max(1) as usize - 1).min(self.lines.len().saturating_sub(1)),
                24 => self.top = self.lines.len().saturating_sub(self.visible_rows()),
                15..=20 | 25..=40 => {}
                _ => return false,
            },
        }
        true
    }

    /// Expression `idx`. Position / size expressions are answered by the caller.
    pub fn expression(&self, idx: usize, args: &[Value]) -> Option<Value> {
        let line = |i: usize| args.get(i).and_then(|v| self.line_index(v)).map(|i| self.lines[i].clone());
        Some(match self.kind {
            WidgetKind::Edit => match idx {
                0 => Value::Str(self.text.clone()),
                1 => Value::Str(String::new()),
                6 => Value::Float(self.text.trim().parse::<f64>().unwrap_or(0.0)),
                _ => return None,
            },
            WidgetKind::Button => match idx {
                // Check box state / selected radio button.
                0 | 4 => Value::Int(self.checked),
                _ => return None,
            },
            WidgetKind::List | WidgetKind::Combo => match idx {
                0 => Value::Int(self.current as i32),
                1 => Value::Str(if self.current > 0 {
                    self.lines[self.current - 1].clone()
                } else if self.kind == WidgetKind::Combo {
                    self.text.clone()
                } else {
                    String::new()
                }),
                2 | 3 | 5 | 6 => Value::Str(String::new()),
                4 => Value::Str(line(0).unwrap_or_default()),
                7 => Value::Int(self.lines.len() as i32),
                _ => return None,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> SaveFs {
        let root = std::env::temp_dir().join("fusion-rt-widget-test");
        SaveFs::new(&root.join("game.exe"), root.join("saves"))
    }

    #[test]
    fn list_lines_are_one_based() {
        let mut l = Widget::from_settings(WidgetKind::List, &[100, 0, 40, 0]);
        let f = fs();
        for t in ["a", "b", "c"] {
            l.action(6, &[Value::Str(t.into())], &f);
        }
        assert_eq!(l.expression(7, &[]).unwrap().as_i32(), 3);
        l.action(8, &[Value::Int(1)], &f);
        assert_eq!(l.expression(4, &[Value::Int(1)]).unwrap().as_string(), "b");
        l.action(8, &[Value::Int(0)], &f);
        assert_eq!(l.lines.len(), 2, "line 0 does not exist");
    }

    #[test]
    fn edit_typing_respects_limit_and_sets_modified() {
        let mut e = Widget::from_settings(WidgetKind::Edit, &[100, 0, 20, 0]);
        let f = fs();
        e.action(23, &[Value::Int(3)], &f);
        e.click(1.0, 1.0, false);
        e.type_text(&['a', 'b', 'c', 'd'], 0);
        assert_eq!(e.text, "abc");
        assert!(e.condition(3).unwrap());
        e.type_text(&[], 1);
        assert_eq!(e.text, "ab");
    }
}
