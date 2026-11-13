//! Per-game player settings (`rekindle.ini` in the game's save folder).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Settings {
    /// None = follow the game's own "fullscreen at start" setting.
    pub fullscreen: Option<bool>,
    /// Upscale the finished frame with filtering instead of sharp pixels.
    pub smooth: bool,
    path: PathBuf,
}

impl Settings {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("rekindle.ini");
        let mut s = Self { fullscreen: None, smooth: true, path };
        if let Ok(text) = std::fs::read_to_string(&s.path) {
            for line in text.lines() {
                let Some((k, v)) = line.split_once('=') else { continue };
                let on = matches!(v.trim(), "1" | "true" | "yes" | "on");
                match k.trim() {
                    "fullscreen" => s.fullscreen = Some(on),
                    "smooth" => s.smooth = on,
                    _ => {}
                }
            }
        }
        s
    }

    pub fn save(&self) {
        let mut text = String::from("[rekindle]\n");
        if let Some(f) = self.fullscreen {
            text += &format!("fullscreen={}\n", f as u8);
        }
        text += &format!("smooth={}\n", self.smooth as u8);
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&self.path, text) {
            eprintln!("could not save {}: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("rekindle-settings-{}", std::process::id()));
        let mut s = Settings::load(&dir);
        assert_eq!(s.fullscreen, None);
        assert!(s.smooth);
        s.fullscreen = Some(true);
        s.smooth = false;
        s.save();
        let t = Settings::load(&dir);
        assert_eq!(t.fullscreen, Some(true));
        assert!(!t.smooth);
        let _ = std::fs::remove_dir_all(dir);
    }
}
