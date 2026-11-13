//! Locating the Fusion game data inside a Windows runtime executable or a bare .ccn.
//!
//! Layout of a built Fusion .exe:
//!   [PE image (stub runtime)] [optional "wwww" pack of embedded files] ["PAME"/"PAMU" game header + chunks]
//! A .ccn file is just the game header + chunks.

use super::reader::{decode_cstr, ByteReader};
use anyhow::{bail, Context, Result};

pub const CHUNK_APP_HEADER: u16 = 0x2223;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    /// Windows executable with embedded game data.
    Exe,
    /// Standalone game data (.ccn / .gam).
    Ccn,
}

#[derive(Debug, Clone)]
pub struct PackFile {
    pub name: String,
    pub size: u32,
}

#[derive(Debug, Clone)]
pub struct Container {
    pub kind: ContainerKind,
    /// Offset of the "PAME"/"PAMU" game header.
    pub data_offset: usize,
    /// Files bundled before the game data (runtime DLLs, extensions, ...).
    pub pack_files: Vec<PackFile>,
}

pub fn is_game_header(data: &[u8], off: usize) -> bool {
    let Some(h) = data.get(off..off + 18) else {
        return false;
    };
    (&h[..4] == b"PAME" || &h[..4] == b"PAMU")
        && u16::from_le_bytes([h[16], h[17]]) == CHUNK_APP_HEADER
}

fn find_game_header(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 4 <= data.len() {
        let rel = data[i..].windows(3).position(|w| w == b"PAM")?;
        let off = i + rel;
        if is_game_header(data, off) {
            return Some(off);
        }
        i = off + 1;
    }
    None
}

/// End of the last PE section on disk, i.e. where appended (overlay) data begins.
fn pe_overlay_offset(data: &[u8]) -> Result<usize> {
    let mut r = ByteReader::new(data);
    r.seek(0x3C)?;
    let pe = r.read_u32()? as usize;
    r.seek(pe)?;
    if r.read_bytes(4)? != b"PE\0\0" {
        bail!("invalid PE signature");
    }
    r.seek(pe + 6)?;
    let sections = r.read_u16()? as usize;
    r.seek(pe + 20)?;
    let opt_size = r.read_u16()? as usize;
    let table = pe + 24 + opt_size;
    let mut end = 0usize;
    for i in 0..sections {
        r.seek(table + 40 * i + 16)?;
        let raw_size = r.read_u32()? as usize;
        let raw_ptr = r.read_u32()? as usize;
        end = end.max(raw_ptr + raw_size);
    }
    Ok(end)
}

fn parse_pack(data: &[u8], off: usize, unicode: bool) -> Result<(Vec<PackFile>, usize)> {
    let mut r = ByteReader::new(data);
    r.seek(off + 8)?;
    let header_size = r.read_u32()? as usize;
    let _data_size = r.read_u32()?;
    let _format = r.read_u32()?;
    r.skip(8)?;
    let count = r.read_u32()?;
    if count > 10_000 {
        bail!("implausible pack file count {count}");
    }
    r.seek(off + header_size)?;
    let mut files = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name_len = r.read_u16()? as usize;
        let name = decode_cstr(
            r.read_bytes(if unicode { name_len * 2 } else { name_len })?,
            unicode,
        );
        let _bingo = r.read_u32()?;
        let size = r.read_u32()?;
        r.skip(size as usize)?;
        if name.is_empty() || name.chars().any(|c| c.is_control()) {
            bail!("garbled pack file name");
        }
        files.push(PackFile { name, size });
    }
    Ok((files, r.pos()))
}

/// Quick check: does this file look like a Fusion game? Only looks where the data would
/// start (right after the executable's code, or the first megabyte of a data file), so
/// large files of other engines are rejected fast.
pub fn looks_like_fusion(data: &[u8]) -> bool {
    const WINDOW: usize = 1 << 20;
    let near = |from: usize| {
        let end = (from + WINDOW).min(data.len());
        from < end && find_game_header(&data[..end], from).is_some()
    };
    if !data.starts_with(b"MZ") {
        return near(0);
    }
    let Ok(overlay) = pe_overlay_offset(data) else { return false };
    if near(overlay) {
        return true;
    }
    // A "wwww" pack of runtime files can come first.
    data.get(overlay..overlay + 4) == Some(b"wwww")
        && [false, true]
            .into_iter()
            .find_map(|unicode| parse_pack(data, overlay, unicode).ok())
            .is_some_and(|(_, end)| near(end))
}

pub fn locate_game_data(data: &[u8]) -> Result<Container> {
    if is_game_header(data, 0) {
        return Ok(Container {
            kind: ContainerKind::Ccn,
            data_offset: 0,
            pack_files: vec![],
        });
    }
    if !data.starts_with(b"MZ") {
        let off = find_game_header(data, 0)
            .context("not a Windows executable and no Fusion game header (PAME/PAMU) found")?;
        return Ok(Container {
            kind: ContainerKind::Ccn,
            data_offset: off,
            pack_files: vec![],
        });
    }

    let overlay = pe_overlay_offset(data).context("parsing PE headers")?;
    let mut pack_files = Vec::new();
    let mut search_from = overlay;
    if data.get(overlay..overlay + 4) == Some(b"wwww") {
        if let Some((files, end)) = [false, true]
            .into_iter()
            .find_map(|unicode| parse_pack(data, overlay, unicode).ok())
        {
            pack_files = files;
            search_from = end;
        }
    }
    let data_offset = find_game_header(data, search_from)
        .or_else(|| find_game_header(data, overlay.min(search_from)))
        .context("executable contains no Fusion game data (PAME/PAMU header not found)")?;
    Ok(Container {
        kind: ContainerKind::Exe,
        data_offset,
        pack_files,
    })
}
