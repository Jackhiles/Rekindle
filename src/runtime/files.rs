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
    /// Tests: never read the player's real AppData files (saves made outside the sandbox),
    /// only the sandbox and the game's own folder.
    pub isolated: bool,
    /// Original files already noted in ORIGINALS_LIST this session.
    noted: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

/// In a save folder: the game paths of original files the game has read (one per line),
/// so the save manager knows which original files belong to the game.
pub const ORIGINALS_LIST: &str = "original-files.txt";

fn norm(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Default save folder for a game title: %APPDATA%\Rekindle\<title> (or ~/.local/share/Rekindle/<title>).
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
    let dir = data_root().join(safe);
    // Files that had the old names inside a game's folder.
    for (old, new) in [("fusion-rt.ini", "rekindle.ini"), ("fusion-rt.log", "rekindle.log")] {
        let (o, n) = (dir.join(old), dir.join(new));
        if o.is_file() && !n.exists() {
            let _ = std::fs::rename(o, n);
        }
    }
    dir
}

/// The save folder of a game file whose header names it `name` (the file name if empty).
pub fn save_root_for(game: &Path, name: &str) -> PathBuf {
    if name.is_empty() {
        default_save_root(&game.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
    } else {
        default_save_root(name)
    }
}

/// `%APPDATA%\Rekindle` (or the platform's data folder): saves, settings and the launcher library.
pub fn data_root() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .unwrap_or_else(|| PathBuf::from("."));
    let root = base.join("Rekindle");
    migrate_old_saves(&base, &root);
    root
}

/// Saves made before the project was renamed live in `fusion-rt`; move them over once.
fn migrate_old_saves(base: &Path, root: &Path) {
    let old = base.join("fusion-rt");
    if old.is_dir() && !root.exists() {
        let _ = std::fs::rename(&old, root);
    }
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
            isolated: false,
            noted: Default::default(),
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
        if let Ok(b) = std::fs::read(self.sandboxed(path)) {
            return Some(b);
        }
        let b = self.real_allowed(path).then(|| std::fs::read(self.real(path)).ok()).flatten()?;
        if !self.isolated {
            self.note_original(path);
        }
        Some(b)
    }

    /// Read a game data file (a sound, an image) the same way, without counting it as a save.
    pub fn read_asset(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.sandboxed(path))
            .ok()
            .or_else(|| self.real_allowed(path).then(|| std::fs::read(self.real(path)).ok()).flatten())
    }

    /// Remember that the game read an original file (see ORIGINALS_LIST).
    fn note_original(&self, path: &str) {
        let key = norm(&self.real(path).to_string_lossy());
        let Ok(mut set) = self.noted.lock() else { return };
        if !set.insert(key) {
            return;
        }
        let list = self.save_root.join(ORIGINALS_LIST);
        let known = std::fs::read_to_string(&list).unwrap_or_default();
        let real = self.real(path).to_string_lossy().into_owned();
        if known.lines().any(|l| norm(l) == norm(&real)) {
            return;
        }
        let _ = std::fs::create_dir_all(&self.save_root);
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&list) {
            let _ = writeln!(f, "{real}");
        }
    }

    /// The original location of a file inside the save folder (`area/rest` as written by
    /// `sandboxed`), or None for locations that cannot be mapped back.
    pub fn original_of(&self, sandboxed: &Path) -> Option<PathBuf> {
        let rel = sandboxed.strip_prefix(&self.save_root).ok()?;
        let mut parts = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned());
        let area = parts.next()?;
        let rest: Vec<String> = parts.collect();
        if rest.is_empty() {
            return None;
        }
        let mut out = match area.as_str() {
            "app" => self.app_dir.clone(),
            "appdata" => self.appdata.clone()?,
            // `other/c/dir/file` came from `C:/dir/file`.
            "other" if cfg!(windows) => {
                let drive = rest.first()?;
                if drive.len() != 1 {
                    return None;
                }
                let mut p = PathBuf::from(format!("{}:\\", drive.to_uppercase()));
                for r in &rest[1..] {
                    p.push(r);
                }
                return Some(p);
            }
            _ => return None,
        };
        for r in &rest {
            out.push(r);
        }
        Some(out)
    }

    /// May the real location of `path` be read? (Not AppData when isolated.)
    fn real_allowed(&self, path: &str) -> bool {
        !self.isolated || self.appdata.as_ref().is_none_or(|a| !self.real(path).starts_with(a))
    }

    pub fn write(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        let target = self.sandboxed(path);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(target, data)
    }

    pub fn exists(&self, path: &str) -> bool {
        self.sandboxed(path).is_file() || (self.real_allowed(path) && self.real(path).is_file())
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
        // An absolute game path on this platform; games then build their paths from it.
        let (exe, save) = if cfg!(windows) {
            ("C:\\Games\\X\\game.exe", "C:\\Games\\X\\save1.txt")
        } else {
            ("/games/x/game.exe", "/games/x/save1.txt")
        };
        let fs = SaveFs::new(Path::new(exe), PathBuf::from("S"));
        let p = fs.sandboxed(save);
        assert_eq!(p, PathBuf::from("S").join("app").join("save1.txt"));
        assert_eq!(
            fs.sandboxed("saves/a.ini"),
            PathBuf::from("S").join("app").join("saves").join("a.ini")
        );
        assert!(!fs.sandboxed("..\\..\\evil").starts_with(".."));
    }
}
