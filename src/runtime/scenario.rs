//! Scripted regression scenarios: drive a game without a window and check the outcome.
//!
//! A scenario is a small text file (game files are never part of this repository, so
//! scenarios live next to the user's games). One command per line, `#` starts a comment:
//!
//! ```text
//! game The Game.exe          # path relative to the scenario file
//! frame 0                    # start frame (optional)
//! press 50 13                # press VK 13 (Enter) at tick 50
//! hold 100 160 39            # hold VK 39 (Right) from tick 100 to 160
//! click 200 320 240          # left-click at (320, 240) at tick 200 (also `rclick`, `mouse`)
//! run 900                    # advance to tick 900
//! dump door                  # debug: log objects whose name contains "door"
//! expect_frame prologue      # current frame, by name or index
//! expect_visited title       # frame entered at some point
//! expect_save app/save1.txt  # file written into the (temporary) save folder
//! sweep 300                  # run every frame for 300 ticks
//! expect_no_unimplemented    # nothing unsupported was hit
//! ```

use super::script::InputScript;
use super::{Input, Runtime};
use crate::ctf_loader::GameData;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct ScenarioResult {
    pub name: String,
    pub failures: Vec<String>,
    pub log: Vec<String>,
}

impl ScenarioResult {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

struct Driver {
    rt: Runtime,
    tick: u64,
    script: InputScript,
    last_mouse: (f64, f64),
    save_root: PathBuf,
}

impl Driver {
    fn advance_to(&mut self, until: u64) {
        let mut input = Input::default();
        while self.tick < until && !self.rt.finished {
            let scroll = self.rt.scroll;
            self.script.apply(self.tick, &mut input, scroll);
            self.rt.tick(&input);
            self.last_mouse = input.mouse;
            input.consume_edges();
            self.tick += 1;
        }
    }

    fn frame_matches(&self, index: usize, want: &str) -> bool {
        want.parse::<usize>().map_or_else(
            |_| self.rt.game.frames.get(index).is_some_and(|f| f.name.eq_ignore_ascii_case(want)),
            |n| n == index,
        )
    }
}

/// Run one scenario file.
pub fn run_scenario(path: &Path) -> Result<ScenarioResult> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut result = ScenarioResult { name, failures: Vec::new(), log: Vec::new() };
    let mut driver: Option<Driver> = None;
    let mut start_frame = 0usize;
    let save_root = std::env::temp_dir().join(format!("fusion-rt-scenario-{}-{}", result.name, std::process::id()));
    let _ = std::fs::remove_dir_all(&save_root);

    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let nums = || -> Result<Vec<u64>> {
            rest.split_whitespace().map(|n| n.parse::<u64>().context("expected a number")).collect()
        };
        let at = |msg: &str| format!("line {}: {msg}", lineno + 1);
        match cmd {
            "game" => {
                let game = GameData::load(dir.join(rest))?;
                let rt = Runtime::with_save_root(game, start_frame, Some(save_root.clone()));
                driver = Some(Driver { rt, tick: 0, script: InputScript::default(), last_mouse: (0.0, 0.0), save_root: save_root.clone() });
            }
            "frame" => {
                start_frame = rest.parse().context("frame index")?;
                if let Some(d) = &mut driver {
                    d.rt.load_frame(start_frame);
                }
            }
            _ => {
                let Some(d) = driver.as_mut() else { bail!(at("`game` must come first")) };
                if d.script.parse_command(cmd, rest).map_err(|e| anyhow::anyhow!(at(&e)))? {
                    continue;
                }
                match cmd {
                    "run" => {
                        let n = nums()?[0];
                        d.advance_to(n);
                        result.log.push(format!(
                            "tick {}: frame {} \"{}\"",
                            d.tick,
                            d.rt.frame_index,
                            d.rt.frame().name
                        ));
                    }
                    "sweep" => {
                        let n = nums()?[0];
                        for f in 0..d.rt.game.frames.len() {
                            d.rt.load_frame(f);
                            let mut input = Input::default();
                            for t in 0..n {
                                if t % 90 == 30 {
                                    input.pressed.insert(13);
                                    input.clicked[0] = true;
                                }
                                d.rt.tick(&input);
                                input.consume_edges();
                                d.rt.finished = false;
                            }
                        }
                    }
                    "dump" => {
                        // Debug: print every instance whose name contains `rest`.
                        let want = rest.to_ascii_lowercase();
                        for (i, inst) in d.rt.instances.iter().enumerate() {
                            let Some(obj) = d.rt.game.object(inst.oi) else { continue };
                            if inst.destroyed || !obj.name.to_ascii_lowercase().contains(&want) {
                                continue;
                            }
                            let (x1, y1, x2, y2) = d.rt.bounds_of(i);
                            result.log.push(format!(
                                "tick {}: \"{}\" at ({:.0}, {:.0}) box ({x1:.0},{y1:.0})-({x2:.0},{y2:.0}) visible {} ink {} alpha {} anim {} frame {} value0 {:?} mouse {:?} scroll {:?}",
                                d.tick, obj.name, inst.x, inst.y, inst.visible, inst.ink_effect, inst.tint[3],
                                inst.anim.anim, inst.anim.frame, inst.values.first(), d.last_mouse, d.rt.scroll
                            ));
                        }
                    }
                    "expect_frame" => {
                        if !d.frame_matches(d.rt.frame_index, rest) {
                            result.failures.push(at(&format!(
                                "expected frame {rest}, got {} \"{}\"",
                                d.rt.frame_index,
                                d.rt.frame().name
                            )));
                        }
                    }
                    "expect_visited" => {
                        if !d.rt.frame_log.iter().any(|&f| d.frame_matches(f, rest)) {
                            result.failures.push(at(&format!("frame {rest} was never entered")));
                        }
                    }
                    "expect_save" => {
                        if !d.save_root.join(rest).is_file() {
                            result.failures.push(at(&format!("no save file {rest}")));
                        }
                    }
                    "expect_no_unimplemented" => {
                        let un = d.rt.unimplemented.borrow();
                        if !un.is_empty() {
                            let list: Vec<String> = un.iter().map(|(k, n)| format!("{k} x{n}")).collect();
                            result.failures.push(at(&format!("unsupported features hit: {}", list.join(", "))));
                        }
                    }
                    other => bail!(at(&format!("unknown command `{other}`"))),
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&save_root);
    Ok(result)
}

/// All `*.scn` files in a directory, sorted.
pub fn find_scenarios(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "scn"))
        .collect();
    v.sort();
    v
}
