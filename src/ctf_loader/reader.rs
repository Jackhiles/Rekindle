//! Little-endian cursor over a byte slice, plus string helpers.

use anyhow::{bail, Result};
use std::ops::{Deref, Range};
use std::sync::Arc;

/// Bytes behind a [`Blob`]: an owned buffer, or a read-only memory-mapped file.
enum Backing {
    Owned(Vec<u8>),
    Mapped(memmap2::Mmap),
}

impl Deref for Backing {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Backing::Owned(v) => v,
            Backing::Mapped(m) => m,
        }
    }
}

/// A cheaply clonable, shared view into a byte buffer.
///
/// The game file is memory-mapped once; uncompressed chunks and bank entries are just
/// ranges into it, so large banks are never copied and the OS only reads (and keeps
/// in memory) the parts that are actually used.
#[derive(Clone)]
pub struct Blob {
    buf: Arc<Backing>,
    range: Range<usize>,
}

impl Blob {
    pub fn new(data: Vec<u8>) -> Self {
        let len = data.len();
        Self {
            buf: Arc::new(Backing::Owned(data)),
            range: 0..len,
        }
    }

    /// Map a file read-only (falls back to reading it if mapping isn't possible).
    pub fn map_file(path: &std::path::Path) -> std::io::Result<Self> {
        let file = std::fs::File::open(path)?;
        // SAFETY: the game file is opened read-only and treated as immutable input;
        // games are not expected to be modified while they run.
        let backing = match unsafe { memmap2::Mmap::map(&file) } {
            Ok(m) => Backing::Mapped(m),
            Err(_) => Backing::Owned(std::fs::read(path)?),
        };
        let len = backing.len();
        Ok(Self {
            buf: Arc::new(backing),
            range: 0..len,
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf[self.range.clone()]
    }

    /// Sub-view relative to this blob's start.
    pub fn slice(&self, range: Range<usize>) -> Result<Blob> {
        if range.start > range.end || range.end > self.range.len() {
            bail!(
                "slice {:?} out of bounds (blob length {})",
                range,
                self.range.len()
            );
        }
        let start = self.range.start + range.start;
        Ok(Blob {
            buf: self.buf.clone(),
            range: start..start + range.len(),
        })
    }
}

impl Deref for Blob {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Blob({} bytes)", self.range.len())
    }
}

pub struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
}

macro_rules! read_le {
    ($name:ident, $t:ty) => {
        pub fn $name(&mut self) -> Result<$t> {
            const N: usize = std::mem::size_of::<$t>();
            let b = self.read_bytes(N)?;
            Ok(<$t>::from_le_bytes(b.try_into().expect("length checked")))
        }
    };
}

impl<'a> ByteReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            bail!("seek to {pos:#x} past end of data ({:#x})", self.data.len());
        }
        self.pos = pos;
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.read_bytes(n).map(|_| ())
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        match self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
        {
            Some(end) => {
                let s = &self.data[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            None => bail!(
                "unexpected end of data: wanted {n} bytes at offset {:#x}, {} left",
                self.pos,
                self.remaining()
            ),
        }
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.data[self.pos.min(self.data.len())..];
        self.pos = self.data.len();
        s
    }

    read_le!(read_u8, u8);
    read_le!(read_i8, i8);
    read_le!(read_u16, u16);
    read_le!(read_i16, i16);
    read_le!(read_u32, u32);
    read_le!(read_i32, i32);
    read_le!(read_f32, f32);

    /// Four colour bytes stored as R, G, B, A.
    pub fn read_color(&mut self) -> Result<[u8; 4]> {
        Ok(self.read_bytes(4)?.try_into().expect("length checked"))
    }

    /// Null-terminated string; UTF-16LE when `unicode`, otherwise Latin-1.
    pub fn read_cstring(&mut self, unicode: bool) -> Result<String> {
        if unicode {
            let mut units = Vec::new();
            loop {
                let u = self.read_u16()?;
                if u == 0 {
                    break;
                }
                units.push(u);
            }
            Ok(String::from_utf16_lossy(&units))
        } else {
            let start = self.pos;
            let rest = &self.data[start..];
            let len = rest
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| anyhow::anyhow!("unterminated string at offset {start:#x}"))?;
            self.pos += len + 1;
            Ok(latin1(&rest[..len]))
        }
    }

    /// Fixed-size string field of `chars` characters, cut at the first null.
    pub fn read_fixed_string(&mut self, chars: usize, unicode: bool) -> Result<String> {
        let b = self.read_bytes(if unicode { chars * 2 } else { chars })?;
        Ok(decode_cstr(b, unicode))
    }
}

pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// Decode a buffer holding a (possibly) null-terminated string.
pub fn decode_cstr(b: &[u8], unicode: bool) -> String {
    if unicode {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        latin1(&b[..end])
    }
}
