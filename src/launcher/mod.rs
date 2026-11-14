//! The game launcher shown when Rekindle is started without a game: a library of game
//! files with Play, desktop-shortcut and remove buttons. Games run in their own process.

mod covers;
pub mod library;
pub mod steam;
pub mod steam_shortcuts;
mod art;
pub mod update;
mod pages;
mod view;
use view::Menu;

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

const BG: Color = Color::new(0.055, 0.055, 0.067, 1.0);
/// Panels and the details pane.
const CARD: Color = Color::new(0.095, 0.095, 0.115, 1.0);
const TEXT: Color = Color::new(0.94, 0.94, 0.96, 1.0);
const MUTED: Color = Color::new(0.56, 0.57, 0.63, 1.0);
const ACCENT: Color = Color::new(0.96, 0.52, 0.18, 1.0);
const BUTTON: Color = Color::new(0.16, 0.16, 0.195, 1.0);
/// Pop-up menus and toasts.
const MENU: Color = Color::new(0.14, 0.14, 0.17, 1.0);
const DANGER: Color = Color::new(0.94, 0.38, 0.35, 1.0);

const HEADER_H: f32 = 64.0;
/// Space kept free at the bottom of panels (toasts appear there).
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
    /// Cover / hero images decoded by the art worker.
    Art(covers::Art),
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
    /// Settings panel save row: cached status line, and a copy waiting for a second click.
    saves_status: Option<(usize, String)>,
    saves_confirm: Option<crate::runtime::save_sync::Direction>,
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
    /// Library artwork: (cover, hero) per game file, and games already asked for.
    art: HashMap<PathBuf, (Option<Texture2D>, Option<Texture2D>)>,
    art_requested: std::collections::HashSet<PathBuf>,
    art_tx: Sender<(PathBuf, String)>,
    /// Typed filter for the library.
    search: String,
    menu: Option<Menu>,
    /// A pop-up menu is open: other buttons ignore the mouse.
    block_input: bool,
    /// Columns in the cover grid (for up / down navigation).
    grid_cols: usize,
    /// Scroll the selected game into view.
    reveal: bool,
}

impl Launcher {
    fn new() -> Self {
        let (tx, rx) = channel();
        let art_out = tx.clone();
        let art_tx = covers::worker(move |a| {
            let _ = art_out.send(Msg::Art(a));
        });
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
            saves_status: None,
            saves_confirm: None,
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
            art: HashMap::new(),
            art_requested: Default::default(),
            art_tx,
            search: String::new(),
            menu: None,
            block_input: false,
            grid_cols: 1,
            reveal: false,
        }
    }

    /// Controller navigation: the D-pad / left stick move through the cover grid, A plays,
    /// Y opens settings, B closes a panel or menu. Returns the action for the selected game.
    fn pad_nav(&mut self) -> Option<(usize, u8)> {
        use gilrs::{Axis, Button};
        self.pad_back = false;
        self.pad_a = false;
        self.running.retain_mut(|(_, c)| matches!(c.try_wait(), Ok(None)));
        let game_running = !self.running.is_empty();
        let g = self.pad.as_mut()?;
        while g.next_event().is_some() {}
        let (_, p) = g.gamepads().find(|(_, p)| p.is_connected())?;
        let (lx, ly) = (p.value(Axis::LeftStickX), p.value(Axis::LeftStickY));
        let mut now = std::collections::HashSet::new();
        for b in [Button::DPadUp, Button::DPadDown, Button::DPadLeft, Button::DPadRight, Button::South, Button::East, Button::North] {
            if p.is_pressed(b) {
                now.insert(b);
            }
        }
        for (on, b) in [(ly > 0.6, Button::DPadUp), (ly < -0.6, Button::DPadDown), (lx < -0.6, Button::DPadLeft), (lx > 0.6, Button::DPadRight)] {
            if on {
                now.insert(b);
            }
        }
        if game_running {
            // Buttons pressed in the game are not for the launcher; also don't act on them
            // when the game closes while they're still held.
            self.pad_held = now;
            return None;
        }
        let pressed = |b: Button| now.contains(&b) && !self.pad_held.contains(&b);
        let moves = [(Button::DPadLeft, -1isize), (Button::DPadRight, 1), (Button::DPadUp, -(self.grid_cols.max(1) as isize)), (Button::DPadDown, self.grid_cols.max(1) as isize)];
        let delta: Option<isize> = moves.iter().find(|(b, _)| pressed(*b)).map(|(_, d)| *d);
        let (a, b, y) = (pressed(Button::South), pressed(Button::East), pressed(Button::North));
        self.pad_held = now;
        self.pad_back = b;
        self.pad_a = a;
        if self.entries.is_empty() || !self.library_view() || self.menu.is_some() {
            if b && self.show_log {
                self.show_log = false;
            }
            return None;
        }
        if let Some(d) = delta {
            self.move_selection(d);
        }
        match (self.selected, a, y) {
            (Some(i), true, _) => Some((i, view::ACT_PLAY)),
            (Some(i), _, true) => Some((i, view::ACT_SETTINGS)),
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
        let hover = enabled && !self.block_input && r.contains(Vec2::from(mouse_position()));
        let base = if accent { ACCENT } else { BUTTON };
        let color = if !enabled {
            view::mix(BG, base, 0.45)
        } else if hover {
            view::mix(base, Color::new(1.0, 1.0, 1.0, 1.0), 0.1)
        } else {
            base
        };
        view::rrect(r.x, r.y, r.w, r.h, 8.0, color);
        let tw = self.bold_width(label, 16);
        let fg = if !enabled { MUTED } else if accent { Color::new(0.1, 0.06, 0.02, 1.0) } else { TEXT };
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
                Msg::Art(a) => {
                    let tex = |img: Option<covers::Rgba>| {
                        img.map(|i| {
                            let t = Texture2D::from_rgba8(i.w, i.h, &i.px);
                            t.set_filter(FilterMode::Linear);
                            t
                        })
                    };
                    self.art.insert(a.game, (tex(a.cover), tex(a.hero)));
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
        self.block_input = self.menu.is_some();
        // Read the controller every frame (panels use B / A from it too).
        let pad_action = self.pad_nav();
        let top = HEADER_H;
        if let Some(i) = self.steam_confirm {
            self.draw_steam_confirm(top, i);
        } else if self.steam_panel {
            self.draw_steam_panel(top);
        } else if self.settings_panel.is_some() {
            self.draw_settings_panel(top, sh - STATUS_H);
        } else if self.show_log {
            self.draw_log(top, sh - STATUS_H);
        } else {
            self.draw_library(sw, sh, pad_action);
        }
        if !self.library_view() {
            while get_char_pressed().is_some() {}
        }
        // The header last, so scrolled content slides under it.
        self.draw_header(sw);
        self.draw_overlays(sw, sh);
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
}

pub async fn run() {
    let mut l = Launcher::new();
    l.start_update_check();
    l.check_steam_shortcut();
    // Testing: REKINDLE_LAUNCHER_SHOT=<file.png> saves the window once artwork has loaded
    // (or after 20 s), then exits. REKINDLE_LAUNCHER_SELECT=<n> selects the n-th game,
    // REKINDLE_LAUNCHER_PAGE=settings|steam|log opens that page.
    let shot = std::env::var_os("REKINDLE_LAUNCHER_SHOT").map(PathBuf::from);
    if let Some(n) = std::env::var("REKINDLE_LAUNCHER_SELECT").ok().and_then(|v| v.parse::<usize>().ok()) {
        l.selected = Some(n);
    }
    match std::env::var("REKINDLE_LAUNCHER_PAGE").as_deref() {
        Ok("settings") if !l.entries.is_empty() => l.game_action(l.selected.unwrap_or(0).min(l.entries.len() - 1), view::ACT_SETTINGS),
        Ok("steam") => l.steam_panel = true,
        Ok("log") => l.show_log = true,
        _ => {}
    }
    let started = get_time();
    loop {
        l.update();
        l.refresh_log();
        l.draw();
        if let Some(p) = &shot {
            let settled = !l.art_requested.is_empty() && l.art_requested.iter().all(|g| l.art.contains_key(g));
            if (settled && get_time() - started > 2.0) || get_time() - started > 20.0 {
                get_screen_data().export_png(&p.to_string_lossy());
                return;
            }
        }
        next_frame().await;
    }
}
