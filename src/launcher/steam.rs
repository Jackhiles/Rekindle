//! Find Clickteam Fusion games installed through Steam.
//!
//! Steam keeps a list of library folders in `steamapps/libraryfolders.vdf`; each library's
//! `steamapps/appmanifest_<id>.acf` names an installed game and its folder under
//! `steamapps/common/`. Game files there are probed with [`looks_like_fusion`].

use crate::ctf_loader::exe::looks_like_fusion;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SteamGame {
    pub app_id: String,
    pub name: String,
    pub folder: PathBuf,
    /// The Fusion game files inside it (usually one .exe).
    pub files: Vec<PathBuf>,
}

/// Path comparison key (Windows paths are case-insensitive).
fn key(p: &Path) -> String {
    let s = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().into_owned();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// Steam installations on this machine.
pub fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(windows)]
    {
        // Where Steam itself says it is installed.
        if let Ok(out) = std::process::Command::new("reg")
            .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = text.lines().find(|l| l.contains("SteamPath")) {
                if let Some(i) = line.find("REG_SZ") {
                    roots.push(PathBuf::from(line[i + 6..].trim().replace('/', "\\")));
                }
            }
        }
        for p in [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"] {
            roots.push(PathBuf::from(p));
        }
    }
    #[cfg(not(windows))]
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".steam/steam"));
        roots.push(home.join(".local/share/Steam"));
        roots.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
    }
    let mut seen = Vec::new();
    roots
        .into_iter()
        .filter(|r| r.join("steamapps").is_dir())
        .filter(|r| {
            let k = key(r);
            let new = !seen.contains(&k);
            seen.push(k);
            new
        })
        .collect()
}

/// Quoted `"key" "value"` pairs from a Steam text file (.vdf / .acf), in order.
fn pairs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split('"').collect();
        // `  "key"  "value"` splits into ["  ", key, "  ", value, ""].
        if parts.len() >= 5 {
            out.push((parts[1].to_ascii_lowercase(), parts[3].replace("\\\\", "\\")));
        }
    }
    out
}

/// All library folders of a Steam installation (including the installation itself).
pub fn libraries(root: &Path) -> Vec<PathBuf> {
    // Steam's own list first: its paths have the proper capitalisation.
    let mut libs: Vec<PathBuf> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(root.join("steamapps").join("libraryfolders.vdf")) {
        for (k, v) in pairs(&text) {
            let p = PathBuf::from(v);
            if k == "path" && p.join("steamapps").is_dir() && !libs.iter().any(|l| key(l) == key(&p)) {
                libs.push(p);
            }
        }
    }
    if !libs.iter().any(|l| key(l) == key(root)) {
        libs.insert(0, root.to_path_buf());
    }
    libs
}

/// Installed games in one library: (app id, name, folder).
pub fn installed(library: &Path) -> Vec<(String, String, PathBuf)> {
    let apps = library.join("steamapps");
    let mut out = Vec::new();
    for e in std::fs::read_dir(&apps).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let kv = pairs(&text);
        let get = |k: &str| kv.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        if let (Some(id), Some(title), Some(dir)) = (get("appid"), get("name"), get("installdir")) {
            let folder = apps.join("common").join(dir);
            if folder.is_dir() {
                out.push((id, title, folder));
            }
        }
    }
    out.sort_by_key(|g| g.1.to_lowercase());
    out
}

/// Fusion game files in a game folder (a couple of levels deep).
pub fn fusion_files(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(folder.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth < 2 {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !matches!(ext.as_str(), "exe" | "dat" | "ccn") {
                continue;
            }
            // A data file that belongs to an .exe next to it is reached through the .exe.
            if ext == "dat" && p.with_extension("exe").is_file() {
                continue;
            }
            if e.metadata().map(|m| m.len()).unwrap_or(0) < 64 * 1024 {
                continue;
            }
            let fusion = crate::ctf_loader::reader::Blob::map_file(&p).is_ok_and(|b| looks_like_fusion(&b))
                || (ext == "exe" && p.with_extension("dat").is_file());
            if fusion {
                found.push(p);
            }
        }
    }
    found.sort();
    found
}

/// Every installed Steam game that contains Fusion game files. `progress` gets each
/// game's name as it is checked.
pub fn find_fusion_games(mut progress: impl FnMut(&str)) -> Vec<SteamGame> {
    let mut out = Vec::new();
    let mut seen_libs = Vec::new();
    for root in steam_roots() {
        for lib in libraries(&root) {
            if seen_libs.contains(&key(&lib)) {
                continue;
            }
            seen_libs.push(key(&lib));
            for (app_id, name, folder) in installed(&lib) {
                progress(&name);
                let files = fusion_files(&folder);
                if !files.is_empty() && !out.iter().any(|g: &SteamGame| key(&g.folder) == key(&folder)) {
                    out.push(SteamGame { app_id, name, folder, files });
                }
            }
        }
    }
    out
}

/// Copy a game folder into `storage` (skipping files that are already there with the same
/// size). Returns the new folder.
pub fn copy_into(game: &SteamGame, storage: &Path) -> std::io::Result<PathBuf> {
    let safe: String = game.name.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let dest = storage.join(safe.trim());
    let mut stack = vec![game.folder.clone()];
    while let Some(dir) = stack.pop() {
        let rel = dir.strip_prefix(&game.folder).unwrap_or(Path::new(""));
        std::fs::create_dir_all(dest.join(rel))?;
        for e in std::fs::read_dir(&dir)?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let target = dest.join(rel).join(e.file_name());
            let same = std::fs::metadata(&target)
                .ok()
                .zip(e.metadata().ok())
                .is_some_and(|(a, b)| a.len() == b.len());
            if !same {
                std::fs::copy(&p, &target)?;
            }
        }
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_a_game_folder_with_subfolders() {
        let base = std::env::temp_dir().join(format!("rekindle-steam-{}", std::process::id()));
        let src = base.join("src").join("My Game");
        std::fs::create_dir_all(src.join("data")).unwrap();
        std::fs::write(src.join("game.exe"), b"exe").unwrap();
        std::fs::write(src.join("data").join("level.bin"), b"lvl").unwrap();
        let g = SteamGame { app_id: "1".into(), name: "My: Game".into(), folder: src.clone(), files: vec![src.join("game.exe")] };
        let dest = copy_into(&g, &base.join("store")).unwrap();
        assert_eq!(dest, base.join("store").join("My_ Game"));
        assert_eq!(std::fs::read(dest.join("data").join("level.bin")).unwrap(), b"lvl");
        assert!(copy_into(&g, &base.join("store")).is_ok(), "copying again skips existing files");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn reads_steam_text_files() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t\t\"label\"\t\t\"\"\n\t}\n}\n";
        let kv = pairs(vdf);
        assert!(kv.contains(&("path".to_string(), "D:\\SteamLibrary".to_string())));
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"123\"\n\t\"name\"\t\t\"Some Game\"\n\t\"installdir\"\t\t\"Some Game\"\n}\n";
        let kv = pairs(acf);
        assert_eq!(kv[0], ("appid".into(), "123".into()));
        assert_eq!(kv[2], ("installdir".into(), "Some Game".into()));
    }
}
