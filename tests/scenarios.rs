//! Runs every scenario (`*.scn`) in the folder named by `REKINDLE_SCENARIOS`.
//! Game files are never part of this repository, so without that variable the test is skipped.

use rekindle::runtime::scenario::{find_scenarios, run_scenario};
use std::path::PathBuf;

#[test]
fn game_scenarios() {
    let Some(dir) = std::env::var_os("REKINDLE_SCENARIOS").map(PathBuf::from) else {
        eprintln!("REKINDLE_SCENARIOS not set: skipping game scenarios");
        return;
    };
    let files = find_scenarios(&dir);
    assert!(!files.is_empty(), "no .scn files in {}", dir.display());
    let mut failed = Vec::new();
    for f in &files {
        match run_scenario(f) {
            Ok(r) if r.passed() => eprintln!("PASS {}", r.name),
            Ok(r) => {
                eprintln!("FAIL {}:\n  {}", r.name, r.failures.join("\n  "));
                failed.push(r.name);
            }
            Err(e) => {
                eprintln!("ERROR {}: {e:#}", f.display());
                failed.push(f.display().to_string());
            }
        }
    }
    assert!(failed.is_empty(), "failing scenarios: {failed:?}");
}
