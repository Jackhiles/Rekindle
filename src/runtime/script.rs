//! Scripted input: key presses, holds, mouse moves and clicks at given ticks.
//!
//! Shared by scenarios (headless) and the screenshot mode (`--screenshot DIR --script FILE`),
//! which also understands `shot TICK name.png`.
//!
//! ```text
//! press 50 13          # VK 13 (Enter) at tick 50
//! hold 100 160 39      # hold VK 39 (Right) from tick 100 until 160
//! mouse 200 320 240    # move the mouse to (320, 240) at tick 200
//! click 210 320 240    # move there and left-click at tick 210 (`rclick` for the right button)
//! type 250 Hello      # type text (into the focused edit box) at tick 250
//! shot 300 menu.png    # screenshot mode only: capture at tick 300
//! ```

use super::Input;

#[derive(Debug, Clone)]
enum Event {
    Press(u64, u16),
    Hold(u64, u64, u16),
    Mouse(u64, f64, f64),
    Click(u64, f64, f64, usize),
    Type(u64, String),
}

#[derive(Debug, Clone, Default)]
pub struct InputScript {
    events: Vec<Event>,
    /// (tick, file name) pairs for screenshot mode.
    pub shots: Vec<(u64, String)>,
}

/// How long a scripted press keeps the key down, in ticks.
const TAP_TICKS: u64 = 3;

impl InputScript {
    /// Parse a whole script; unknown commands are an error.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut s = Self::default();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            if !s.parse_command(cmd, rest.trim())? {
                return Err(format!("line {}: unknown command `{cmd}`", n + 1));
            }
        }
        Ok(s)
    }

    /// Handle one input command. Returns false if `cmd` isn't an input command.
    pub fn parse_command(&mut self, cmd: &str, rest: &str) -> Result<bool, String> {
        let nums = |want: usize| -> Result<Vec<f64>, String> {
            let v: Vec<f64> = rest
                .split_whitespace()
                .take(want)
                .map(|n| n.parse::<f64>().map_err(|_| format!("`{cmd}`: expected a number, got `{n}`")))
                .collect::<Result<_, _>>()?;
            if v.len() < want {
                return Err(format!("`{cmd}` needs {want} numbers"));
            }
            Ok(v)
        };
        match cmd {
            "press" => {
                let n = nums(2)?;
                self.events.push(Event::Press(n[0] as u64, n[1] as u16));
            }
            "hold" => {
                let n = nums(3)?;
                self.events.push(Event::Hold(n[0] as u64, n[1] as u64, n[2] as u16));
            }
            "mouse" => {
                let n = nums(3)?;
                self.events.push(Event::Mouse(n[0] as u64, n[1], n[2]));
            }
            "click" | "rclick" => {
                let n = nums(3)?;
                let button = if cmd == "click" { 0 } else { 2 };
                self.events.push(Event::Click(n[0] as u64, n[1], n[2], button));
            }
            "type" => {
                let (t, text) = rest.split_once(char::is_whitespace).ok_or("`type` needs a tick and text")?;
                let t = t.parse::<u64>().map_err(|_| "`type`: bad tick".to_string())?;
                self.events.push(Event::Type(t, text.trim().to_string()));
            }
            "shot" => {
                let (t, name) = rest.split_once(char::is_whitespace).ok_or("`shot` needs a tick and a file name")?;
                let t = t.parse::<u64>().map_err(|_| "`shot`: bad tick".to_string())?;
                self.shots.push((t, name.trim().to_string()));
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The last tick anything happens at.
    pub fn last_tick(&self) -> u64 {
        let ev = self.events.iter().map(|e| match *e {
            Event::Press(t, _) | Event::Mouse(t, ..) | Event::Click(t, ..) | Event::Type(t, _) => t,
            Event::Hold(_, to, _) => to,
        });
        ev.chain(self.shots.iter().map(|s| s.0)).max().unwrap_or(0)
    }

    /// Set `input` for `tick`. Mouse positions are window coordinates, so `scroll` (the
    /// frame's current scroll) is added, as for a real mouse. Call before `Runtime::tick`,
    /// then `consume_edges` after it.
    pub fn apply(&self, tick: u64, input: &mut Input, scroll: (f64, f64)) {
        input.down.clear();
        let last_move = self
            .events
            .iter()
            .filter_map(|e| match *e {
                Event::Mouse(t, x, y) | Event::Click(t, x, y, _) if t <= tick => Some((t, x, y)),
                _ => None,
            })
            .max_by_key(|m| m.0);
        if let Some((_, x, y)) = last_move {
            input.mouse = (x + scroll.0, y + scroll.1);
        }
        for e in &self.events {
            match *e {
                Event::Press(t, k) => {
                    if t == tick {
                        input.pressed.insert(k);
                    }
                    if (t..t + TAP_TICKS).contains(&tick) {
                        input.down.insert(k);
                    }
                }
                Event::Hold(from, to, k) => {
                    if (from..to).contains(&tick) {
                        input.down.insert(k);
                        if from == tick {
                            input.pressed.insert(k);
                        }
                    }
                }
                Event::Mouse(..) => {}
                Event::Type(t, ref text) => {
                    if t == tick {
                        input.typed.extend(text.chars());
                    }
                }
                Event::Click(t, _, _, b) => {
                    if t == tick {
                        input.clicked[b] = true;
                    }
                    if (t..t + TAP_TICKS).contains(&tick) {
                        input.down.insert([1u16, 4, 2][b]);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_moves_mouse_and_sets_button() {
        let s = InputScript::parse("click 5 10 20\npress 6 13\nshot 9 a.png").unwrap();
        let mut i = Input::default();
        s.apply(5, &mut i, (0.0, 0.0));
        assert_eq!(i.mouse, (10.0, 20.0));
        assert!(i.clicked[0] && i.down.contains(&1));
        i.consume_edges();
        s.apply(6, &mut i, (100.0, 0.0));
        assert_eq!(i.mouse, (110.0, 20.0), "mouse position persists and follows the scroll");
        assert!(!i.clicked[0] && i.pressed.contains(&13));
        assert_eq!(s.last_tick(), 9);
    }
}
