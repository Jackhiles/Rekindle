//! The in-game overlay (F1): off, a compact status pill, or full panels with the game's
//! state, performance, view settings, unsupported features and the key list. F4 adds an
//! inspector for the object under the mouse.

use super::Viewer;
use macroquad::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HudMode {
    Off,
    Compact,
    Full,
}

impl HudMode {
    pub fn next(self) -> Self {
        match self {
            HudMode::Off => HudMode::Compact,
            HudMode::Compact => HudMode::Full,
            HudMode::Full => HudMode::Off,
        }
    }
}

/// Game speeds offered by Ctrl+- / Ctrl+=.
pub const SPEEDS: [f32; 7] = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 4.0];
pub const NORMAL_SPEED: usize = 3;

const PANEL: Color = Color::new(0.06, 0.07, 0.09, 0.82);
const EDGE: Color = Color::new(1.0, 1.0, 1.0, 0.10);
const TEXT: Color = Color::new(0.93, 0.94, 0.96, 1.0);
const MUTED: Color = Color::new(0.62, 0.65, 0.70, 1.0);
const ACCENT: Color = Color::new(1.0, 0.62, 0.22, 1.0);
const GOOD: Color = Color::new(0.40, 0.85, 0.50, 1.0);
const WARN: Color = Color::new(0.98, 0.78, 0.30, 1.0);
const BAD: Color = Color::new(0.98, 0.42, 0.40, 1.0);

/// Running numbers the overlay shows.
#[derive(Default)]
pub struct HudStats {
    /// Recent frame times in ms (oldest first).
    pub frame_ms: std::collections::VecDeque<f32>,
    /// Smoothed time spent in game logic and in drawing, ms per frame.
    pub logic_ms: f32,
    pub draw_ms: f32,
}

impl HudStats {
    pub fn record(&mut self, frame_ms: f32, logic_ms: f32, draw_ms: f32) {
        if self.frame_ms.len() >= 120 {
            self.frame_ms.pop_front();
        }
        self.frame_ms.push_back(frame_ms);
        let k = 0.1;
        self.logic_ms += (logic_ms - self.logic_ms) * k;
        self.draw_ms += (draw_ms - self.draw_ms) * k;
    }
}

/// A line of text: (text, colour).
type Line = Vec<(String, Color)>;

fn line(parts: &[(&str, Color)]) -> Line {
    parts.iter().map(|(s, c)| (s.to_string(), *c)).collect()
}

impl Viewer {
    fn hud_text(&self, s: &str, x: f32, y: f32, size: u16, color: Color) -> f32 {
        let params = TextParams { font: self.hud_font.as_ref(), font_size: size, color, ..Default::default() };
        draw_text_ex(s, x, y, params);
        measure_text(s, self.hud_font.as_ref(), size, 1.0).width
    }

    fn hud_width(&self, s: &str, size: u16) -> f32 {
        measure_text(s, self.hud_font.as_ref(), size, 1.0).width
    }

    /// A rounded panel.
    fn hud_panel(&self, x: f32, y: f32, w: f32, h: f32) {
        let r = 8.0;
        draw_rectangle(x + r, y, w - 2.0 * r, h, PANEL);
        draw_rectangle(x, y + r, r, h - 2.0 * r, PANEL);
        draw_rectangle(x + w - r, y + r, r, h - 2.0 * r, PANEL);
        for (cx, cy) in [(x + r, y + r), (x + w - r, y + r), (x + r, y + h - r), (x + w - r, y + h - r)] {
            // Quarter circles: draw the circle clipped by the rectangles around it.
            draw_circle(cx, cy, r, PANEL);
        }
        draw_rectangle_lines(x + r, y, w - 2.0 * r, 1.0, 1.0, EDGE);
    }

    /// A titled card of coloured lines; returns its height.
    fn hud_card(&self, x: f32, y: f32, w: f32, title: &str, lines: &[Line]) -> f32 {
        let row = 19.0;
        let h = 34.0 + row * lines.len() as f32;
        self.hud_panel(x, y, w, h);
        self.hud_text(title, x + 12.0, y + 22.0, 15, ACCENT);
        for (i, l) in lines.iter().enumerate() {
            let mut cx = x + 12.0;
            for (s, c) in l {
                cx += self.hud_text(s, cx, y + 44.0 + row * i as f32, 15, *c);
            }
        }
        h
    }

    pub(super) fn draw_hud(&self) {
        match self.hud {
            HudMode::Off => {}
            HudMode::Compact => self.draw_hud_compact(),
            HudMode::Full => self.draw_hud_full(),
        }
        if self.inspector {
            self.draw_inspector();
        }
    }

    fn state_label(&self) -> (String, Color) {
        if self.rt.finished {
            ("ended".into(), MUTED)
        } else if !self.running {
            ("paused".into(), WARN)
        } else {
            ("running".into(), GOOD)
        }
    }

    fn fps_color(fps: i32) -> Color {
        if fps >= 55 {
            GOOD
        } else if fps >= 30 {
            WARN
        } else {
            BAD
        }
    }

    fn draw_hud_compact(&self) {
        let f = self.rt.frame();
        let fps = get_fps();
        let (state, sc) = self.state_label();
        let parts: Vec<(String, Color)> = vec![
            (format!("{fps} fps"), Self::fps_color(fps)),
            ("   ".into(), MUTED),
            (format!("{}/{} ", self.rt.frame_index, self.rt.game.frames.len().saturating_sub(1)), MUTED),
            (format!("\"{}\"", f.name), TEXT),
            ("   ".into(), MUTED),
            (state, sc),
            (if self.speed_index != NORMAL_SPEED { format!("   x{}", SPEEDS[self.speed_index]) } else { String::new() }, ACCENT),
            (if self.audio.muted() { "   muted".into() } else { String::new() }, WARN),
        ];
        let w: f32 = parts.iter().map(|(s, _)| self.hud_width(s, 15)).sum::<f32>() + 24.0;
        self.hud_panel(8.0, 8.0, w, 28.0);
        let mut x = 20.0;
        for (s, c) in &parts {
            x += self.hud_text(s, x, 27.0, 15, *c);
        }
    }

    fn draw_hud_full(&self) {
        let f = self.rt.frame();
        let fps = get_fps();
        let (state, sc) = self.state_label();
        let col = 300.0;
        let (x0, mut y) = (8.0, 8.0);

        // Game.
        let speed = SPEEDS[self.speed_index];
        let game = [
            line(&[(&format!("{}/{}  ", self.rt.frame_index, self.rt.game.frames.len().saturating_sub(1)), MUTED), (&format!("\"{}\"", f.name), TEXT)]),
            line(&[(&format!("{} x {}", f.width, f.height), TEXT), ("   objects ", MUTED), (&self.rt.instances.iter().filter(|i| !i.destroyed).count().to_string(), TEXT)]),
            line(&[("tick ", MUTED), (&self.rt.tick.to_string(), TEXT), ("   time ", MUTED), (&format!("{:.1}s", self.rt.timer_ms / 1000.0), TEXT)]),
            line(&[(&state, sc), ("   speed ", MUTED), (&format!("x{speed}"), if speed == 1.0 { TEXT } else { ACCENT }), ("   sound ", MUTED), (if self.audio.muted() { "muted" } else { "on" }, if self.audio.muted() { WARN } else { TEXT })]),
        ];
        y += self.hud_card(x0, y, col, "GAME", &game) + 8.0;

        // Performance, with a frame-time graph.
        let tex_mb = self.textures.bytes as f32 / 1048576.0;
        let perf = [
            line(&[(&format!("{fps} fps"), Self::fps_color(fps)), ("   logic ", MUTED), (&format!("{:.2} ms", self.stats.logic_ms), TEXT), ("   draw ", MUTED), (&format!("{:.2} ms", self.stats.draw_ms), TEXT)]),
            line(&[("textures ", MUTED), (&format!("{tex_mb:.0} MB"), TEXT), ("   sounds playing ", MUTED), (&self.audio.voices().to_string(), TEXT)]),
            Vec::new(),
            Vec::new(),
        ];
        let h = self.hud_card(x0, y, col, "PERFORMANCE", &perf);
        self.draw_frame_graph(x0 + 12.0, y + h - 44.0, col - 24.0, 34.0);
        y += h + 8.0;

        // View.
        let layers: Line = {
            let mut l: Line = Vec::new();
            for (i, layer) in f.layers.iter().enumerate().take(10) {
                let off = self.layer_off.get(i).copied().unwrap_or(false);
                l.push((format!("{} ", (i + 1) % 10), if off { MUTED } else { ACCENT }));
                let name: String = layer.name.chars().take(10).collect();
                l.push((format!("{name}  "), if off { MUTED } else { TEXT }));
            }
            l
        };
        let mut layer_lines: Vec<Line> = Vec::new();
        // Wrap the layer list to the card width.
        let mut cur: Line = Vec::new();
        let mut w = 0.0;
        for part in layers.chunks(2) {
            let pw: f32 = part.iter().map(|(s, _)| self.hud_width(s, 15)).sum();
            if w + pw > col - 24.0 && !cur.is_empty() {
                layer_lines.push(std::mem::take(&mut cur));
                w = 0.0;
            }
            cur.extend(part.iter().cloned());
            w += pw;
        }
        if !cur.is_empty() {
            layer_lines.push(cur);
        }
        let scroll = self.current_scroll();
        let mut view = vec![
            line(&[("camera ", MUTED), (&format!("{:.0}, {:.0}", scroll.x, scroll.y), TEXT), ("   ", MUTED), (if self.fit { "whole frame" } else { "game view" }, TEXT)]),
            line(&[("scaling ", MUTED), (self.settings.scaling.name(), TEXT), ("   filter ", MUTED), (self.settings.filter.name(), TEXT)]),
            line(&[("hidden ", MUTED), (if self.show_hidden { "shown" } else { "off" }, if self.show_hidden { ACCENT } else { TEXT }), ("   boxes ", MUTED), (if self.debug_boxes { "on" } else { "off" }, if self.debug_boxes { ACCENT } else { TEXT })]),
        ];
        view.extend(layer_lines);
        y += self.hud_card(x0, y, col, "VIEW", &view) + 8.0;

        // Unsupported features.
        let un = self.rt.unimplemented.borrow();
        if !un.is_empty() {
            let mut top: Vec<_> = un.iter().collect();
            top.sort_by(|a, b| b.1.cmp(a.1));
            let lines: Vec<Line> = top
                .iter()
                .take(6)
                .map(|(k, n)| line(&[(k.as_str(), WARN), (&format!("  x{n}"), MUTED)]))
                .collect();
            self.hud_card(x0, y, col, &format!("UNSUPPORTED ({})", top.len()), &lines);
        }
        drop(un);

        // Keys, along the bottom.
        let keys: [(&str, &str); 14] = [
            ("F1", "overlay"),
            ("F2", "pause"),
            ("F3", "step"),
            ("F4", "inspect"),
            ("F5", "restart"),
            ("F6", "boxes"),
            ("F7", "hidden"),
            ("F8", "whole frame"),
            ("F9", "filter"),
            ("F10", "scaling"),
            ("F11", "fullscreen"),
            ("Ctrl F12", "screenshot"),
            ("Ctrl -/=", "speed"),
            ("Ctrl M", "mute"),
        ];
        let more: [(&str, &str); 3] = [("Ctrl PgUp/PgDn", "frame"), ("Ctrl 1-0", "layer"), ("Ctrl Q", "quit")];
        let sw = screen_width();
        let mut rows: Vec<Vec<(&str, &str)>> = vec![Vec::new()];
        let mut w = 0.0;
        for k in keys.iter().chain(more.iter()) {
            let kw = self.hud_width(k.0, 14) + self.hud_width(k.1, 14) + 30.0;
            if w + kw > sw - 40.0 && !rows.last().expect("non-empty").is_empty() {
                rows.push(Vec::new());
                w = 0.0;
            }
            rows.last_mut().expect("non-empty").push(*k);
            w += kw;
        }
        let h = 12.0 + 20.0 * rows.len() as f32;
        let top = screen_height() - h - 8.0;
        self.hud_panel(8.0, top, sw - 16.0, h);
        for (r, row) in rows.iter().enumerate() {
            let mut x = 20.0;
            let ty = top + 22.0 + 20.0 * r as f32;
            for (k, what) in row {
                // Key cap.
                let kw = self.hud_width(k, 14);
                draw_rectangle(x - 4.0, ty - 14.0, kw + 8.0, 18.0, Color::new(1.0, 1.0, 1.0, 0.12));
                self.hud_text(k, x, ty, 14, TEXT);
                x += kw + 8.0;
                x += self.hud_text(what, x, ty, 14, MUTED) + 18.0;
            }
        }
    }

    /// Frame times as bars (green under 17 ms, yellow under 34, red above).
    fn draw_frame_graph(&self, x: f32, y: f32, w: f32, h: f32) {
        draw_rectangle(x, y, w, h, Color::new(0.0, 0.0, 0.0, 0.25));
        let n = self.stats.frame_ms.len().max(1);
        let bw = w / 120.0;
        let scale = h / 50.0; // 50 ms = full height
        for (i, ms) in self.stats.frame_ms.iter().enumerate() {
            let bh = (ms * scale).min(h);
            let c = if *ms < 17.5 {
                GOOD
            } else if *ms < 34.0 {
                WARN
            } else {
                BAD
            };
            let bx = x + w - (n - i) as f32 * bw;
            draw_rectangle(bx, y + h - bh, (bw - 0.5).max(0.5), bh, Color::new(c.r, c.g, c.b, 0.85));
        }
        // 60 fps line.
        let ly = y + h - 16.7 * scale;
        draw_line(x, ly, x + w, ly, 1.0, Color::new(1.0, 1.0, 1.0, 0.25));
    }

    /// Details of the object under the mouse, next to the pointer.
    fn draw_inspector(&self) {
        let (mx, my) = self.input.mouse;
        let Some(idx) = self.rt.object_at(mx, my) else {
            let m = Vec2::from(mouse_position());
            self.hud_panel(m.x + 16.0, m.y + 16.0, 150.0, 28.0);
            self.hud_text("no object here", m.x + 28.0, m.y + 35.0, 14, MUTED);
            return;
        };
        let inst = &self.rt.instances[idx];
        let obj = self.rt.game.object(inst.oi);
        let name = obj.map_or_else(|| format!("#{}", inst.oi), |o| if o.name.is_empty() { format!("#{}", inst.oi) } else { o.name.clone() });
        let (bx, by, bw, bh) = self.rt.bounds_of(idx);
        let mut lines = vec![
            line(&[(&format!("{:?}", inst.kind), MUTED), ("   layer ", MUTED), (&(inst.layer + 1).to_string(), TEXT), ("   id ", MUTED), (&inst.id.to_string(), TEXT)]),
            line(&[("at ", MUTED), (&format!("{:.0}, {:.0}", inst.x, inst.y), TEXT), ("   size ", MUTED), (&format!("{bw:.0} x {bh:.0}"), TEXT)]),
            line(&[("box ", MUTED), (&format!("{bx:.0}, {by:.0}"), TEXT), ("   ", MUTED), (if inst.visible { "visible" } else { "hidden" }, if inst.visible { TEXT } else { WARN }), (if inst.fixed { "  fixed" } else { "" }, MUTED)]),
            line(&[("anim ", MUTED), (&format!("{} frame {}", inst.anim.anim, inst.anim.frame), TEXT), ("   dir ", MUTED), (&inst.direction().to_string(), TEXT), ("   speed ", MUTED), (&format!("{:.0}", inst.current_speed()), TEXT)]),
        ];
        let vals: Vec<String> = inst.values.iter().enumerate().filter(|(_, v)| v.as_f64() != 0.0 || !v.as_string().is_empty() && v.as_string() != "0").take(8).map(|(i, v)| format!("{}={}", (b'A' + i as u8) as char, v.as_string())).collect();
        if !vals.is_empty() {
            lines.push(line(&[("values ", MUTED), (&vals.join(" "), TEXT)]));
        }
        let strs: Vec<String> = inst.strings.iter().enumerate().filter(|(_, s)| !s.is_empty()).take(3).map(|(i, s)| format!("{}=\"{}\"", (b'A' + i as u8) as char, s.chars().take(24).collect::<String>())).collect();
        if !strs.is_empty() {
            lines.push(line(&[("strings ", MUTED), (&strs.join(" "), TEXT)]));
        }
        if inst.flags != 0 {
            lines.push(line(&[("flags ", MUTED), (&format!("{:032b}", inst.flags).trim_start_matches('0').to_string(), TEXT)]));
        }
        if !inst.text.is_empty() {
            lines.push(line(&[("text ", MUTED), (&format!("\"{}\"", inst.text.chars().take(32).collect::<String>()), TEXT)]));
        }
        let w = 330.0;
        let h = 34.0 + 19.0 * lines.len() as f32;
        let m = Vec2::from(mouse_position());
        let x = if m.x + 16.0 + w > screen_width() { m.x - 16.0 - w } else { m.x + 16.0 };
        let y = if m.y + 16.0 + h > screen_height() { (screen_height() - h - 8.0).max(8.0) } else { m.y + 16.0 };
        self.hud_card(x, y, w, &name, &lines);
        // Outline the object.
        let (o, s) = self.viewport();
        let sc = self.current_scroll();
        let coef = if self.running { Vec2::ONE } else { Vec2::ONE };
        let p = o + (vec2(bx as f32, by as f32) - sc * coef) * s;
        draw_rectangle_lines(p.x, p.y, bw as f32 * s, bh as f32 * s, 2.0, ACCENT);
    }
}
