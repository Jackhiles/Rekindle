//! "Joypad object" (joystick.mfx), answered as "a controller with nothing pressed".
//!
//! Games built on it keep their keyboard path working only while it reports a usable
//! joystick (condition 2), and test analog sticks as distance from the centre (32767), so:
//! condition 2 is true, other conditions (buttons, POV directions) are false, the X / Y
//! axis expressions (0, 1) are centred and other expressions are 0. Real gamepads still
//! work through Rekindle's own controller support, which presses the game's keys.

use super::Extension;
use crate::ctf_loader::events::Comparison;
use crate::runtime::files::SaveFs;
use crate::runtime::value::Value;

pub struct Joypad;

impl Extension for Joypad {
    fn condition(&self, idx: usize, _args: &[Value], _cmp: Comparison, _fs: &SaveFs) -> Option<bool> {
        Some(idx == 2)
    }

    fn action(&mut self, _idx: usize, _args: &[Value], _fs: &SaveFs) -> bool {
        true
    }

    fn expression(&self, idx: usize, _args: &[Value], _fs: &SaveFs) -> Option<Value> {
        Some(Value::Int(if idx <= 1 { 32767 } else { 0 }))
    }
}
