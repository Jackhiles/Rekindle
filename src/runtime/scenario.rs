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
//! dump_files                 # debug: log the save files written so far
//! set "door" value 0 5       # put an object into a state (counter V | value N V)
//! expect_frame prologue      # current frame, by name or index
//! expect "door" value 0 = 1  # an object's state: counter | value N | x | y | visible, with = != < > <= >=
//! expect "door" @2 x > 100   # only the 2nd instance named "door"
//! expect_visited title       # frame entered at some point
//! expect_save app/save1.txt  # file written into the (temporary) save folder
//! sweep 300                  # run every frame for 300 ticks
//! expect_no_unimplemented    # nothing unsupported was hit
//! ```

use super::script::InputScript;
use super::value::Value;
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
        let mut input = Input { mouse: self.last_mouse, ..Input::default() };
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
    let save_root = std::env::temp_dir().join(format!("rekindle-scenario-{}-{}", result.name, std::process::id()));
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
                                "tick {}: \"{}\" oflags {:#x} layer {} fixed {} at ({:.0}, {:.0}) box ({x1:.0},{y1:.0})-({x2:.0},{y2:.0}) visible {} ink {} alpha {} anim {} frame {} values {:?} counter {} mouse {:?} scroll {:?}",
                                d.tick, obj.name, obj.flags, inst.layer, inst.fixed, inst.x, inst.y, inst.visible, inst.ink_effect, inst.tint[3],
                                inst.anim.anim, inst.anim.frame, inst.values.iter().take(6).map(|v| v.to_string()).collect::<Vec<_>>(), inst.counter, d.last_mouse, d.rt.scroll
                            ));
                            if let Some(h) = inst.current_image(obj) {
                                result.log.push(format!("    image {h}"));
                            }
                            if let Some(w) = &inst.widget {
                                result.log.push(format!(
                                    "    widget {:?} {}x{} text {:?} lines {} current {} visible {} focused {} font {} {}px",
                                    w.kind, w.w, w.h, w.text, w.lines.len(), w.current, w.visible, w.focused, w.font_face, w.font_px
                                ));
                            }
                            if !inst.background {
                                if let Some(o) = d.rt.obstacle_hit(i, true) {
                                    let b = &d.rt.instances[o];
                                    let name = d.rt.game.object(b.oi).map_or("?", |x| x.name.as_str());
                                    result.log.push(format!(
                                        "    overlaps obstacle \"{name}\" (type {}) box {:?}",
                                        b.obstacle,
                                        d.rt.bounds_of(o)
                                    ));
                                }
                            }
                            if let crate::ctf_loader::ObjectProperties::Common(c) = &obj.props {
                                if let Some(data) = &c.extension_data {
                                    let hex: Vec<String> = data.iter().take(320).map(|b| format!("{b:02x}")).collect();
                                    let text: String = data.iter().take(320).map(|&b| if (32..127).contains(&b) { b as char } else { '.' }).collect();
                                    result.log.push(format!("    ext data {} bytes: {}
      {text}", data.len(), hex.join(" ")));
                                }
                                if let Some(m) = c.movements.first() {
                                    let mv = format!("{:?}", m.kind);
                                    result.log.push(format!("    movement {} start {:?}", &mv[..mv.len().min(160)], inst.movement.start));
                                }
                            }
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
                    "dump_files" => {
                        // Debug: list the save sandbox with the start of each text file.
                        let mut stack = vec![d.save_root.clone()];
                        while let Some(dir) = stack.pop() {
                            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                                let p = e.path();
                                if p.is_dir() {
                                    stack.push(p);
                                    continue;
                                }
                                let text = std::fs::read_to_string(&p).unwrap_or_default();
                                let short: String = text.chars().take(600).collect();
                                result.log.push(format!("file {}:
{short}", p.display()));
                            }
                        }
                    }
                    "set" => {
                        // `set "name" counter V` / `set "name" value N V`: put a game into a state.
                        let (name, tail) = rest
                            .strip_prefix('"')
                            .and_then(|r| r.split_once('"'))
                            .ok_or_else(|| anyhow::anyhow!(at("set: object name must be in quotes")))?;
                        let w: Vec<&str> = tail.split_whitespace().collect();
                        let num = |i: usize| -> Result<f64> {
                            w.get(i).and_then(|x| x.parse().ok()).ok_or_else(|| anyhow::anyhow!(at("set: expected a number")))
                        };
                        let (field, index, v) = match w.first() {
                            Some(&"counter") => ("counter", 0, num(1)?),
                            Some(&"value") => ("value", num(1)? as usize, num(2)?),
                            _ => bail!(at("set: field must be counter or value")),
                        };
                        let value = if v.fract() == 0.0 { Value::Int(v as i32) } else { Value::Float(v) };
                        let game = &d.rt.game;
                        let hits: Vec<usize> = d
                            .rt
                            .instances
                            .iter()
                            .enumerate()
                            .filter(|(_, i)| !i.destroyed && game.object(i.oi).is_some_and(|o| o.name.eq_ignore_ascii_case(name)))
                            .map(|(k, _)| k)
                            .collect();
                        if hits.is_empty() {
                            result.failures.push(at(&format!("set: no object \"{name}\"")));
                        }
                        for k in hits {
                            let inst = &mut d.rt.instances[k];
                            if field == "counter" {
                                inst.counter = value.clone();
                            } else {
                                inst.set_value(index, value.clone());
                            }
                        }
                    }
                    "expect" => match check_object(&d.rt, rest) {
                        Ok(None) => {}
                        Ok(Some(msg)) => result.failures.push(at(&msg)),
                        Err(e) => bail!(at(&e)),
                    },
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

/// `"name" field [index] op value`: true if any live instance with that exact name
/// matches. Returns a failure message, or an error for a malformed line.
fn check_object(rt: &Runtime, rest: &str) -> std::result::Result<Option<String>, String> {
    let rest = rest.trim();
    let (name, tail) = rest
        .strip_prefix('"')
        .and_then(|r| r.split_once('"'))
        .ok_or("expect: object name must be in quotes")?;
    let mut words = tail.split_whitespace().peekable();
    // Optional `@N`: only the Nth instance (1-based, creation order).
    let nth = match words.peek() {
        Some(w) if w.starts_with('@') => {
            let n = w[1..].parse::<usize>().map_err(|_| "expect: bad @N")?;
            words.next();
            Some(n.max(1) - 1)
        }
        _ => None,
    };
    let field = words.next().ok_or("expect: missing field")?;
    let index = if field == "value" {
        Some(words.next().and_then(|w| w.parse::<usize>().ok()).ok_or("expect: value needs an index")?)
    } else {
        None
    };
    let op = words.next().ok_or("expect: missing operator")?;
    let want: f64 = words.next().and_then(|w| w.parse().ok()).ok_or("expect: missing number")?;
    let cmp = |v: f64| match op {
        "=" | "==" => (v - want).abs() < 1e-9,
        "!=" => (v - want).abs() >= 1e-9,
        "<" => v < want,
        ">" => v > want,
        "<=" => v <= want,
        ">=" => v >= want,
        _ => false,
    };
    if !matches!(op, "=" | "==" | "!=" | "<" | ">" | "<=" | ">=") {
        return Err(format!("expect: unknown operator `{op}`"));
    }
    let mut seen = Vec::new();
    let named = rt.instances.iter().filter(|i| {
        !i.destroyed && rt.game.object(i.oi).is_some_and(|o| o.name.eq_ignore_ascii_case(name))
    });
    for (k, inst) in named.enumerate() {
        if nth.is_some_and(|n| n != k) {
            continue;
        }
        let v = match field {
            "counter" => inst.counter.as_f64(),
            "value" => inst.value(index.unwrap_or(0)).as_f64(),
            "x" => inst.x,
            "y" => inst.y,
            "visible" => inst.visible as u8 as f64,
            other => return Err(format!("expect: unknown field `{other}`")),
        };
        if cmp(v) {
            return Ok(None);
        }
        seen.push(v);
    }
    Ok(Some(if seen.is_empty() {
        format!("no object \"{name}\"")
    } else {
        format!("\"{name}\" {field} {op} {want} not met (got {seen:?})")
    }))
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
