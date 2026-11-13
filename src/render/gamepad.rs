//! Gamepad support for games that only know keyboard and mouse: controller input is turned
//! into the key presses, clicks and mouse movement the game already understands.
//!
//! Default layout (per game, changeable in the launcher or the game's settings file):
//!
//! | Control              | Becomes                                   |
//! |----------------------|-------------------------------------------|
//! | Left stick / D-pad   | arrow keys (or W A S D)                   |
//! | Right stick          | moves an on-screen mouse cursor           |
//! | RB, RT               | left click                                |
//! | LB, LT               | right click                               |
//! | A / Start            | Enter                                     |
//! | B                    | Space                                     |
//! | X                    | Ctrl                                      |
//! | Y                    | Shift                                     |
//! | Back / Select        | Escape                                    |
//!
//! Remaps go in the settings file as `pad_<button>=<key>`, where button is one of
//! `south east west north start select lb rb lt rt l3 r3 up down left right` and the value
//! is a Windows key code (13 = Enter, 32 = Space, 65 = A, ...), `click`, `rclick` or `none`.

use super::settings::Settings;
use crate::runtime::Input;
use gilrs::{Axis, Button, Gilrs};
use macroquad::prelude::*;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Key(u16),
    Click(usize),
    None,
}

const BUTTONS: [(&str, Button); 16] = [
    ("south", Button::South),
    ("east", Button::East),
    ("west", Button::West),
    ("north", Button::North),
    ("start", Button::Start),
    ("select", Button::Select),
    ("lb", Button::LeftTrigger),
    ("rb", Button::RightTrigger),
    ("lt", Button::LeftTrigger2),
    ("rt", Button::RightTrigger2),
    ("l3", Button::LeftThumb),
    ("r3", Button::RightThumb),
    ("up", Button::DPadUp),
    ("down", Button::DPadDown),
    ("left", Button::DPadLeft),
    ("right", Button::DPadRight),
];

fn default_action(b: Button, wasd: bool) -> Action {
    let dir = |arrow: u16, letter: u16| Action::Key(if wasd { letter } else { arrow });
    match b {
        Button::South | Button::Start => Action::Key(13),
        Button::East => Action::Key(32),
        Button::West => Action::Key(17),
        Button::North => Action::Key(16),
        Button::Select => Action::Key(27),
        Button::RightTrigger | Button::RightTrigger2 => Action::Click(0),
        Button::LeftTrigger | Button::LeftTrigger2 => Action::Click(2),
        Button::DPadUp => dir(38, 87),
        Button::DPadDown => dir(40, 83),
        Button::DPadLeft => dir(37, 65),
        Button::DPadRight => dir(39, 68),
        _ => Action::None,
    }
}

fn parse_action(v: &str) -> Option<Action> {
    match v.trim() {
        "click" => Some(Action::Click(0)),
        "rclick" => Some(Action::Click(2)),
        "none" => Some(Action::None),
        n => n.parse::<u16>().ok().map(Action::Key),
    }
}

pub struct Gamepad {
    gilrs: Option<Gilrs>,
    map: Vec<(Button, Action)>,
    wasd: bool,
    speed: f32,
    enabled: bool,
    /// Buttons / stick directions held last frame (for press edges).
    held: HashSet<Button>,
    held_stick: HashSet<u16>,
    /// Virtual cursor in game-view coordinates, while the right stick drives the mouse.
    pub cursor: Option<Vec2>,
    last_real_mouse: Vec2,
}

impl Gamepad {
    pub fn new(s: &Settings) -> Self {
        let map = BUTTONS
            .iter()
            .map(|&(name, b)| {
                let custom = s.pad_map.iter().find(|(k, _)| k == name).and_then(|(_, v)| parse_action(v));
                (b, custom.unwrap_or_else(|| default_action(b, s.pad_wasd)))
            })
            .collect();
        Self {
            gilrs: if s.pad { Gilrs::new().ok() } else { None },
            map,
            wasd: s.pad_wasd,
            speed: [350.0, 650.0, 1100.0][(s.pad_speed.clamp(1, 3) - 1) as usize],
            enabled: s.pad,
            held: HashSet::new(),
            held_stick: HashSet::new(),
            cursor: None,
            last_real_mouse: Vec2::from(mouse_position()),
        }
    }

    /// Add the gamepad's input to `input`. `view` is the game view size; `to_view` maps a
    /// window position into it and `scroll` is the frame's current scroll.
    pub fn apply(&mut self, input: &mut Input, view: Vec2, to_view: impl Fn(Vec2) -> Vec2, scroll: Vec2) {
        if !self.enabled {
            return;
        }
        let Some(g) = self.gilrs.as_mut() else { return };
        while g.next_event().is_some() {}
        let Some((_, pad)) = g.gamepads().find(|(_, p)| p.is_connected()) else {
            self.cursor = None;
            return;
        };

        // Buttons.
        let mut now = HashSet::new();
        for &(b, action) in &self.map {
            if !pad.is_pressed(b) {
                continue;
            }
            now.insert(b);
            let edge = !self.held.contains(&b);
            match action {
                Action::Key(k) => {
                    input.down.insert(k);
                    if edge {
                        input.pressed.insert(k);
                    }
                }
                Action::Click(i) => {
                    input.down.insert([1u16, 4, 2][i.min(2)]);
                    if edge {
                        input.clicked[i.min(2)] = true;
                    }
                }
                Action::None => {}
            }
        }
        self.held = now;

        // Left stick acts like the D-pad.
        let (lx, ly) = (pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
        let mut stick = HashSet::new();
        let keys = if self.wasd { [87, 83, 65, 68] } else { [38, 40, 37, 39] };
        for (on, k) in [(ly > 0.45, keys[0]), (ly < -0.45, keys[1]), (lx < -0.45, keys[2]), (lx > 0.45, keys[3])] {
            if on {
                stick.insert(k);
                input.down.insert(k);
                if !self.held_stick.contains(&k) {
                    input.pressed.insert(k);
                }
            }
        }
        self.held_stick = stick;

        // Right stick moves a virtual cursor; touching the real mouse hands control back.
        let real = Vec2::from(mouse_position());
        if (real - self.last_real_mouse).length() > 1.0 {
            self.cursor = None;
        }
        self.last_real_mouse = real;
        let (rx, ry) = (pad.value(Axis::RightStickX), pad.value(Axis::RightStickY));
        let v = vec2(rx, -ry);
        let mag = v.length();
        if mag > 0.18 {
            // Ease in: small tilts move slowly for precise aiming.
            let scaled = v / mag * ((mag - 0.18) / 0.82).powf(1.6);
            let start = self.cursor.unwrap_or_else(|| to_view(real));
            let next = start + scaled * self.speed * get_frame_time();
            self.cursor = Some(next.clamp(Vec2::ZERO, view - Vec2::ONE));
        }
        if let Some(c) = self.cursor {
            let m = c + scroll;
            input.mouse = (m.x as f64, m.y as f64);
        }
    }

    /// Draw the virtual cursor at a window position.
    pub fn draw_cursor(&self, at: Vec2) {
        let (a, b, c) = (at, at + vec2(0.0, 18.0), at + vec2(12.0, 13.0));
        draw_triangle(a + vec2(1.5, 1.5), b + vec2(1.5, 1.5), c + vec2(1.5, 1.5), Color::new(0.0, 0.0, 0.0, 0.6));
        draw_triangle(a, b, c, WHITE);
        draw_triangle_lines(a, b, c, 1.5, BLACK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaps_parse() {
        assert_eq!(parse_action("65"), Some(Action::Key(65)));
        assert_eq!(parse_action("click"), Some(Action::Click(0)));
        assert_eq!(parse_action("none"), Some(Action::None));
        assert_eq!(default_action(Button::DPadUp, true), Action::Key(87));
        assert_eq!(default_action(Button::DPadUp, false), Action::Key(38));
    }
}
