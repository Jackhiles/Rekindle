//! Application header (chunk 0x2223).

use super::reader::ByteReader;
use anyhow::Result;

#[derive(Debug, Clone, Default)]
pub struct PlayerControls {
    /// 0 = joystick 1, ..., 4 = keyboard (editor control type).
    pub control_type: u16,
    /// Up, Down, Left, Right, Fire1, Fire2, Fire3, Fire4 (Windows virtual-key codes).
    pub keys: [u16; 8],
}

#[derive(Debug, Clone, Default)]
pub struct AppHeader {
    pub flags: u16,
    pub new_flags: u16,
    pub graphics_mode: u16,
    pub other_flags: u16,
    pub window_width: u16,
    pub window_height: u16,
    pub initial_score: u32,
    pub initial_lives: u32,
    pub controls: [PlayerControls; 4],
    pub border_color: [u8; 4],
    pub frame_count: u32,
    pub frame_rate: u32,
    pub windows_menu_index: u8,
}

/// `AppHeader::flags` bits.
pub mod app_flags {
    pub const STRETCH: u16 = 0x0010;
    pub const MAXIMIZE: u16 = 0x0200;
    pub const FULLSCREEN_AT_START: u16 = 0x0800;
    pub const FULLSCREEN_SWITCH: u16 = 0x1000;
}

impl AppHeader {
    /// The game asks to start in fullscreen.
    pub fn fullscreen_at_start(&self) -> bool {
        self.flags & app_flags::FULLSCREEN_AT_START != 0
    }

    /// "Resize display to fill window": stretch the game to the window instead of centring it.
    pub fn stretch(&self) -> bool {
        self.flags & app_flags::STRETCH != 0
    }

    /// Multimedia Fusion 1.5 layout: no size prefix, score/lives stored as-is,
    /// six keys per player and no frame rate (the runtime ran at 50 fps).
    pub fn parse_old(data: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(data);
        let mut h = AppHeader {
            flags: r.read_u16()?,
            new_flags: r.read_u16()?,
            graphics_mode: r.read_u16()?,
            other_flags: r.read_u16()?,
            window_width: r.read_u16()?,
            window_height: r.read_u16()?,
            initial_score: r.read_u32()?,
            initial_lives: r.read_u32()?,
            frame_rate: 50,
            ..Default::default()
        };
        for c in h.controls.iter_mut() {
            c.control_type = r.read_u16()?;
        }
        for c in h.controls.iter_mut() {
            for k in c.keys.iter_mut().take(6) {
                *k = r.read_u16()?;
            }
        }
        h.border_color = r.read_color()?;
        h.frame_count = r.read_u32()?;
        Ok(h)
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(data);
        let _size = r.read_u32()?;
        let mut h = AppHeader {
            flags: r.read_u16()?,
            new_flags: r.read_u16()?,
            graphics_mode: r.read_u16()?,
            other_flags: r.read_u16()?,
            window_width: r.read_u16()?,
            window_height: r.read_u16()?,
            // Stored bit-inverted.
            initial_score: r.read_u32()? ^ 0xFFFF_FFFF,
            initial_lives: r.read_u32()? ^ 0xFFFF_FFFF,
            ..Default::default()
        };
        for c in h.controls.iter_mut() {
            c.control_type = r.read_u16()?;
        }
        for c in h.controls.iter_mut() {
            for k in c.keys.iter_mut() {
                *k = r.read_u16()?;
            }
        }
        h.border_color = r.read_color()?;
        h.frame_count = r.read_u32()?;
        h.frame_rate = r.read_u32()?;
        h.windows_menu_index = r.read_u8().unwrap_or(0);
        Ok(h)
    }
}
