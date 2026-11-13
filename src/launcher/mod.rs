//! The game launcher shown when Rekindle is started without a game: a library of game
//! files with Play, desktop-shortcut and remove buttons. Games run in their own process.

pub mod library;
pub mod playlog;

use crate::render::settings::{Filter, Scaling, Settings};
use library::Entry;
use macroquad::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

const BG: Color = Color::new(0.07, 0.07, 0.09, 1.0);
const CARD: Color = Color::new(0.12, 0.12, 0.15, 1.0);
const CARD_HOVER: Color = Color::new(0.16, 0.16, 0.20, 1.0);
const TEXT: Color = Color::new(0.93, 0.93, 0.95, 1.0);
const MUTED: Color = Color::new(0.55, 0.56, 0.62, 1.0);
const ACCENT: Color = Color::new(0.96, 0.52, 0.18, 1.0);
const BUTTON: Color = Color::new(0.22, 0.22, 0.27, 1.0);

const ROW_H: f32 = 76.0;
const HEADER_H: f32 = 72.0;
const STATUS_H: f32 = 30.0;

/// A small flame drawn in code (the repository ships no image files).
pub fn flame_icon(size: usize) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    let s = size as f32;
    for y in 0..size {
        for x in 0..size {
            // Teardrop: round at the bottom, pointed at the top.
            let u = (x as f32 + 0.5) / s * 2.0 - 1.0;
            let v = (y as f32 + 0.5) / s; // 0 top .. 1 bottom
            let half_w = if v < 0.62 { 0.62 * (v / 0.62).powf(1.6) } else { 0.62 * (1.0 - ((v - 0.62) / 0.36).powi(2)).max(0.0).sqrt() };
            let lean = 0.12 * (1.0 - v) * (v * 9.0).sin();
            let d = ((u - lean).abs() / half_w.max(1e-3)).min(2.0);
            if d >= 1.0 || v > 0.98 {
                continue;
            }
            // Hot yellow core, orange body, red rim.
            let core = (1.0 - d) * v;
            let (r, g, b) = (255.0, 90.0 + 160.0 * core.min(1.0), 20.0 + 90.0 * (core - 0.5).max(0.0));
            let a = ((1.0 - d) * 6.0).min(1.0);
            let o = (y * size + x) * 4;
            px[o..o + 4].copy_from_slice(&[r as u8, g.min(255.0) as u8, b as u8, (a * 255.0) as u8]);
        }
    }
    px
}

pub fn window_conf() -> Conf {
    let mut icon = macroquad::miniquad::conf::Icon::miniquad_logo();
    icon.small.copy_from_slice(&flame_icon(16));
    icon.medium.copy_from_slice(&flame_icon(32));
    icon.big.copy_from_slice(&flame_icon(64));
    Conf {
        window_title: "Rekindle".into(),
        icon: Some(icon),
        window_width: 960,
        window_height: 640,
        window_resizable: true,
        high_dpi: false,
        ..Default::default()
    }
}

enum Msg {
    Found(Entry),
    Status(String),
    Done(String),
}

struct Launcher {
    entries: Vec<Entry>,
    icons: HashMap<PathBuf, Option<Texture2D>>,
    font: Option<Font>,
    scroll: f32,
    status: Option<(String, f64)>,
    busy: Option<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    last_click: Option<(usize, f64)>,
    /// Play sessions (re-read when the log file changes).
    sessions: Vec<playlog::Session>,
    totals: HashMap<String, (u64, String)>,
    log_stamp: Option<std::time::SystemTime>,
    next_log_check: f64,
    /// Showing the play log instead of the library.
    show_log: bool,
    /// Settings panel open for a game: (library index, its settings).
    settings_panel: Option<(usize, Settings)>,
}

impl Launcher {
    fn new() -> Self {
        let (tx, rx) = channel();
        let font = crate::render::text::find_font_file("Segoe UI", false, false)
            .or_else(|| crate::render::text::find_font_file("arial", false, false))
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| load_ttf_font_from_bytes(&b).ok());
        Self {
            entries: library::load(),
            icons: HashMap::new(),
            font,
            scroll: 0.0,
            status: None,
            busy: None,
            tx,
            rx,
            last_click: None,
            sessions: Vec::new(),
            totals: HashMap::new(),
            log_stamp: None,
            next_log_check: 0.0,
            show_log: false,
            settings_panel: None,
        }
    }

    fn refresh_log(&mut self) {
        if get_time() < self.next_log_check {
            return;
        }
        self.next_log_check = get_time() + 2.0;
        let stamp = std::fs::metadata(playlog::file()).and_then(|m| m.modified()).ok();
        if stamp != self.log_stamp || self.log_stamp.is_none() {
            self.log_stamp = stamp;
            self.sessions = playlog::load();
            self.totals = playlog::totals(&self.sessions);
        }
    }

    /// Open the Rekindle data folder (saves, play log, error logs) in the file manager.
    fn open_data_folder(&mut self) {
        let dir = library::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let opener = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        if let Err(e) = std::process::Command::new(opener).arg(&dir).spawn() {
            self.say(format!("Could not open {}: {e}", dir.display()));
        }
    }

    /// The play log view: newest sessions first.
    fn draw_log(&mut self, top: f32, bottom: f32) {
        let sw = screen_width();
        let total: u64 = self.sessions.iter().map(|s| s.seconds).sum();
        self.text(
            &format!(
                "{} session{}, {} played in total",
                self.sessions.len(),
                if self.sessions.len() == 1 { "" } else { "s" },
                playlog::duration(total)
            ),
            24.0,
            top + 22.0,
            17,
            MUTED,
        );
        let list_top = top + 36.0;
        let row_h = 30.0;
        let content_h = self.sessions.len() as f32 * row_h;
        let max_scroll = (content_h - (bottom - list_top)).max(0.0);
        self.scroll = (self.scroll - mouse_wheel().1 * 0.5).clamp(0.0, max_scroll);
        if self.sessions.is_empty() {
            self.text("Nothing yet: play a game and its session shows up here.", 24.0, list_top + 30.0, 18, MUTED);
        }
        for (k, s) in self.sessions.iter().rev().enumerate() {
            let y = list_top + k as f32 * row_h - self.scroll;
            if y < list_top - row_h || y > bottom {
                continue;
            }
            if k % 2 == 0 {
                draw_rectangle(16.0, y, sw - 32.0, row_h, CARD);
            }
            self.text(&s.started, 28.0, y + 20.0, 16, MUTED);
            self.text(&s.title, 200.0, y + 20.0, 17, TEXT);
            let d = playlog::duration(s.seconds);
            let w = self.text_width(&d, 17);
            self.text(&d, sw - 32.0 - w, y + 20.0, 17, TEXT);
        }
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), get_time() + 6.0));
    }

    fn text(&self, s: &str, x: f32, y: f32, size: u16, color: Color) {
        let params = TextParams { font: self.font.as_ref(), font_size: size, color, ..Default::default() };
        draw_text_ex(s, x, y, params);
    }

    fn text_width(&self, s: &str, size: u16) -> f32 {
        measure_text(s, self.font.as_ref(), size, 1.0).width
    }

    /// A button; returns true when clicked.
    fn button(&self, r: Rect, label: &str, accent: bool, enabled: bool) -> bool {
        let hover = enabled && r.contains(Vec2::from(mouse_position()));
        let base = if accent { ACCENT } else { BUTTON };
        let color = if !enabled {
            Color::new(base.r, base.g, base.b, 0.4)
        } else if hover {
            Color::new((base.r + 0.08).min(1.0), (base.g + 0.08).min(1.0), (base.b + 0.08).min(1.0), 1.0)
        } else {
            base
        };
        draw_rectangle(r.x, r.y, r.w, r.h, color);
        let tw = self.text_width(label, 17);
        let fg = if accent { Color::new(0.1, 0.06, 0.02, 1.0) } else { TEXT };
        self.text(label, r.x + (r.w - tw) / 2.0, r.y + r.h / 2.0 + 6.0, 17, fg);
        hover && is_mouse_button_pressed(MouseButton::Left)
    }

    fn icon(&mut self, game: &Path) -> Option<Texture2D> {
        self.icons
            .entry(game.to_path_buf())
            .or_insert_with(|| {
                let bytes = std::fs::read(library::icon_path(game)).ok()?;
                let t = Texture2D::from_file_with_format(&bytes, Some(ImageFormat::Png));
                t.set_filter(FilterMode::Linear);
                Some(t)
            })
            .clone()
    }

    fn has(&self, p: &Path) -> bool {
        let key = p.to_string_lossy().to_lowercase();
        self.entries.iter().any(|e| e.path.to_string_lossy().to_lowercase() == key)
    }

    /// Inspect files on a worker thread and send back the games found.
    fn add_paths(&mut self, files: Vec<PathBuf>, what: String) {
        let files: Vec<PathBuf> = files.into_iter().filter(|f| !self.has(f)).collect();
        if files.is_empty() {
            self.say("No new games to add.");
            return;
        }
        self.busy = Some(format!("Adding {what}..."));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut found = 0;
            let n = files.len();
            for (i, f) in files.into_iter().enumerate() {
                let name = f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = tx.send(Msg::Status(format!("Checking {name} ({}/{n})", i + 1)));
                if let Some(e) = library::inspect(&f) {
                    found += 1;
                    let _ = tx.send(Msg::Found(e));
                }
            }
            let _ = tx.send(Msg::Done(match found {
                0 => "No Fusion games found there.".into(),
                1 => "Added 1 game.".into(),
                k => format!("Added {k} games."),
            }));
        });
    }

    fn play(&mut self, i: usize) {
        let e = &self.entries[i];
        if !e.path.is_file() {
            self.say(format!("Can't find {} any more.", e.path.display()));
            return;
        }
        match std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .arg(&e.path)
                .current_dir(e.path.parent().unwrap_or(Path::new(".")))
                .spawn()
        }) {
            Ok(_) => self.say(format!("Starting {}...", e.title)),
            Err(err) => self.say(format!("Could not start the game: {err}")),
        }
    }

    fn update(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Found(e) => {
                    self.icons.remove(&e.path);
                    if library::add(&mut self.entries, e) {
                        library::save(&self.entries);
                    }
                }
                Msg::Status(s) => self.busy = Some(s),
                Msg::Done(s) => {
                    self.busy = None;
                    self.say(s);
                }
            }
        }
    }

    fn draw(&mut self) {
        let (sw, sh) = (screen_width(), screen_height());
        clear_background(BG);
        let idle = self.busy.is_none();

        // Header.
        self.text("Rekindle", 24.0, 46.0, 34, TEXT);
        let tw = self.text_width("Rekindle", 34);
        self.text("Fusion game launcher", 24.0 + tw + 14.0, 46.0, 17, MUTED);
        let add_folder = Rect::new(sw - 24.0 - 150.0, 20.0, 150.0, 36.0);
        let add_game = Rect::new(add_folder.x - 12.0 - 140.0, 20.0, 140.0, 36.0);
        let log_btn = Rect::new(add_game.x - 12.0 - 120.0, 20.0, 120.0, 36.0);
        let folder_btn = Rect::new(log_btn.x - 12.0 - 130.0, 20.0, 130.0, 36.0);
        if self.button(log_btn, if self.show_log { "Library" } else { "Play log" }, false, true) {
            self.show_log = !self.show_log;
            self.scroll = 0.0;
        }
        if self.button(folder_btn, "Open folder", false, true) {
            self.open_data_folder();
        }
        if self.button(add_game, "Add game...", true, idle) {
            let picked = rfd::FileDialog::new()
                .set_title("Choose a Fusion game")
                .add_filter("Fusion games", &["exe", "ccn", "dat"])
                .pick_file();
            if let Some(f) = picked {
                self.add_paths(vec![f], "game".into());
            }
        }
        if self.button(add_folder, "Add folder...", false, idle) {
            if let Some(dir) = rfd::FileDialog::new().set_title("Choose a folder with games").pick_folder() {
                let files = library::find_games(&dir);
                self.add_paths(files, "games from folder".into());
            }
        }

        // Game list (or the play log).
        let top = HEADER_H;
        let bottom = sh - STATUS_H;
        if self.settings_panel.is_some() {
            self.draw_settings_panel(top, bottom);
            draw_rectangle(0.0, bottom, sw, STATUS_H, BG);
            self.draw_status(sh);
            return;
        }
        if self.show_log {
            self.draw_log(top, bottom);
            draw_rectangle(0.0, bottom, sw, STATUS_H, BG);
            self.draw_status(sh);
            return;
        }
        let content_h = self.entries.len() as f32 * ROW_H;
        let max_scroll = (content_h - (bottom - top) + 12.0).max(0.0);
        self.scroll = (self.scroll - mouse_wheel().1 * 0.5).clamp(0.0, max_scroll);
        if self.entries.is_empty() {
            let msg = "No games yet. Use \"Add game...\" or \"Add folder...\" to build your library.";
            let w = self.text_width(msg, 18);
            self.text(msg, (sw - w) / 2.0, sh / 2.0, 18, MUTED);
        }
        let mouse = Vec2::from(mouse_position());
        let mut action: Option<(usize, u8)> = None;
        for i in 0..self.entries.len() {
            let y = top + i as f32 * ROW_H - self.scroll;
            if y + ROW_H < top || y > bottom {
                continue;
            }
            let row = Rect::new(16.0, y + 4.0, sw - 32.0, ROW_H - 8.0);
            let hover = row.contains(mouse) && mouse.y > top && mouse.y < bottom;
            draw_rectangle(row.x, row.y, row.w, row.h, if hover { CARD_HOVER } else { CARD });
            let icon_rect = Rect::new(row.x + 12.0, row.y + (row.h - 48.0) / 2.0, 48.0, 48.0);
            let path = self.entries[i].path.clone();
            match self.icon(&path) {
                Some(t) => draw_texture_ex(&t, icon_rect.x, icon_rect.y, WHITE, DrawTextureParams {
                    dest_size: Some(vec2(48.0, 48.0)),
                    ..Default::default()
                }),
                None => draw_rectangle(icon_rect.x, icon_rect.y, 48.0, 48.0, BUTTON),
            }
            let e = &self.entries[i];
            let missing = !e.path.is_file();
            self.text(&e.title, row.x + 76.0, row.y + 30.0, 21, if missing { MUTED } else { TEXT });
            let mut p = e.path.display().to_string();
            if missing {
                p = format!("(missing) {p}");
            }
            if let Some((secs, last)) = self.totals.get(&e.path.to_string_lossy().to_lowercase()) {
                let tw = self.text_width(&e.title, 21);
                let stats = format!("Played {}  -  last {last}", playlog::duration(*secs));
                self.text(&stats, row.x + 76.0 + tw + 16.0, row.y + 30.0, 15, ACCENT);
            }
            let max_w = row.w - 76.0 - 460.0;
            while self.text_width(&p, 14) > max_w && p.chars().count() > 8 {
                p = format!("...{}", p.chars().skip(4).collect::<String>());
            }
            self.text(&p, row.x + 76.0, row.y + 52.0, 14, MUTED);

            let by = row.y + (row.h - 34.0) / 2.0;
            let remove = Rect::new(row.x + row.w - 12.0 - 40.0, by, 40.0, 34.0);
            let mods = Rect::new(remove.x - 8.0 - 70.0, by, 70.0, 34.0);
            let settings = Rect::new(mods.x - 8.0 - 90.0, by, 90.0, 34.0);
            let shortcut = Rect::new(settings.x - 8.0 - 100.0, by, 100.0, 34.0);
            let play = Rect::new(shortcut.x - 8.0 - 100.0, by, 100.0, 34.0);
            let in_list = mouse.y > top && mouse.y < bottom;
            if self.button(play, "Play", true, !missing) && in_list {
                action = Some((i, 0));
            } else if self.button(settings, "Settings", false, true) && in_list {
                action = Some((i, 4));
            } else if self.button(shortcut, "Shortcut", false, !missing) && in_list {
                action = Some((i, 1));
            } else if self.button(mods, "Mods", false, true) && in_list {
                action = Some((i, 3));
            } else if self.button(remove, "X", false, true) && in_list {
                action = Some((i, 2));
            } else if hover && is_mouse_button_pressed(MouseButton::Left) {
                // Double-click a card to play.
                let now = get_time();
                if self.last_click.is_some_and(|(j, t)| j == i && now - t < 0.4) {
                    action = Some((i, 0));
                    self.last_click = None;
                } else {
                    self.last_click = Some((i, now));
                }
            }
        }
        // Keep the header and status bar on top of scrolled cards.
        draw_rectangle(0.0, bottom, sw, STATUS_H, BG);
        match action {
            Some((i, 0)) => self.play(i),
            Some((i, 1)) => {
                let e = self.entries[i].clone();
                match library::create_shortcut(&e.path, &e.title) {
                    Ok(p) => self.say(format!("Shortcut created: {}", p.display())),
                    Err(err) => self.say(format!("Could not create the shortcut: {err}")),
                }
            }
            Some((i, 4)) => {
                let path = self.entries[i].path.clone();
                let legacy = crate::runtime::files::default_save_root(&self.entries[i].title);
                self.settings_panel = Some((i, Settings::for_game(&path, Some(&legacy))));
            }
            Some((i, 3)) => {
                let path = self.entries[i].path.clone();
                match crate::mods::Mods::ensure_dir(&path) {
                    Ok(dir) => {
                        let opener = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
                        let _ = std::process::Command::new(opener).arg(&dir).spawn();
                        self.say(format!("Mods folder: {} (see README.txt inside)", dir.display()));
                    }
                    Err(err) => self.say(format!("Could not create the mods folder: {err}")),
                }
            }
            Some((i, _)) => {
                let e = self.entries.remove(i);
                library::save(&self.entries);
                self.say(format!("Removed {} from the library (the game files are untouched).", e.title));
            }
            None => {}
        }

        self.draw_status(sh);
    }

    /// A row of mutually exclusive choices; returns the index clicked, if any.
    fn choice_row(&self, y: f32, label: &str, hint: &str, options: &[&str], selected: usize) -> Option<usize> {
        let x0 = 48.0;
        self.text(label, x0, y + 23.0, 19, TEXT);
        self.text(hint, x0, y + 46.0, 14, MUTED);
        let mut x = 400.0;
        let mut clicked = None;
        for (k, o) in options.iter().enumerate() {
            let w = self.text_width(o, 17) + 30.0;
            if self.button(Rect::new(x, y + 6.0, w, 32.0), o, k == selected, true) {
                clicked = Some(k);
            }
            x += w + 8.0;
        }
        clicked
    }

    fn draw_settings_panel(&mut self, top: f32, _bottom: f32) {
        let sw = screen_width();
        let Some((i, mut s)) = self.settings_panel.clone() else { return };
        let title = self.entries.get(i).map(|e| e.title.clone()).unwrap_or_default();
        draw_rectangle(16.0, top + 4.0, sw - 32.0, 420.0, CARD);
        self.text(&format!("Settings - {title}"), 36.0, top + 40.0, 24, TEXT);
        self.text("Saved right away; applied the next time the game starts. In game: F11 fullscreen, F9 filter, F10 scaling, F1 HUD.", 36.0, top + 64.0, 14, MUTED);
        let mut y = top + 80.0;
        let mut changed = false;
        let fs = match s.fullscreen {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        };
        if let Some(k) = self.choice_row(y, "Fullscreen", "Start in fullscreen or in a window", &["Game default", "On", "Off"], fs) {
            s.fullscreen = [None, Some(true), Some(false)][k];
            changed = true;
        }
        y += 64.0;
        if let Some(k) = self.choice_row(y, "Window size", "Size of the window when not fullscreen", &["Auto", "1x", "2x", "3x"], s.window_scale.min(3) as usize) {
            s.window_scale = k as u8;
            changed = true;
        }
        y += 64.0;
        let sc = Scaling::ALL.iter().position(|v| *v == s.scaling).unwrap_or(0);
        if let Some(k) = self.choice_row(y, "Scaling", "Fit keeps the shape; Integer keeps pixels crisp", &["Fit", "Integer", "Stretch"], sc) {
            s.scaling = Scaling::ALL[k];
            changed = true;
        }
        y += 64.0;
        let fi = Filter::ALL.iter().position(|v| *v == s.filter).unwrap_or(1);
        if let Some(k) = self.choice_row(y, "Upscaling", "How the picture is enlarged: Enhanced is sharpest", &["Sharp pixels", "Smooth", "Enhanced"], fi) {
            s.filter = Filter::ALL[k];
            changed = true;
        }
        y += 64.0;
        if let Some(k) = self.choice_row(y, "Debug HUD", "Frame / tick overlay at start (F1 toggles)", &["Off", "On"], s.hud as usize) {
            s.hud = k == 1;
            changed = true;
        }
        if changed {
            s.save();
        }
        let done = Rect::new(sw - 36.0 - 120.0, top + 20.0, 120.0, 36.0);
        let close = self.button(done, "Done", true, true) || is_key_pressed(KeyCode::Escape);
        self.settings_panel = if close { None } else { Some((i, s)) };
    }

    fn draw_status(&self, sh: f32) {
        let line = match (&self.busy, &self.status) {
            (Some(b), _) => Some(b.clone()),
            (None, Some((s, until))) if get_time() < *until => Some(s.clone()),
            _ => None,
        };
        if let Some(s) = line {
            self.text(&s, 20.0, sh - 10.0, 16, MUTED);
        }
    }
}

pub async fn run() {
    let mut l = Launcher::new();
    loop {
        l.update();
        l.refresh_log();
        l.draw();
        next_frame().await;
    }
}
