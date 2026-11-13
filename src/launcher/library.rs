//! The launcher's game library: a plain list of game files with cached titles and icons,
//! kept in `<data dir>\Rekindle\library.txt` (one `path<TAB>title` per line) and
//! `<data dir>\Rekindle\icons\`.

use crate::ctf_loader::GameData;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub title: String,
}

/// `%APPDATA%\Rekindle` (or the platform's data folder).
pub fn data_dir() -> PathBuf {
    crate::runtime::files::data_root()
}

fn library_file() -> PathBuf {
    data_dir().join("library.txt")
}

/// Cached icon (PNG) for a game file.
pub fn icon_path(game: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    game.to_string_lossy().to_lowercase().hash(&mut h);
    data_dir().join("icons").join(format!("{:016x}.png", h.finish()))
}

pub fn load() -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(library_file()) else { return Vec::new() };
    text.lines()
        .filter_map(|l| {
            let (p, t) = l.split_once('\t')?;
            Some(Entry { path: PathBuf::from(p), title: t.to_string() })
        })
        .collect()
}

/// Add `e` unless its file is already listed; a title that's already used gets the file
/// name appended (two games can share an internal title). Returns false for duplicates.
pub fn add(entries: &mut Vec<Entry>, mut e: Entry) -> bool {
    let key = e.path.to_string_lossy().to_lowercase();
    if entries.iter().any(|x| x.path.to_string_lossy().to_lowercase() == key) {
        return false;
    }
    if entries.iter().any(|x| x.title.eq_ignore_ascii_case(&e.title)) {
        let stem = e.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        e.title = format!("{} ({stem})", e.title);
    }
    entries.push(e);
    entries.sort_by_key(|e| e.title.to_lowercase());
    true
}

pub fn save(entries: &[Entry]) {
    let text: String = entries
        .iter()
        .map(|e| format!("{}\t{}\n", e.path.display(), e.title.replace(['\t', '\n'], " ")))
        .collect();
    let _ = std::fs::create_dir_all(data_dir());
    if let Err(e) = std::fs::write(library_file(), text) {
        eprintln!("could not save the game library: {e}");
    }
}

/// Read a game's title and cache its icon. `None` if the file isn't a Fusion game.
pub fn inspect(path: &Path) -> Option<Entry> {
    let game = GameData::load(path).ok()?;
    let title = if game.name.trim().is_empty() {
        path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    } else {
        game.name.trim().to_string()
    };
    if let Some(icon) = &game.icon {
        let out = icon_path(path);
        if let Some(dir) = out.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = write_png(&out, 64, &icon.resized(64));
    }
    // Remember the file the user picked (an .exe stays an .exe even if its data is a .dat).
    Some(Entry { path: path.to_path_buf(), title })
}

pub fn write_png(path: &Path, size: u32, rgba: &[u8]) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, size, size);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(std::io::Error::other)?;
    w.write_image_data(rgba).map_err(std::io::Error::other)
}

/// Fusion game files under `dir` (a few levels deep). Data files that belong to an
/// `.exe` next to them are skipped in favour of the `.exe`.
pub fn find_games(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                if depth < 4 {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let skip_dat = ext == "dat" && p.with_extension("exe").is_file();
            if (ext == "exe" || ext == "ccn" || ext == "dat") && !skip_dat {
                out.push(p);
            }
        }
    }
    out
}

/// Create a desktop shortcut that starts `game` with Rekindle, using the game's own icon.
pub fn create_shortcut(game: &Path, title: &str) -> Result<PathBuf, String> {
    let exe = super::update::program_path().map_err(|e| e.to_string())?;
    let game = std::path::absolute(game).unwrap_or_else(|_| game.to_path_buf());
    let safe: String = title.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect();
    // A ".dat" game's icon lives in the .exe next to it.
    let icon_src = if game.extension().is_some_and(|e| e.eq_ignore_ascii_case("dat")) && game.with_extension("exe").is_file() {
        game.with_extension("exe")
    } else {
        game.clone()
    };
    shortcut_impl(&exe, &game, &icon_src, safe.trim())
}

#[cfg(windows)]
fn shortcut_impl(exe: &Path, game: &Path, icon: &Path, name: &str) -> Result<PathBuf, String> {
    // Values go through environment variables, so names with quotes or apostrophes are safe.
    const SCRIPT: &str = "$d=[Environment]::GetFolderPath('Desktop'); \
        $p=Join-Path $d ($env:RK_NAME + '.lnk'); \
        $s=(New-Object -ComObject WScript.Shell).CreateShortcut($p); \
        $s.TargetPath=$env:RK_TARGET; $s.Arguments=('\"' + $env:RK_GAME + '\"'); \
        $s.WorkingDirectory=$env:RK_DIR; $s.IconLocation=($env:RK_ICON + ',0'); \
        $s.Description='Play with Rekindle'; $s.Save(); Write-Output $p";
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("RK_NAME", name)
        .env("RK_TARGET", exe)
        .env("RK_GAME", game)
        .env("RK_DIR", game.parent().unwrap_or(Path::new(".")))
        .env("RK_ICON", icon)
        .output()
        .map_err(|e| format!("could not run PowerShell: {e}"))?;
    if out.status.success() {
        Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(not(windows))]
fn shortcut_impl(exe: &Path, game: &Path, _icon: &Path, name: &str) -> Result<PathBuf, String> {
    // A freedesktop launcher entry on the desktop.
    let home = std::env::var_os("HOME").ok_or("no home folder")?;
    let path = PathBuf::from(home).join("Desktop").join(format!("{name}.desktop"));
    let text = format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec=\"{}\" \"{}\"\nPath={}\n",
        exe.display(),
        game.display(),
        game.parent().unwrap_or(Path::new(".")).display()
    );
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}
