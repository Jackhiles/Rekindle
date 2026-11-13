//! Self-update from GitHub releases.
//!
//! The launcher asks `api.github.com` for the latest release of [`REPO`]. If its version is
//! newer than this build and it has a file for this platform ([`asset_name`]), the launcher
//! offers to install it: the new executable is downloaded next to the current one, the
//! running file is renamed out of the way (Windows allows renaming a running .exe) and the
//! new one takes its place. The old copy is deleted on the next start.
//!
//! Downloads use the system's `curl` (built into Windows 10+, macOS and SteamOS), so no
//! TLS stack is compiled in. Copies running from a `target` build folder never update.

use std::path::{Path, PathBuf};
use std::process::Command;

/// GitHub `owner/name` that publishes releases.
pub const REPO: &str = "Jackhiles/Rekindle";

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub notes: String,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Release file for this platform.
pub fn asset_name() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("rekindle-windows-x86_64.exe")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("rekindle-linux-x86_64")
    } else {
        None
    }
}

/// A developer build (run from Cargo's `target` folder) is never replaced.
pub fn is_dev_build() -> bool {
    std::env::current_exe()
        .map(|p| p.components().any(|c| c.as_os_str().eq_ignore_ascii_case("target")))
        .unwrap_or(true)
}

/// Is version `a` newer than `b`? (Numeric `major.minor.patch`; a leading "v" is ignored.)
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |s: &str| -> Vec<u64> {
        s.trim_start_matches(['v', 'V']).split(['.', '-']).map(|p| p.parse().unwrap_or(0)).take(3).collect()
    };
    parts(a) > parts(b)
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "120", "-H", "User-Agent: Rekindle-updater"])
        .args(args)
        .output()
        .map_err(|e| format!("curl is not available: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The latest release, if it is newer than this build and has a file for this platform.
pub fn check() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = curl(&["-H", "Accept: application/vnd.github+json", &url])?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let tag = json["tag_name"].as_str().unwrap_or_default();
    if tag.is_empty() || !newer(tag, current_version()) {
        return Ok(None);
    }
    let Some(want) = asset_name() else { return Ok(None) };
    let url = json["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str() == Some(want))
        .and_then(|a| a["browser_download_url"].as_str());
    Ok(url.map(|u| Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url: u.to_string(),
        notes: json["body"].as_str().unwrap_or_default().to_string(),
    }))
}

fn old_copy(exe: &Path) -> PathBuf {
    let name = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "rekindle".into());
    let ext = exe.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    exe.with_file_name(format!("{name}.old{ext}"))
}

/// Delete the copy left behind by the last update.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_copy(&exe));
    }
}

/// Download and swap in the new executable. Returns its path (start it to finish updating).
pub fn install(r: &Release) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let tmp = exe.with_extension("download");
    let tmp_s = tmp.to_string_lossy().into_owned();
    curl(&["-o", &tmp_s, &r.url])?;
    let size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    if size < 512 * 1024 {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("the download looks incomplete ({size} bytes)"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    let old = old_copy(&exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("could not move the old version aside: {e}"))?;
    if let Err(e) = std::fs::rename(&tmp, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("could not install the new version: {e}"));
    }
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_order() {
        assert!(newer("v0.2.0", "0.1.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.1.1"));
    }
}
