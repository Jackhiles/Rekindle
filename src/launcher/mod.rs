//! The game launcher shown when Rekindle is started without a game: a library of game
//! files with Play, desktop-shortcut and remove buttons. Games run in their own process.

pub mod library;
pub mod steam;
pub mod steam_shortcuts;
mod art;
pub mod update;

/// File / folder pickers.
mod dialogs {
    use std::path::PathBuf;

    #[cfg(not(target_os = "linux"))]
    pub fn pick_game() -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Choose a Fusion game")
            .add_filter("Fusion games", &["exe", "ccn", "dat"])
            .pick_file()
    }

    #[cfg(not(target_os = "linux"))]
    pub fn pick_folder() -> Option<PathBuf> {
        rfd::FileDialog::new().set_title("Choose a folder with games").pick_folder()
    }

    /// Ask the desktop's own picker: kdialog (KDE, e.g. Steam Deck desktop mode) or zenity.
    #[cfg(target_os = "linux")]
    fn run(kdialog: &[&str], zenity: &[&str]) -> Option<PathBuf> {
        for (cmd, args) in [("kdialog", kdialog), ("zenity", zenity)] {
            if let Ok(out) = std::process::Command::new(cmd).args(args).output() {
                let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return (out.status.success() && !p.is_empty()).then(|| PathBuf::from(p));
            }
        }
        None
    }

    #[cfg(target_os = "linux")]
    pub fn pick_game() -> Option<PathBuf> {
        run(
            &["--title", "Choose a Fusion game", "--getopenfilename", ".", "*.exe *.ccn *.dat"],
            &["--file-selection", "--title=Choose a Fusion game", "--file-filter=Fusion games | *.exe *.ccn *.dat"],
        )
    }

    #[cfg(target_os = "linux")]
    pub fn pick_folder() -> Option<PathBuf> {
        run(
            &["--title", "Choose a folder with games", "--getexistingdirectory", "."],
            &["--file-selection", "--directory", "--title=Choose a folder with games"],
        )
    }
}
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
/// Vertical step between rows of the settings panel.
const ROW_STEP: f32 = 54.0;
const HEADER_H: f32 = 72.0;
const STATUS_H: f32 = 30.0;

/// Rekindle's icon (assets/icon, drawn by make_icon.py) and the launcher's UI font
/// (Inter, SIL Open Font License; see assets/fonts).
pub const ICON_PNG: &[u8] = include_bytes!("../../assets/icon/rekindle-256.png");
pub(crate) const FONT_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
pub(crate) const FONT_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");

/// The icon decoded to 256 x 256 RGBA.
pub fn icon_image() -> Option<crate::ctf_loader::icon::IconImage> {
    let dec = png::Decoder::new(std::io::Cursor::new(ICON_PNG));
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    (info.color_type == png::ColorType::Rgba).then(|| crate::ctf_loader::icon::IconImage {
        width: info.width,
        height: info.height,
        rgba: buf[..(info.width * info.height * 4) as usize].to_vec(),
    })
}

/// Window / taskbar icon in the sizes the window system wants.
pub fn window_icon() -> Option<macroquad::miniquad::conf::Icon> {
    let img = icon_image()?;
    let mut icon = macroquad::miniquad::conf::Icon::miniquad_logo();
    icon.small.copy_from_slice(&img.resized(16));
    icon.medium.copy_from_slice(&img.resized(32));
    icon.big.copy_from_slice(&img.resized(64));
    Some(icon)
}

pub fn window_conf() -> Conf {
    let icon = window_icon();
    Conf {
        window_title: "Rekindle".into(),
        icon,
        window_width: 960,
        window_height: 640,
        window_resizable: true,
        high_dpi: false,
        ..Default::default()
    }
}

/// Launcher-wide options in `<data dir>\Rekindle\launcher.ini`.
#[derive(Debug, Clone)]
struct LauncherConfig {
    /// Copy Steam games into `storage` instead of linking them where Steam installed them.
    steam_copy: bool,
    storage: PathBuf,
    /// Don't offer to add Rekindle itself to Steam.
    steam_self_never: bool,
}

impl LauncherConfig {
    fn file() -> PathBuf {
        library::data_dir().join("launcher.ini")
    }

    fn load() -> Self {
        let mut c = Self { steam_copy: false, storage: library::data_dir().join("games"), steam_self_never: false };
        if let Ok(text) = std::fs::read_to_string(Self::file()) {
            for line in text.lines() {
                match line.split_once('=') {
                    Some(("steam_mode", v)) => c.steam_copy = v.trim() == "copy",
                    Some(("storage", v)) if !v.trim().is_empty() => c.storage = PathBuf::from(v.trim()),
                    Some(("add_self_to_steam", v)) => c.steam_self_never = v.trim() == "never",
                    _ => {}
                }
            }
        }
        c
    }

    fn save(&self) {
        let text = format!(
            "[launcher]\nsteam_mode={}\nstorage={}\nadd_self_to_steam={}\n",
            if self.steam_copy { "copy" } else { "link" },
            self.storage.display(),
            if self.steam_self_never { "never" } else { "ask" }
        );
        let _ = std::fs::create_dir_all(library::data_dir());
        let _ = std::fs::write(Self::file(), text);
    }
}

enum Msg {
    Found(Entry),
    Status(String),
    Done(String),
    UpdateAvailable(update::Release),
    /// Steam is open and has no Rekindle shortcut: offer to add one.
    OfferSteamShortcut,
    /// A line for the Steam panel's results.
    SteamLine(String),
    UpdateInstalled(Result<PathBuf, String>),
}

struct Launcher {
    entries: Vec<Entry>,
    icons: HashMap<PathBuf, Option<Texture2D>>,
    font: Option<Font>,
    /// Title and button labels.
    bold: Option<Font>,
    logo: Option<Texture2D>,
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
    /// The Steam games panel is open.
    steam_panel: bool,
    steam_lines: Vec<String>,
    config: LauncherConfig,
    /// A newer release, once the background check found one.
    update: Option<update::Release>,
    updating: bool,
    /// Gamepad navigation: selected card, and this frame's button presses.
    pad: Option<gilrs::Gilrs>,
    pad_held: std::collections::HashSet<gilrs::Button>,
    selected: Option<usize>,
    pad_back: bool,
    pad_a: bool,
    /// Games started from here that are still running. While one is, the controller
    /// belongs to the game (gilrs sees it even when this window is in the background).
    running: Vec<(PathBuf, std::process::Child)>,
    /// Steam is installed (offer "Add to Steam").
    has_steam: bool,
    /// Asking to restart Steam before adding this library entry to it.
    steam_confirm: Option<usize>,
    /// Offering (in the status bar) to add Rekindle itself to Steam.
    steam_offer: bool,
}

impl Launcher {
    fn new() -> Self {
        let (tx, rx) = channel();
        let font = load_ttf_font_from_bytes(FONT_REGULAR).ok();
        let bold = load_ttf_font_from_bytes(FONT_SEMIBOLD).ok();
        Self {
            entries: library::load(),
            icons: HashMap::new(),
            font,
            bold,
            logo: None,
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
            steam_panel: false,
            steam_lines: Vec::new(),
            config: LauncherConfig::load(),
            update: None,
            updating: false,
            pad: gilrs::Gilrs::new().ok(),
            pad_held: Default::default(),
            selected: None,
            pad_back: false,
            pad_a: false,
            running: Vec::new(),
            has_steam: !steam::steam_roots().is_empty(),
            steam_confirm: None,
            steam_offer: false,
        }
    }

    /// Controller navigation: up / down select a game, A plays it, Y opens its settings,
    /// B closes a panel. Returns the action for the selected game (0 play, 4 settings).
    fn pad_nav(&mut self) -> Option<(usize, u8)> {
        use gilrs::{Axis, Button};
        self.pad_back = false;
        self.pad_a = false;
        self.running.retain_mut(|(_, c)| matches!(c.try_wait(), Ok(None)));
        let game_running = !self.running.is_empty();
        let g = self.pad.as_mut()?;
        while g.next_event().is_some() {}
        let (_, p) = g.gamepads().find(|(_, p)| p.is_connected())?;
        let ly = p.value(Axis::LeftStickY);
        let mut now = std::collections::HashSet::new();
        for b in [Button::DPadUp, Button::DPadDown, Button::South, Button::East, Button::North] {
            if p.is_pressed(b) {
                now.insert(b);
            }
        }
        if ly > 0.6 {
            now.insert(Button::DPadUp);
        }
        if ly < -0.6 {
            now.insert(Button::DPadDown);
        }
        if game_running {
            // Buttons pressed in the game are not for the launcher; also don't act on them
            // when the game closes while they're still held.
            self.pad_held = now;
            return None;
        }
        let pressed = |b: Button| now.contains(&b) && !self.pad_held.contains(&b);
        let (up, down, a, b, y) = (
            pressed(Button::DPadUp),
            pressed(Button::DPadDown),
            pressed(Button::South),
            pressed(Button::East),
            pressed(Button::North),
        );
        self.pad_held = now;
        self.pad_back = b;
        self.pad_a = a;
        if self.entries.is_empty() || self.settings_panel.is_some() || self.show_log || self.steam_panel || self.steam_confirm.is_some() {
            if b && self.show_log {
                self.show_log = false;
            }
            return None;
        }
        let n = self.entries.len();
        let cur = self.selected.unwrap_or(0).min(n - 1);
        if up || down {
            let next = if self.selected.is_none() { 0 } else if up { cur.saturating_sub(1) } else { (cur + 1).min(n - 1) };
            self.selected = Some(next);
            // Keep the selection on screen.
            let y = next as f32 * ROW_H;
            let visible = screen_height() - HEADER_H - STATUS_H;
            if y < self.scroll {
                self.scroll = y;
            } else if y + ROW_H > self.scroll + visible {
                self.scroll = y + ROW_H - visible;
            }
        }
        match (self.selected, a, y) {
            (Some(i), true, _) => Some((i.min(n - 1), 0)),
            (Some(i), _, true) => Some((i.min(n - 1), 4)),
            _ => None,
        }
    }

    /// Look for a newer release in the background (not for developer builds).
    fn start_update_check(&self) {
        update::cleanup();
        if update::is_dev_build() || std::env::var_os("REKINDLE_NO_UPDATE_CHECK").is_some() {
            return;
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            if let Ok(Some(r)) = update::check() {
                let _ = tx.send(Msg::UpdateAvailable(r));
            }
        });
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

    /// `s` shortened with "..." to fit `max_w` pixels.
    fn fit(&self, s: &str, size: u16, max_w: f32) -> String {
        if self.text_width(s, size) <= max_w {
            return s.to_string();
        }
        let mut chars: Vec<char> = s.chars().collect();
        while !chars.is_empty() && self.text_width(&format!("{}...", chars.iter().collect::<String>()), size) > max_w {
            chars.pop();
        }
        format!("{}...", chars.iter().collect::<String>().trim_end())
    }

    fn bold_text(&self, s: &str, x: f32, y: f32, size: u16, color: Color) {
        let params = TextParams { font: self.bold.as_ref().or(self.font.as_ref()), font_size: size, color, ..Default::default() };
        draw_text_ex(s, x, y, params);
    }

    fn bold_width(&self, s: &str, size: u16) -> f32 {
        measure_text(s, self.bold.as_ref().or(self.font.as_ref()), size, 1.0).width
    }

    fn logo(&mut self) -> Option<Texture2D> {
        if self.logo.is_none() {
            let t = Texture2D::from_file_with_format(ICON_PNG, Some(ImageFormat::Png));
            t.set_filter(FilterMode::Linear);
            self.logo = Some(t);
        }
        self.logo.clone()
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
        let tw = self.bold_width(label, 16);
        let fg = if accent { Color::new(0.1, 0.06, 0.02, 1.0) } else { TEXT };
        self.bold_text(label, r.x + (r.w - tw) / 2.0, r.y + r.h / 2.0 + 6.0, 16, fg);
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
            let single = (n == 1).then(|| files[0].clone());
            for (i, f) in files.into_iter().enumerate() {
                let name = f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = tx.send(Msg::Status(format!("Checking {name} ({}/{n})", i + 1)));
                if let Some(e) = library::inspect(&f) {
                    found += 1;
                    let _ = tx.send(Msg::Found(e));
                }
            }
            let _ = tx.send(Msg::Done(match found {
                0 if single.is_some() => library::why_not(single.as_deref().expect("checked")),
                0 => "No Fusion games found there.".into(),
                1 => "Added 1 game.".into(),
                k => format!("Added {k} games."),
            }));
        });
    }

    fn is_running(&mut self, path: &Path) -> bool {
        self.running.retain_mut(|(_, c)| matches!(c.try_wait(), Ok(None)));
        self.running.iter().any(|(p, _)| p == path)
    }

    fn play(&mut self, i: usize) {
        let e = self.entries[i].clone();
        if !e.path.is_file() {
            self.say(format!("Can't find {} any more.", e.path.display()));
            return;
        }
        if self.is_running(&e.path) {
            self.say(format!("{} is already running.", e.title));
            return;
        }
        match update::program_path().and_then(|exe| {
            std::process::Command::new(exe)
                .arg(&e.path)
                .current_dir(e.path.parent().unwrap_or(Path::new(".")))
                .spawn()
        }) {
            Ok(child) => {
                self.running.push((e.path.clone(), child));
                self.say(format!("Starting {}...", e.title));
            }
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
                Msg::UpdateAvailable(r) => self.update = Some(r),
                Msg::OfferSteamShortcut => self.steam_offer = true,
                Msg::SteamLine(l) => self.steam_lines.push(l),
                Msg::UpdateInstalled(Ok(exe)) => {
                    // Start the new version and close this one.
                    match std::process::Command::new(&exe).spawn() {
                        Ok(_) => std::process::exit(0),
                        Err(e) => {
                            self.updating = false;
                            self.say(format!("Updated; please restart Rekindle ({e})"));
                        }
                    }
                }
                Msg::UpdateInstalled(Err(e)) => {
                    self.updating = false;
                    self.busy = None;
                    self.say(format!("Update failed: {e}"));
                }
            }
        }
    }

    fn draw(&mut self) {
        let (sw, sh) = (screen_width(), screen_height());
        clear_background(BG);
        let idle = self.busy.is_none();

        // Header.
        // Logo and title.
        if let Some(t) = self.logo() {
            draw_texture_ex(&t, 22.0, 18.0, WHITE, DrawTextureParams { dest_size: Some(vec2(38.0, 38.0)), ..Default::default() });
        }
        self.bold_text("Rekindle", 70.0, 47.0, 32, TEXT);
        let tw = self.bold_width("Rekindle", 32) + 46.0;
        let subtitle = format!("Fusion game launcher  v{}", update::current_version());
        // Only when it fits left of the header buttons (4 buttons + gaps = 586 px).
        if 24.0 + tw + 14.0 + self.text_width(&subtitle, 17) < sw - 24.0 - 738.0 - 12.0 {
            self.text(&subtitle, 24.0 + tw + 14.0, 46.0, 17, MUTED);
        }
        let add_folder = Rect::new(sw - 24.0 - 150.0, 20.0, 150.0, 36.0);
        let add_game = Rect::new(add_folder.x - 12.0 - 140.0, 20.0, 140.0, 36.0);
        let log_btn = Rect::new(add_game.x - 12.0 - 120.0, 20.0, 120.0, 36.0);
        let folder_btn = Rect::new(log_btn.x - 12.0 - 130.0, 20.0, 130.0, 36.0);
        let steam_btn = Rect::new(folder_btn.x - 12.0 - 140.0, 20.0, 140.0, 36.0);
        if self.button(steam_btn, "Steam games", false, true) {
            self.steam_panel = !self.steam_panel;
            self.settings_panel = None;
            self.show_log = false;
        }
        if self.button(log_btn, if self.show_log { "Library" } else { "Play log" }, false, true) {
            self.show_log = !self.show_log;
            self.scroll = 0.0;
        }
        if self.button(folder_btn, "Open folder", false, true) {
            self.open_data_folder();
        }
        if self.button(add_game, "Add game...", true, idle) {
            let picked = dialogs::pick_game();
            if let Some(f) = picked {
                self.add_paths(vec![f], "game".into());
            }
        }
        if self.button(add_folder, "Add folder...", false, idle) {
            if let Some(dir) = dialogs::pick_folder() {
                let files = library::find_games(&dir);
                self.add_paths(files, "games from folder".into());
            }
        }

        // Game list (or the play log).
        let top = HEADER_H;
        let bottom = sh - STATUS_H;
        // Read the controller every frame (panels use B / A from it too).
        let pad_action = self.pad_nav();
        if let Some(i) = self.steam_confirm {
            self.draw_steam_confirm(top, i);
            draw_rectangle(0.0, bottom, sw, STATUS_H, BG);
            self.draw_status(sh);
            return;
        }
        if self.steam_panel {
            self.draw_steam_panel(top);
            draw_rectangle(0.0, bottom, sw, STATUS_H, BG);
            self.draw_status(sh);
            return;
        }
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
        let mut action: Option<(usize, u8)> = pad_action;
        for i in 0..self.entries.len() {
            let y = top + i as f32 * ROW_H - self.scroll;
            if y + ROW_H < top || y > bottom {
                continue;
            }
            let row = Rect::new(16.0, y + 4.0, sw - 32.0, ROW_H - 8.0);
            let hover = row.contains(mouse) && mouse.y > top && mouse.y < bottom;
            draw_rectangle(row.x, row.y, row.w, row.h, if hover { CARD_HOVER } else { CARD });
            if self.selected == Some(i) {
                draw_rectangle_lines(row.x, row.y, row.w, row.h, 3.0, ACCENT);
            }
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
            let buttons_w = if self.has_steam { 558.0 } else { 470.0 };
            let title_w = row.w - 76.0 - buttons_w;
            let title = self.fit(&e.title, 21, title_w);
            self.text(&title, row.x + 76.0, row.y + 30.0, 21, if missing { MUTED } else { TEXT });
            let mut p = e.path.display().to_string();
            if missing {
                p = format!("(missing) {p}");
            }
            if let Some((secs, last)) = self.totals.get(&e.path.to_string_lossy().to_lowercase()) {
                let tw = self.text_width(&title, 21);
                let stats = format!("Played {}  -  last {last}", playlog::duration(*secs));
                // Next to the title when there's room, otherwise in place of the path.
                if tw + 16.0 + self.text_width(&stats, 15) <= title_w {
                    self.text(&stats, row.x + 76.0 + tw + 16.0, row.y + 30.0, 15, ACCENT);
                } else {
                    p = stats;
                }
            }
            let max_w = row.w - 76.0 - buttons_w + 10.0;
            while self.text_width(&p, 14) > max_w && p.chars().count() > 8 {
                p = format!("...{}", p.chars().skip(4).collect::<String>());
            }
            self.text(&p, row.x + 76.0, row.y + 52.0, 14, MUTED);

            let by = row.y + (row.h - 34.0) / 2.0;
            let remove = Rect::new(row.x + row.w - 12.0 - 40.0, by, 40.0, 34.0);
            let mods = Rect::new(remove.x - 8.0 - 70.0, by, 70.0, 34.0);
            let settings = Rect::new(mods.x - 8.0 - 90.0, by, 90.0, 34.0);
            let shortcut = Rect::new(settings.x - 8.0 - 100.0, by, 100.0, 34.0);
            let to_steam = Rect::new(shortcut.x - 8.0 - 80.0, by, 80.0, 34.0);
            let play = if self.has_steam { Rect::new(to_steam.x - 8.0 - 100.0, by, 100.0, 34.0) } else { Rect::new(shortcut.x - 8.0 - 100.0, by, 100.0, 34.0) };
            let in_list = mouse.y > top && mouse.y < bottom;
            let running = self.running.iter().any(|(p, _)| *p == path);
            if self.button(play, if running { "Running" } else { "Play" }, true, !missing && !running) && in_list {
                action = Some((i, 0));
            } else if self.button(settings, "Settings", false, true) && in_list {
                action = Some((i, 4));
            } else if self.button(shortcut, "Shortcut", false, !missing) && in_list {
                action = Some((i, 1));
            } else if self.has_steam && self.button(to_steam, "Steam", false, !missing && self.busy.is_none()) && in_list {
                action = Some((i, 5));
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
            Some((i, 5)) => {
                if steam_shortcuts::steam_running() && !steam_shortcuts::in_gaming_mode() {
                    self.steam_confirm = Some(i);
                } else {
                    self.add_to_steam(i);
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
        draw_rectangle(16.0, top + 4.0, sw - 32.0, screen_height() - top - STATUS_H - 8.0, CARD);
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
        y += ROW_STEP;
        if let Some(k) = self.choice_row(y, "Window size", "Size of the window when not fullscreen", &["Auto", "1x", "2x", "3x"], s.window_scale.min(3) as usize) {
            s.window_scale = k as u8;
            changed = true;
        }
        y += ROW_STEP;
        let sc = Scaling::ALL.iter().position(|v| *v == s.scaling).unwrap_or(0);
        if let Some(k) = self.choice_row(y, "Scaling", "Fit keeps the shape; Integer keeps pixels crisp", &["Fit", "Integer", "Stretch"], sc) {
            s.scaling = Scaling::ALL[k];
            changed = true;
        }
        y += ROW_STEP;
        let fi = Filter::ALL.iter().position(|v| *v == s.filter).unwrap_or(1);
        if let Some(k) = self.choice_row(y, "Upscaling", "How the picture is enlarged: Enhanced is sharpest", &["Sharp pixels", "Smooth", "Enhanced"], fi) {
            s.filter = Filter::ALL[k];
            changed = true;
        }
        y += ROW_STEP;
        if let Some(k) = self.choice_row(y, "Debug HUD", "Frame / tick overlay at start (F1 toggles)", &["Off", "On"], s.hud as usize) {
            s.hud = k == 1;
            changed = true;
        }
        y += ROW_STEP;
        if let Some(k) = self.choice_row(y, "Controller", "Play with a gamepad (mapped to keys + mouse)", &["Off", "On"], s.pad as usize) {
            s.pad = k == 1;
            changed = true;
        }
        y += ROW_STEP;
        if let Some(k) = self.choice_row(y, "Stick moves with", "Which keys the left stick / D-pad press", &["Arrow keys", "W A S D"], s.pad_wasd as usize) {
            s.pad_wasd = k == 1;
            changed = true;
        }
        y += ROW_STEP;
        if let Some(k) = self.choice_row(y, "Cursor speed", "Right stick mouse cursor (RT / RB click)", &["Slow", "Normal", "Fast"], (s.pad_speed.clamp(1, 3) - 1) as usize) {
            s.pad_speed = k as u8 + 1;
            changed = true;
        }
        if changed {
            s.save();
        }
        let done = Rect::new(sw - 36.0 - 120.0, top + 20.0, 120.0, 36.0);
        let close = self.button(done, "Done", true, true) || is_key_pressed(KeyCode::Escape) || self.pad_back;
        self.settings_panel = if close { None } else { Some((i, s)) };
    }

    /// Find Fusion games in Steam and add them (linked in place, or copied to storage).
    fn scan_steam(&mut self) {
        self.steam_lines.clear();
        self.busy = Some("Looking for Clickteam games in Steam...".into());
        let tx = self.tx.clone();
        let copy = self.config.steam_copy;
        let storage = self.config.storage.clone();
        let known: Vec<String> = self.entries.iter().map(|e| e.path.to_string_lossy().to_lowercase()).collect();
        std::thread::spawn(move || {
            if steam::steam_roots().is_empty() {
                let _ = tx.send(Msg::Done("Steam isn't installed (or its folder wasn't found).".into()));
                return;
            }
            let games = steam::find_fusion_games(|name| {
                let _ = tx.send(Msg::Status(format!("Checking {name}...")));
            });
            let (mut added, mut skipped) = (0, 0);
            for g in &games {
                let folder = if copy {
                    let _ = tx.send(Msg::Status(format!("Copying {}...", g.name)));
                    match steam::copy_into(g, &storage) {
                        Ok(d) => d,
                        Err(e) => {
                            let _ = tx.send(Msg::SteamLine(format!("{}: copy failed ({e})", g.name)));
                            continue;
                        }
                    }
                } else {
                    g.folder.clone()
                };
                for f in &g.files {
                    let path = folder.join(f.strip_prefix(&g.folder).unwrap_or(f));
                    if known.contains(&path.to_string_lossy().to_lowercase()) {
                        skipped += 1;
                        let _ = tx.send(Msg::SteamLine(format!("{}: already in the library", g.name)));
                        continue;
                    }
                    if let Some(e) = library::inspect(&path) {
                        added += 1;
                        let how = if copy { "copied and added" } else { "added" };
                        let _ = tx.send(Msg::SteamLine(format!("{} ({}): {how}", e.title, g.name)));
                        let _ = tx.send(Msg::Found(e));
                    }
                }
            }
            let _ = tx.send(Msg::Done(match (games.len(), added) {
                (0, _) => "No Clickteam games found in your Steam libraries.".into(),
                (_, 0) => format!("Found {} Clickteam game(s); all were already in the library ({skipped}).", games.len()),
                (_, n) => format!("Added {n} game(s) from Steam."),
            }));
        });
    }

    /// Add a library entry to Steam in the background (restarting Steam if it's running).
    fn add_to_steam(&mut self, i: usize) {
        let Some(e) = self.entries.get(i).cloned() else { return };
        self.busy = Some(format!("Adding {} to Steam...", e.title));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let progress = |s: &str| {
                let _ = tx.send(Msg::Status(s.to_string()));
            };
            let msg = match steam_shortcuts::add_game(&e.path, &e.title, true, progress) {
                Ok(m) => m,
                Err(err) => format!("Could not add {} to Steam: {err}", e.title),
            };
            let _ = tx.send(Msg::Done(msg));
        });
    }

    fn draw_steam_confirm(&mut self, top: f32, i: usize) {
        let sw = screen_width();
        let title = self.entries.get(i).map(|e| e.title.clone()).unwrap_or_default();
        draw_rectangle(16.0, top + 4.0, sw - 32.0, screen_height() - top - STATUS_H - 8.0, CARD);
        self.bold_text(&format!("Add {title} to Steam"), 36.0, top + 40.0, 24, TEXT);
        let lines = [
            "Steam is running. It only reads its shortcuts when it starts, and can overwrite",
            "changes made while it's open, so Rekindle will close Steam, add the shortcut",
            "(with the game's Steam artwork if it has any) and start Steam again.",
        ];
        for (k, l) in lines.iter().enumerate() {
            self.text(l, 36.0, top + 76.0 + k as f32 * 24.0, 17, MUTED);
        }
        let y = top + 76.0 + lines.len() as f32 * 24.0 + 20.0;
        let go = Rect::new(36.0, y, 260.0, 38.0);
        let cancel = Rect::new(go.x + go.w + 12.0, y, 120.0, 38.0);
        if self.button(go, "Restart Steam and add", true, true) || self.pad_a || is_key_pressed(KeyCode::Enter) {
            self.steam_confirm = None;
            self.add_to_steam(i);
        } else if self.button(cancel, "Cancel", false, true) || is_key_pressed(KeyCode::Escape) || self.pad_back {
            self.steam_confirm = None;
        }
    }

    fn draw_steam_panel(&mut self, top: f32) {
        let sw = screen_width();
        draw_rectangle(16.0, top + 4.0, sw - 32.0, screen_height() - top - STATUS_H - 8.0, CARD);
        self.bold_text("Steam games", 36.0, top + 40.0, 24, TEXT);
        self.text("Finds Clickteam Fusion games installed through Steam and adds them to the library.", 36.0, top + 64.0, 14, MUTED);
        let idle = self.busy.is_none();
        let mut y = top + 80.0;
        if let Some(k) = self.choice_row(y, "Add games by", "Linking uses Steam's copy; copying keeps your own", &["Linking in place", "Copying to storage"], self.config.steam_copy as usize) {
            self.config.steam_copy = k == 1;
            self.config.save();
        }
        y += ROW_STEP;
        self.text("Storage folder", 48.0, y + 23.0, 19, TEXT);
        self.text("Where copied games go", 48.0, y + 46.0, 14, MUTED);
        let shown = self.fit(&self.config.storage.display().to_string(), 15, sw - 400.0 - 160.0);
        self.text(&shown, 400.0, y + 28.0, 15, if self.config.steam_copy { TEXT } else { MUTED });
        if self.button(Rect::new(sw - 36.0 - 120.0, y + 6.0, 120.0, 32.0), "Choose...", false, idle) {
            if let Some(dir) = dialogs::pick_folder() {
                self.config.storage = dir;
                self.config.save();
            }
        }
        y += ROW_STEP + 8.0;
        if self.button(Rect::new(48.0, y, 300.0, 38.0), "Find Clickteam games in Steam", true, idle) {
            self.scan_steam();
        }
        y += 56.0;
        for (k, line) in self.steam_lines.iter().enumerate() {
            let ly = y + k as f32 * 22.0;
            if ly > screen_height() - STATUS_H - 20.0 {
                break;
            }
            self.text(line, 48.0, ly, 16, TEXT);
        }
        let done = Rect::new(sw - 36.0 - 120.0, top + 20.0, 120.0, 36.0);
        if self.button(done, "Done", true, true) || is_key_pressed(KeyCode::Escape) || self.pad_back {
            self.steam_panel = false;
        }
    }

    /// On start: make sure Steam has a shortcut to Rekindle. Added quietly when Steam is
    /// closed; when it's open (adding means restarting it) the status bar asks first.
    fn check_steam_shortcut(&self) {
        if !self.has_steam || self.config.steam_self_never || update::is_dev_build() || steam_shortcuts::in_gaming_mode() {
            return;
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            if steam_shortcuts::has_launcher_shortcut() || steam_shortcuts::user_config_dir().is_none() {
                return;
            }
            if steam_shortcuts::steam_running() {
                let _ = tx.send(Msg::OfferSteamShortcut);
            } else if let Ok(msg) = steam_shortcuts::add_launcher(false, |_| {}) {
                let _ = tx.send(Msg::Done(msg));
            }
        });
    }

    fn draw_steam_offer(&mut self, sh: f32) {
        let y = sh - STATUS_H + 2.0;
        let msg = "Add Rekindle to Steam? (Steam restarts)";
        self.text(msg, 20.0, sh - 10.0, 16, TEXT);
        let mut x = 20.0 + self.text_width(msg, 16) + 14.0;
        let mut clicked = None;
        for (k, (label, accent)) in [("Add", true), ("Not now", false), ("Don't ask again", false)].into_iter().enumerate() {
            let w = self.text_width(label, 16) + 26.0;
            if self.button(Rect::new(x, y, w, STATUS_H - 4.0), label, accent, true) {
                clicked = Some(k);
            }
            x += w + 8.0;
        }
        match clicked {
            Some(0) => {
                self.steam_offer = false;
                self.busy = Some("Adding Rekindle to Steam...".into());
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let progress = |s: &str| {
                        let _ = tx.send(Msg::Status(s.to_string()));
                    };
                    let msg = steam_shortcuts::add_launcher(true, progress).unwrap_or_else(|e| format!("Could not add Rekindle to Steam: {e}"));
                    let _ = tx.send(Msg::Done(msg));
                });
            }
            Some(1) => self.steam_offer = false,
            Some(_) => {
                self.steam_offer = false;
                self.config.steam_self_never = true;
                self.config.save();
            }
            None => {}
        }
    }

    fn draw_status(&mut self, sh: f32) {
        // Update offer at the right of the status bar.
        if let Some(r) = self.update.clone() {
            let label = if self.updating { format!("Updating to v{}...", r.version) } else { format!("Update to v{}", r.version) };
            let w = self.text_width(&label, 17) + 30.0;
            let rect = Rect::new(screen_width() - 16.0 - w, sh - STATUS_H + 2.0, w, STATUS_H - 4.0);
            if self.button(rect, &label, true, !self.updating) {
                self.updating = true;
                self.busy = Some(format!("Downloading Rekindle v{}...", r.version));
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::UpdateInstalled(update::install(&r)));
                });
            }
        }
        if self.steam_offer && self.busy.is_none() {
            self.draw_steam_offer(sh);
            return;
        }
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
    l.start_update_check();
    l.check_steam_shortcut();
    loop {
        l.update();
        l.refresh_log();
        l.draw();
        next_frame().await;
    }
}
