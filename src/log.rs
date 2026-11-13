//! A plain-text log next to the game's saves (`fusion-rt.log`), so problems can be
//! reported even when the program was started without a console.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The message of the most recent panic, captured by [`install_panic_hook`].
static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);

/// Remember panic messages (with their source location) for [`take_panic`], then run
/// the default hook as usual.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut last) = LAST_PANIC.lock() {
            *last = Some(info.to_string());
        }
        default(info);
    }));
}

/// The message of the last panic, if one happened since the previous call.
pub fn take_panic() -> Option<String> {
    LAST_PANIC.lock().ok().and_then(|mut p| p.take())
}

pub struct Log {
    file: Option<File>,
    pub path: PathBuf,
}

impl Log {
    /// Open (append to) `fusion-rt.log` in `dir`. Logging is silently disabled if the
    /// file can't be created.
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("fusion-rt.log");
        let file = std::fs::create_dir_all(dir)
            .ok()
            .and_then(|_| OpenOptions::new().create(true).append(true).open(&path).ok());
        Self { file, path }
    }

    pub fn line(&mut self, msg: &str) {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "[{stamp}] {msg}");
            let _ = f.flush();
        }
    }
}
