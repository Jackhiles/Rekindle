//! Sandboxed file access for games (save files, INI files, ...).
//!
//! Games build real Windows paths ("<exe dir>\save1.txt", "%APPDATA%\MMFApplications\game").
//! Writes are redirected into a per-game save folder so the runtime never touches the
//! original game's files; reads look in that folder first and fall back to the real
//! path, so existing saves from the original game are picked up (read-only).

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SaveFs {
    /// The game executable / data file.
    pub app_file: PathBuf,
    /// Directory containing it.
    pub app_dir: PathBuf,
    /// Where writes go.
    pub save_root: PathBuf,
    /// The real roaming AppData folder (Windows), used to map AppData paths.
    pub appdata: Option<PathBuf>,
}

fn norm(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Default save folder for a game title: %APPDATA%\fusion-rt\<title> (or ~/.local/share/fusion-rt/<title>).
pub fn default_save_root(title: &str) -> PathBuf {
    let safe: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.'".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.trim().is_empty() {
        "game".to_string()
    } else {
        safe.trim().to_string()
    };
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("fusion-rt").join(safe)
}

impl SaveFs {
    pub fn new(app_file: &Path, save_root: PathBuf) -> Self {
        let app_file = std::path::absolute(app_file).unwrap_or_else(|_| app_file.to_path_buf());
        let app_dir = app_file.parent().map(Path::to_path_buf).unwrap_or_default();
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
        Self {
            app_file,
            app_dir,
            save_root,
            appdata,
        }
    }

    /// "C:" style drive of the application (empty on systems without drives).
    pub fn app_drive(&self) -> String {
        match self.app_dir.components().next() {
            Some(Component::Prefix(p)) => p.as_os_str().to_string_lossy().into_owned(),
            _ => String::new(),
        }
    }

    /// Application directory without the drive, with a trailing separator.
    pub fn app_directory(&self) -> String {
        let full = self.app_path();
        full[self.app_drive().len()..].to_string()
    }

    /// Application directory with drive and trailing separator.
    pub fn app_path(&self) -> String {
        let mut s = self.app_dir.to_string_lossy().into_owned();
        if !s.ends_with(['\\', '/']) {
            s.push(std::path::MAIN_SEPARATOR);
        }
        s
    }

    /// Path inside the save folder that stands in for a game path.
    pub fn sandboxed(&self, path: &str) -> PathBuf {
        let n = norm(path);
        let app = norm(&self.app_dir.to_string_lossy());
        let rel_under = |base: &str| {
            n.strip_prefix(base)
                .map(|r| r.trim_start_matches('\\').to_string())
        };
        let (area, rel) = if let Some(r) = rel_under(&app) {
            ("app", r)
        } else if let Some(r) = self
            .appdata
            .as_ref()
            .and_then(|a| rel_under(&norm(&a.to_string_lossy())))
        {
            ("appdata", r)
        } else if !n.contains(':') && !n.starts_with('\\') {
            ("app", n.clone())
        } else {
            ("other", n.replace(':', ""))
        };
        let mut out = self.save_root.join(area);
        for part in rel
            .split('\\')
            .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        {
            out.push(part);
        }
        out
    }

    /// The real location a game path refers to (for reading original saves).
    fn real(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path.replace('\\', std::path::MAIN_SEPARATOR_STR));
        if p.is_absolute() {
            p
        } else {
            self.app_dir.join(p)
        }
    }

    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.sandboxed(path))
            .ok()
            .or_else(|| std::fs::read(self.real(path)).ok())
    }

    pub fn write(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        let target = self.sandboxed(path);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(target, data)
    }

    pub fn exists(&self, path: &str) -> bool {
        self.sandboxed(path).is_file() || self.real(path).is_file()
    }

    /// Deletes only the sandboxed copy; original game files are never touched.
    pub fn delete(&self, path: &str) {
        let _ = std::fs::remove_file(self.sandboxed(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_relative_paths_map_into_the_sandbox() {
        let fs = SaveFs::new(Path::new("C:\\Games\\X\\game.exe"), PathBuf::from("S"));
        let p = fs.sandboxed("C:\\Games\\X\\save1.txt");
        assert_eq!(p, PathBuf::from("S").join("app").join("save1.txt"));
        assert_eq!(
            fs.sandboxed("saves/a.ini"),
            PathBuf::from("S").join("app").join("saves").join("a.ini")
        );
        assert!(!fs.sandboxed("..\\..\\evil").starts_with(".."));
    }
}
