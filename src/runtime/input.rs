//! Platform-independent input snapshot. Keys are Windows virtual-key codes,
//! which is what Fusion stores in key parameters and player controls.

use std::collections::HashSet;

pub mod vk {
    pub const LBUTTON: u16 = 0x01;
    pub const RBUTTON: u16 = 0x02;
    pub const MBUTTON: u16 = 0x04;
}

#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Keys currently held (includes mouse buttons as VK 1/2/4).
    pub down: HashSet<u16>,
    /// Keys that went down since the previous tick.
    pub pressed: HashSet<u16>,
    /// Mouse position in frame coordinates.
    pub mouse: (f64, f64),
    /// Buttons clicked since the previous tick: 0 left, 1 middle, 2 right.
    pub clicked: [bool; 3],
    pub double_clicked: [bool; 3],
    pub wheel: f32,
}

impl Input {
    /// Clear the edge-triggered parts after a tick consumed them.
    pub fn consume_edges(&mut self) {
        self.pressed.clear();
        self.clicked = [false; 3];
        self.double_clicked = [false; 3];
        self.wheel = 0.0;
    }
}
