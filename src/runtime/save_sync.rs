//! Save manager: move a game's saves between Rekindle's save folder and the places the
//! original game keeps them, in either direction.
//!
//! The save folder mirrors the original locations (`app\…` = the game's folder,
//! `appdata\…` = AppData, `other\c\…` = `C:\…`; see `SaveFs::sandboxed`), so every file in
//! it has an original counterpart. Original files the game has read but Rekindle has not
//! written yet are listed in `ORIGINALS_LIST`. For each of those files, a copy makes the
//! destination match the source: files are copied, and destination files with no source
//! are removed. Everything overwritten or removed goes to `backups\<time>\` in the save
//! folder first, so both directions can be undone by hand.

use super::files::{SaveFs, ORIGINALS_LIST};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One save file and its two copies.
#[derive(Debug, Clone)]
pub struct SaveFile {
    /// Inside Rekindle's save folder.
    pub rekindle: PathBuf,
    /// Where the original game keeps it.
    pub original: PathBuf,
    pub rekindle_time: Option<SystemTime>,
    pub original_time: Option<SystemTime>,
}

/// Which way to copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Original game -> Rekindle.
    Import,
    /// Rekindle -> original game.
    Export,
}

/// Summary for the launcher.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub files: usize,
    pub in_rekindle: usize,
    pub in_original: usize,
    /// Newest change on each side.
    pub rekindle_newest: Option<SystemTime>,
    pub original_newest: Option<SystemTime>,
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok().filter(|m| m.is_file()).and_then(|m| m.modified().ok())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// The game's save files: everything in the save folder's mapped areas plus the original
/// files the game has read.
pub fn list(fs: &SaveFs) -> Vec<SaveFile> {
    let mut found: Vec<PathBuf> = Vec::new();
    for area in ["app", "appdata", "other"] {
        walk(&fs.save_root.join(area), &mut found);
    }
    if let Ok(text) = std::fs::read_to_string(fs.save_root.join(ORIGINALS_LIST)) {
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            found.push(fs.sandboxed(line));
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut files = Vec::new();
    for rekindle in found {
        let key = rekindle.to_string_lossy().to_lowercase();
        if !seen.insert(key) {
            continue;
        }
        let Some(original) = fs.original_of(&rekindle) else { continue };
        files.push(SaveFile {
            rekindle_time: modified(&rekindle),
            original_time: modified(&original),
            rekindle,
            original,
        });
    }
    files.sort_by(|a, b| a.rekindle.cmp(&b.rekindle));
    files
}

/// "3 files, newest 2 hours ago" style age of a time.
pub fn age(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return "none".into() };
    let s = SystemTime::now().duration_since(t).map_or(0, |d| d.as_secs());
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

impl Status {
    /// One line for the launcher.
    pub fn line(&self) -> String {
        if self.files == 0 {
            return "No saves yet (the game hasn't saved or loaded anything here).".into();
        }
        let newer = match (self.rekindle_newest, self.original_newest) {
            (Some(a), Some(b)) if a > b => " - Rekindle's are newer",
            (Some(a), Some(b)) if b > a => " - the original game's are newer",
            _ => "",
        };
        format!(
            "Rekindle: {} file(s), changed {}.  Original game: {} file(s), changed {}{newer}.",
            self.in_rekindle,
            age(self.rekindle_newest),
            self.in_original,
            age(self.original_newest)
        )
    }
}

pub fn status(fs: &SaveFs) -> Status {
    let files = list(fs);
    Status {
        files: files.len(),
        in_rekindle: files.iter().filter(|f| f.rekindle_time.is_some()).count(),
        in_original: files.iter().filter(|f| f.original_time.is_some()).count(),
        rekindle_newest: files.iter().filter_map(|f| f.rekindle_time).max(),
        original_newest: files.iter().filter_map(|f| f.original_time).max(),
    }
}

/// What a copy did.
#[derive(Debug, Default)]
pub struct Report {
    pub copied: usize,
    pub removed: usize,
    /// Folder holding what was overwritten or removed (None if nothing was).
    pub backup: Option<PathBuf>,
    pub errors: Vec<String>,
}

/// Copy the saves one way. Destination files are backed up before being replaced.
pub fn copy(fs: &SaveFs, dir: Direction) -> Report {
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string();
    let backup_root = fs.save_root.join("backups").join(&stamp);
    let mut report = Report::default();
    for f in list(fs) {
        let (src, dst) = match dir {
            Direction::Import => (&f.original, &f.rekindle),
            Direction::Export => (&f.rekindle, &f.original),
        };
        let src_exists = src.is_file();
        if !src_exists && !dst.is_file() {
            continue;
        }
        if src_exists && dst.is_file() && std::fs::read(src).ok() == std::fs::read(dst).ok() {
            continue;
        }
        // Back up what is about to change, under the area path it has in the save folder.
        if dst.is_file() {
            let side = match dir {
                Direction::Import => "rekindle",
                Direction::Export => "original",
            };
            let rel = f.rekindle.strip_prefix(&fs.save_root).unwrap_or(&f.rekindle);
            let b = backup_root.join(side).join(rel);
            let done = b.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::copy(dst, &b).map(|_| ()));
            if let Err(e) = done {
                report.errors.push(format!("backup of {}: {e}", dst.display()));
                continue;
            }
            report.backup = Some(backup_root.clone());
        }
        let result = if src_exists {
            dst.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::copy(src, dst).map(|_| ()))
        } else {
            std::fs::remove_file(dst)
        };
        match result {
            Ok(()) if src_exists => report.copied += 1,
            Ok(()) => report.removed += 1,
            Err(e) => report.errors.push(format!("{}: {e}", dst.display())),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rekindle-savesync-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn import_and_export_round_trip_with_backups() {
        let root = tmp("rt");
        let game_dir = root.join("game");
        std::fs::create_dir_all(&game_dir).unwrap();
        let exe = game_dir.join("game.exe");
        std::fs::write(&exe, b"").unwrap();
        let fs = SaveFs::new(&exe, root.join("saves"));

        // The original game has a save; the game reads it (falls back to the original).
        std::fs::write(game_dir.join("save1.txt"), b"original").unwrap();
        assert_eq!(fs.read("save1.txt").as_deref(), Some(&b"original"[..]));
        let files = list(&fs);
        assert_eq!(files.len(), 1);
        assert!(files[0].rekindle_time.is_none() && files[0].original_time.is_some());

        // Import copies it in; Rekindle then saves its own progress.
        let r = copy(&fs, Direction::Import);
        assert_eq!((r.copied, r.removed), (1, 0));
        fs.write("save1.txt", b"rekindle").unwrap();
        fs.write("save2.txt", b"new file").unwrap();

        // Export: the original is backed up, then replaced; the new file is added.
        let r = copy(&fs, Direction::Export);
        assert_eq!((r.copied, r.removed), (2, 0), "{:?}", r.errors);
        assert_eq!(std::fs::read(game_dir.join("save1.txt")).unwrap(), b"rekindle");
        assert_eq!(std::fs::read(game_dir.join("save2.txt")).unwrap(), b"new file");
        let backup = r.backup.unwrap();
        assert_eq!(std::fs::read(backup.join("original").join("app").join("save1.txt")).unwrap(), b"original");

        // Import when the original lost a file: Rekindle's copy is backed up and removed.
        std::fs::remove_file(game_dir.join("save2.txt")).unwrap();
        let r = copy(&fs, Direction::Import);
        assert_eq!((r.copied, r.removed), (0, 1));
        assert!(!fs.sandboxed("save2.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
