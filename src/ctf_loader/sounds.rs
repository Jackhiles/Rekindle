//! Sound (0x6668) and music (0x6669) banks.
//!
//! Per entry: `handle, checksum, references, decompressed_size, flags, reserved,
//! name_length` (all u32), then either `compressed_size: u32 + zlib` or raw bytes.
//! The payload starts with the sound name (`name_length` chars) followed by the file.

use super::chunk::{inflate, inflate_prefix};
use super::reader::{decode_cstr, Blob, ByteReader};
use anyhow::{Context, Result};
use std::collections::HashMap;

/// Flag value marking an uncompressed entry.
const FLAGS_UNCOMPRESSED: u32 = 33;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AudioFormat {
    Wav,
    Ogg,
    Mp3,
    Midi,
    Aiff,
    Mod,
    Unknown,
}

impl AudioFormat {
    pub fn sniff(b: &[u8]) -> Self {
        match b {
            [b'R', b'I', b'F', b'F', ..] => Self::Wav,
            [b'O', b'g', b'g', b'S', ..] => Self::Ogg,
            [b'I', b'D', b'3', ..] => Self::Mp3,
            [0xFF, b, ..] if b & 0xE0 == 0xE0 => Self::Mp3,
            [b'M', b'T', b'h', b'd', ..] => Self::Midi,
            [b'F', b'O', b'R', b'M', ..] => Self::Aiff,
            _ if b.len() >= 1084
                && matches!(&b[1080..1084], b"M.K." | b"M!K!" | b"4CHN" | b"8CHN") =>
            {
                Self::Mod
            }
            _ => Self::Unknown,
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Ogg => "ogg",
            Self::Mp3 => "mp3",
            Self::Midi => "mid",
            Self::Aiff => "aiff",
            Self::Mod => "mod",
            Self::Unknown => "bin",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SoundEntry {
    pub handle: u32,
    pub name: String,
    pub flags: u32,
    pub format: AudioFormat,
    pub decompressed_size: u32,
    name_bytes: usize,
    compressed: bool,
    payload: Blob,
}

impl SoundEntry {
    /// The embedded audio file (WAV/OGG/...), name prefix stripped.
    pub fn data(&self) -> Result<Vec<u8>> {
        let full = if self.compressed {
            inflate(&self.payload, self.decompressed_size as usize)?
        } else {
            self.payload.to_vec()
        };
        Ok(full.get(self.name_bytes..).unwrap_or_default().to_vec())
    }
}

#[derive(Debug, Default)]
pub struct SoundBank {
    pub sounds: Vec<SoundEntry>,
    by_handle: HashMap<u32, usize>,
}

impl SoundBank {
    /// Multimedia Fusion 1.5 sound bank: each entry is `handle u32` plus a 1.5-compressed
    /// block of `checksum u16, references u32, size u32, flags u32, reserved u32,
    /// name_len u32, name, WAVEFORMAT (format u16, channels u16, rate u32, byte rate u32,
    /// block align u16, bits u16), u16, data_size u32, PCM data`. Rebuilt as WAV files.
    pub fn parse_old(bank: &Blob) -> Result<Self> {
        let mut r = ByteReader::new(bank);
        let count = r.read_u32()? as usize;
        let mut sounds = Vec::with_capacity(count);
        for i in 0..count {
            let handle = r.read_u32()?;
            let (raw, used) = super::old_inflate::inflate_block(&bank[r.pos()..])
                .with_context(|| format!("sound #{i} (handle {handle})"))?;
            r.skip(used)?;
            let mut s = ByteReader::new(&raw);
            let _checksum = s.read_u16()?;
            let _references = s.read_u32()?;
            let _size = s.read_u32()?;
            let flags = s.read_u32()?;
            let _reserved = s.read_u32()?;
            let name_len = s.read_u32()? as usize;
            let name = super::reader::decode_cstr(s.read_bytes(name_len)?, false)
                .trim()
                .to_string();
            let fmt = s.read_bytes(16)?.to_vec();
            let _unknown = s.read_u16()?;
            let data_len = s.read_u32()? as usize;
            let pcm = s.read_bytes(data_len.min(s.remaining()))?;
            let mut wav = Vec::with_capacity(44 + pcm.len());
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16u32.to_le_bytes());
            wav.extend_from_slice(&fmt);
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
            wav.extend_from_slice(pcm);
            sounds.push(SoundEntry {
                handle,
                name,
                flags,
                format: AudioFormat::Wav,
                decompressed_size: wav.len() as u32,
                name_bytes: 0,
                compressed: false,
                payload: Blob::new(wav),
            });
        }
        let by_handle = sounds
            .iter()
            .enumerate()
            .map(|(i, s)| (s.handle, i))
            .collect();
        Ok(Self { sounds, by_handle })
    }

    /// Builds >= 284 store handles 1-based; they are normalised to the 0-based references used in events.
    pub fn parse(bank: &Blob, unicode: bool, build: u32) -> Result<Self> {
        let mut r = ByteReader::new(bank);
        let count = r.read_u32()? as usize;
        let mut sounds = Vec::with_capacity(count);
        for i in 0..count {
            let handle = r.read_u32()?.saturating_sub((build >= 284) as u32);
            let _checksum = r.read_i32()?;
            let _references = r.read_u32()?;
            let decompressed_size = r.read_u32()?;
            let flags = r.read_u32()?;
            let _reserved = r.read_u32()?;
            let name_len = r.read_u32()? as usize;
            let compressed = flags != FLAGS_UNCOMPRESSED;
            let size = if compressed {
                r.read_u32()? as usize
            } else {
                decompressed_size as usize
            };
            let start = r.pos();
            r.skip(size)
                .with_context(|| format!("sound #{i} (handle {handle}) truncated"))?;
            let payload = bank.slice(start..start + size)?;

            let name_bytes = if unicode { name_len * 2 } else { name_len };
            let peek_len = name_bytes + 1100;
            let head = if compressed {
                inflate_prefix(&payload, peek_len)
                    .with_context(|| format!("sound #{i} (handle {handle})"))?
            } else {
                payload[..peek_len.min(payload.len())].to_vec()
            };
            let name = decode_cstr(&head[..name_bytes.min(head.len())], unicode)
                .trim()
                .to_string();
            let format = AudioFormat::sniff(head.get(name_bytes..).unwrap_or_default());
            sounds.push(SoundEntry {
                handle,
                name,
                flags,
                format,
                decompressed_size,
                name_bytes,
                compressed,
                payload,
            });
        }
        let by_handle = sounds
            .iter()
            .enumerate()
            .map(|(i, s)| (s.handle, i))
            .collect();
        Ok(Self { sounds, by_handle })
    }

    pub fn get(&self, handle: u32) -> Option<&SoundEntry> {
        self.by_handle.get(&handle).map(|&i| &self.sounds[i])
    }

    pub fn len(&self) -> usize {
        self.sounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
