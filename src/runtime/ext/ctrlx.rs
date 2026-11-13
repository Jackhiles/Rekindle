//! "Control X" (ctrlx.mfx): keyboard tests by key name, so games can let players rebind
//! keys by storing names like "X", "Space" or "Down".
//!
//! Implemented: condition 0, "key is pressed (by name)". Other conditions answer false.

use super::{arg_s, Extension};
use crate::ctf_loader::events::Comparison;
use crate::runtime::files::SaveFs;
use crate::runtime::value::Value;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// Keys held this tick, shared with the runtime.
pub type Keys = Rc<RefCell<HashSet<u16>>>;

pub struct ControlX {
    pub keys: Keys,
}

/// Windows virtual-key code for a key name as Windows' key-name API spells it.
pub fn vk_for_name(name: &str) -> Option<u16> {
    let n = name.trim();
    let upper = n.to_ascii_uppercase();
    if upper.len() == 1 {
        let c = upper.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u16);
        }
    }
    if let Some(f) = upper.strip_prefix('F').and_then(|d| d.parse::<u16>().ok()) {
        if (1..=24).contains(&f) {
            return Some(0x6F + f);
        }
    }
    if let Some(d) = upper.strip_prefix("NUM ").and_then(|d| d.parse::<u16>().ok()) {
        if d <= 9 {
            return Some(0x60 + d);
        }
    }
    Some(match upper.as_str() {
        "SPACE" => 0x20,
        "ENTER" | "RETURN" => 0x0D,
        "ESC" | "ESCAPE" => 0x1B,
        "TAB" => 0x09,
        "BACKSPACE" => 0x08,
        "SHIFT" | "LEFT SHIFT" | "RIGHT SHIFT" => 0x10,
        "CTRL" | "CONTROL" | "LEFT CTRL" | "RIGHT CTRL" => 0x11,
        "ALT" | "LEFT ALT" | "RIGHT ALT" => 0x12,
        "UP" => 0x26,
        "DOWN" => 0x28,
        "LEFT" => 0x25,
        "RIGHT" => 0x27,
        "INSERT" | "INS" => 0x2D,
        "DELETE" | "DEL" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGE UP" | "PGUP" => 0x21,
        "PAGE DOWN" | "PGDN" => 0x22,
        "CAPS LOCK" => 0x14,
        _ => return None,
    })
}

impl Extension for ControlX {
    fn condition(&self, idx: usize, args: &[Value], _cmp: Comparison, _fs: &SaveFs) -> Option<bool> {
        match idx {
            0 => Some(vk_for_name(&arg_s(args, 0)).is_some_and(|vk| {
                let keys = self.keys.borrow();
                // Shift / Ctrl / Alt by name match either side.
                keys.contains(&vk) || matches!(vk, 0x10..=0x12) && keys.iter().any(|k| (0xA0..=0xA5).contains(k) && (k - 0xA0) / 2 == vk - 0x10)
            })),
            _ => Some(false),
        }
    }

    fn action(&mut self, _idx: usize, _args: &[Value], _fs: &SaveFs) -> bool {
        true
    }

    fn expression(&self, _idx: usize, _args: &[Value], _fs: &SaveFs) -> Option<Value> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::vk_for_name;

    #[test]
    fn key_names() {
        assert_eq!(vk_for_name("X"), Some(0x58));
        assert_eq!(vk_for_name("space"), Some(0x20));
        assert_eq!(vk_for_name("Down"), Some(0x28));
        assert_eq!(vk_for_name("F5"), Some(0x74));
        assert_eq!(vk_for_name("nonsense"), None);
    }
}
