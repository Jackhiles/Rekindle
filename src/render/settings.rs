//! Per-game player settings, in `<data dir>\Rekindle\settings\<game file name>.ini`
//! (edited by the launcher, and by F9 / F10 / F11 in game).

use std::path::{Path, PathBuf};

/// How the finished frame is enlarged to the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// Hard pixel edges.
    Sharp,
    /// Bilinear.
    Smooth,
    /// Bicubic (Catmull-Rom) upscaling: smooth but crisper than bilinear.
    Enhanced,
}

/// How the game view is fitted into the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scaling {
    /// Largest size that keeps the shape (black bars fill the rest).
    Fit,
    /// Largest whole multiple of the game size (crisp pixels), centred.
    Integer,
    /// Fill the whole window, ignoring the shape.
    Stretch,
}

impl Filter {
    pub const ALL: [Filter; 3] = [Filter::Sharp, Filter::Smooth, Filter::Enhanced];
    pub fn name(self) -> &'static str {
        match self {
            Filter::Sharp => "sharp",
            Filter::Smooth => "smooth",
            Filter::Enhanced => "enhanced",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == s)
    }
}

impl Scaling {
    pub const ALL: [Scaling; 3] = [Scaling::Fit, Scaling::Integer, Scaling::Stretch];
    pub fn name(self) -> &'static str {
        match self {
            Scaling::Fit => "fit",
            Scaling::Integer => "integer",
            Scaling::Stretch => "stretch",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == s)
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    /// None = follow the game's own "fullscreen at start" setting.
    pub fullscreen: Option<bool>,
    pub filter: Filter,
    pub scaling: Scaling,
    /// Windowed size as a multiple of the game size; 0 = automatic (fits the desktop).
    pub window_scale: u8,
    /// Show the debug overlay (frame, tick, unsupported features) when the game starts.
    pub hud: bool,
    path: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { fullscreen: None, filter: Filter::Smooth, scaling: Scaling::Fit, window_scale: 0, hud: false, path: None }
    }
}

impl Settings {
    /// Settings file for a game file.
    pub fn path_for(game: &Path) -> PathBuf {
        let stem = game.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "game".into());
        let safe: String = stem.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect();
        crate::runtime::files::data_root().join("settings").join(format!("{}.ini", safe.trim()))
    }

    /// Load a game's settings. `legacy_dir` is the game's save folder, where older versions
    /// kept `rekindle.ini`; it is read if the new file doesn't exist yet.
    pub fn for_game(game: &Path, legacy_dir: Option<&Path>) -> Self {
        let path = Self::path_for(game);
        let mut s = Self { path: Some(path.clone()), ..Self::default() };
        let text = std::fs::read_to_string(&path)
            .ok()
            .or_else(|| legacy_dir.and_then(|d| std::fs::read_to_string(d.join("rekindle.ini")).ok()));
        if let Some(text) = text {
            s.apply(&text);
        }
        s
    }

    fn apply(&mut self, text: &str) {
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim().to_ascii_lowercase();
            let on = matches!(v.as_str(), "1" | "true" | "yes" | "on");
            match k.trim() {
                "fullscreen" => self.fullscreen = if v == "default" { None } else { Some(on) },
                // Older files: smooth=0/1.
                "smooth" => self.filter = if on { Filter::Smooth } else { Filter::Sharp },
                "filter" => self.filter = Filter::parse(&v).unwrap_or(self.filter),
                "scaling" => self.scaling = Scaling::parse(&v).unwrap_or(self.scaling),
                "window_scale" => self.window_scale = v.parse::<u8>().unwrap_or(0).min(4),
                "hud" => self.hud = on,
                _ => {}
            }
        }
    }

    pub fn to_text(&self) -> String {
        let mut text = String::from("[rekindle]\n");
        match self.fullscreen {
            Some(f) => text += &format!("fullscreen={}\n", f as u8),
            None => text += "fullscreen=default\n",
        }
        text += &format!("filter={}\n", self.filter.name());
        text += &format!("scaling={}\n", self.scaling.name());
        text += &format!("window_scale={}\n", self.window_scale);
        text += &format!("hud={}\n", self.hud as u8);
        text
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(path, self.to_text()) {
            eprintln!("could not save {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_legacy_keys() {
        let mut s = Settings::default();
        s.fullscreen = Some(true);
        s.filter = Filter::Enhanced;
        s.scaling = Scaling::Integer;
        s.window_scale = 2;
        s.hud = true;
        let mut t = Settings::default();
        t.apply(&s.to_text());
        assert_eq!((t.fullscreen, t.filter, t.scaling, t.window_scale, t.hud), (Some(true), Filter::Enhanced, Scaling::Integer, 2, true));
        let mut old = Settings::default();
        old.apply("[rekindle]\nfullscreen=0\nsmooth=0\n");
        assert_eq!((old.fullscreen, old.filter), (Some(false), Filter::Sharp));
    }
}
