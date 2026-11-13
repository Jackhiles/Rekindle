//! Play-time log: one line per play session in `<data dir>\Rekindle\playtime.tsv`
//! (`started<TAB>seconds<TAB>game path<TAB>title`), written by the game process when the
//! window closes and read by the launcher.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Session {
    /// Local start time, "YYYY-MM-DD HH:MM".
    pub started: String,
    pub seconds: u64,
    pub path: PathBuf,
    pub title: String,
}

pub fn file() -> PathBuf {
    crate::runtime::files::data_root().join("playtime.tsv")
}

/// Append one session (sessions shorter than 3 seconds are not worth logging).
pub fn record(path: &Path, title: &str, started: chrono::DateTime<chrono::Local>, seconds: u64) {
    if seconds < 3 {
        return;
    }
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let line = format!(
        "{}\t{}\t{}\t{}\n",
        started.format("%Y-%m-%d %H:%M"),
        seconds,
        path.display(),
        title.replace(['\t', '\n'], " ")
    );
    let f = file();
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(f) {
        let _ = out.write_all(line.as_bytes());
    }
}

/// All sessions, oldest first.
pub fn load() -> Vec<Session> {
    let Ok(text) = std::fs::read_to_string(file()) else { return Vec::new() };
    text.lines()
        .filter_map(|l| {
            let mut p = l.splitn(4, '\t');
            Some(Session {
                started: p.next()?.to_string(),
                seconds: p.next()?.parse().ok()?,
                path: PathBuf::from(p.next()?),
                title: p.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Per game (lower-cased path): total seconds and the last start time.
pub fn totals(sessions: &[Session]) -> HashMap<String, (u64, String)> {
    let mut m: HashMap<String, (u64, String)> = HashMap::new();
    for s in sessions {
        let e = m.entry(s.path.to_string_lossy().to_lowercase()).or_default();
        e.0 += s.seconds;
        if s.started > e.1 {
            e.1 = s.started.clone();
        }
    }
    m
}

/// "2h 13m", "14m", "40s".
pub fn duration(secs: u64) -> String {
    let (h, m) = (secs / 3600, secs / 60 % 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_and_durations() {
        let s = |t: &str, secs, p: &str| Session { started: t.into(), seconds: secs, path: p.into(), title: String::new() };
        let all = vec![s("2026-11-12 20:00", 600, "C:\\G.exe"), s("2026-11-13 09:00", 4000, "c:\\g.exe")];
        let t = totals(&all);
        assert_eq!(t["c:\\g.exe"], (4600, "2026-11-13 09:00".to_string()));
        assert_eq!(duration(4600), "1h 16m");
        assert_eq!(duration(59), "59s");
    }
}
