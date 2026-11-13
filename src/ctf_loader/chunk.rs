//! Generic chunk container format and chunk body decoding (zlib + optional encryption).
//!
//! Every chunk is `id: u16, flags: u16, size: u32, body[size]`. A chunk list ends
//! with id 0x7F7F. Flags: bit 0 = zlib-compressed, bit 1 = encrypted.

use super::reader::{Blob, ByteReader};
use anyhow::{bail, Context, Result};
use flate2::read::ZlibDecoder;
use std::io::Read;

pub mod id {
    pub const APP_HEADER: u16 = 0x2223;
    pub const APP_NAME: u16 = 0x2224;
    pub const APP_AUTHOR: u16 = 0x2225;
    pub const MENU: u16 = 0x2226;
    pub const EXTRA_PATH: u16 = 0x2227;
    pub const FRAME_ITEMS: u16 = 0x2229;
    pub const GLOBAL_EVENTS: u16 = 0x222A;
    pub const FRAME_HANDLES: u16 = 0x222B;
    pub const EXTRA_DATA: u16 = 0x222C;
    pub const EDITOR_FILENAME: u16 = 0x222E;
    pub const TARGET_FILENAME: u16 = 0x222F;
    pub const APP_DOC: u16 = 0x2230;
    pub const OTHER_EXTENSIONS: u16 = 0x2231;
    pub const GLOBAL_VALUES: u16 = 0x2232;
    pub const GLOBAL_STRINGS: u16 = 0x2233;
    pub const EXTENSIONS: u16 = 0x2234;
    pub const APP_ICON: u16 = 0x2235;
    pub const SEC_NUM: u16 = 0x2237;
    pub const BINARY_FILES: u16 = 0x2238;
    pub const ABOUT_TEXT: u16 = 0x223A;
    pub const COPYRIGHT: u16 = 0x223B;
    pub const GLOBAL_VALUE_NAMES: u16 = 0x223C;
    pub const GLOBAL_STRING_NAMES: u16 = 0x223D;
    pub const FRAME_ITEMS_2: u16 = 0x223F;
    pub const EXE_ONLY: u16 = 0x2240;
    pub const PROTECTION: u16 = 0x2242;
    pub const SHADERS: u16 = 0x2243;
    pub const EXTENDED_HEADER: u16 = 0x2245;

    pub const FRAME: u16 = 0x3333;
    pub const FRAME_HEADER: u16 = 0x3334;
    pub const FRAME_NAME: u16 = 0x3335;
    pub const FRAME_PASSWORD: u16 = 0x3336;
    pub const FRAME_PALETTE: u16 = 0x3337;
    pub const FRAME_ITEM_INSTANCES: u16 = 0x3338;
    pub const FRAME_FADE_IN: u16 = 0x333B;
    pub const FRAME_FADE_OUT: u16 = 0x333C;
    pub const FRAME_EVENTS: u16 = 0x333D;
    pub const FRAME_LAYERS: u16 = 0x3341;
    pub const FRAME_VIRTUAL_RECT: u16 = 0x3342;
    pub const FRAME_RANDOM_SEED: u16 = 0x3344;
    pub const FRAME_LAYER_EFFECTS: u16 = 0x3345;
    pub const FRAME_MOVE_TIMER: u16 = 0x3347;
    pub const FRAME_EFFECTS: u16 = 0x3349;

    pub const OBJECT_HEADER: u16 = 0x4444;
    pub const OBJECT_NAME: u16 = 0x4445;
    pub const OBJECT_PROPERTIES: u16 = 0x4446;
    pub const OBJECT_EFFECT: u16 = 0x4448;

    pub const IMAGE_HANDLES: u16 = 0x5555;
    pub const FONT_HANDLES: u16 = 0x5556;
    pub const SOUND_HANDLES: u16 = 0x5557;
    pub const MUSIC_HANDLES: u16 = 0x5558;
    pub const IMAGE_BANK: u16 = 0x6666;
    pub const FONT_BANK: u16 = 0x6667;
    pub const SOUND_BANK: u16 = 0x6668;
    pub const MUSIC_BANK: u16 = 0x6669;
    pub const LAST: u16 = 0x7F7F;
}

pub fn chunk_name(cid: u16) -> &'static str {
    match cid {
        0x2223 => "AppHeader",
        0x2224 => "AppName",
        0x2225 => "AppAuthor",
        0x2226 => "AppMenu",
        0x2227 => "ExtraPath",
        0x2228 => "Extensions(old)",
        0x2229 => "FrameItems",
        0x222A => "GlobalEvents",
        0x222B => "FrameHandles",
        0x222C => "ExtraData",
        0x222D => "AdditionalExtension",
        0x222E => "EditorFilename",
        0x222F => "TargetFilename",
        0x2230 => "AppDoc",
        0x2231 => "OtherExtensions",
        0x2232 => "GlobalValues",
        0x2233 => "GlobalStrings",
        0x2234 => "Extensions",
        0x2235 => "AppIcon",
        0x2236 => "DemoVersion",
        0x2237 => "SecNum",
        0x2238 => "BinaryFiles",
        0x2239 => "AppMenuImages",
        0x223A => "AboutText",
        0x223B => "Copyright",
        0x223C => "GlobalValueNames",
        0x223D => "GlobalStringNames",
        0x223E => "MovementExtensions",
        0x223F => "FrameItems2",
        0x2240 => "ExeOnly",
        0x2242 => "Protection",
        0x2243 => "Shaders",
        0x2245 => "ExtendedHeader",
        0x2246 => "Spacer",
        0x224D => "FrameOffsets",
        0x224F => "AdditionalFrameInfo",
        0x2251 => "TitleText2",
        0x2253 => "ObjectProperties2",
        0x2254 => "GlobalEventInfo",
        0x2256 => "ObjectHeaders2",
        0x2257 => "ObjectNames2",
        0x2258 => "ObjectShaders",
        0x3333 => "Frame",
        0x3334 => "FrameHeader",
        0x3335 => "FrameName",
        0x3336 => "FramePassword",
        0x3337 => "FramePalette",
        0x3338 => "FrameItemInstances",
        0x3339 => "FrameFadeInFrame",
        0x333A => "FrameFadeOutFrame",
        0x333B => "FrameFadeIn",
        0x333C => "FrameFadeOut",
        0x333D => "FrameEvents",
        0x333E => "FramePlayHeader",
        0x333F => "FrameAdditionalItem",
        0x3340 => "FrameAdditionalItemInstance",
        0x3341 => "FrameLayers",
        0x3342 => "FrameVirtualRect",
        0x3343 => "DemoFilePath",
        0x3344 => "RandomSeed",
        0x3345 => "FrameLayerEffects",
        0x3346 => "BlurayOptions",
        0x3347 => "MovementTimerBase",
        0x3348 => "MosaicImageTable",
        0x3349 => "FrameEffects",
        0x334A => "FrameIphoneOptions",
        0x4444 => "ObjectHeader",
        0x4445 => "ObjectName",
        0x4446 => "ObjectProperties",
        0x4447 => "ObjectUnknown",
        0x4448 => "ObjectEffect",
        0x5555 => "ImageHandles",
        0x5556 => "FontHandles",
        0x5557 => "SoundHandles",
        0x5558 => "MusicHandles",
        0x6665 => "BankOffsets",
        0x6666 => "ImageBank",
        0x6667 => "FontBank",
        0x6668 => "SoundBank",
        0x6669 => "MusicBank",
        0x7F7F => "Last",
        _ => "?",
    }
}

#[derive(Debug, Clone)]
pub struct RawChunk {
    pub id: u16,
    pub flags: u16,
    pub body: Blob,
}

impl RawChunk {
    pub fn is_encrypted(&self) -> bool {
        self.flags & 2 != 0
    }
}

/// Read a chunk list starting at `pos`. Stops after the 0x7F7F terminator or at the
/// end of the buffer. Returns the chunks (terminator excluded) and the end position.
pub fn read_chunk_list(blob: &Blob, pos: usize) -> Result<(Vec<RawChunk>, usize)> {
    let mut r = ByteReader::new(blob);
    r.seek(pos)?;
    let mut out = Vec::new();
    while r.remaining() >= 8 {
        let start = r.pos();
        let id = r.read_u16()?;
        let flags = r.read_u16()?;
        let size = r.read_u32()? as usize;
        let body_start = r.pos();
        r.skip(size).with_context(|| {
            format!("chunk {id:#06x} at {start:#x} claims {size} bytes, past end of data")
        })?;
        if id == id::LAST {
            break;
        }
        out.push(RawChunk {
            id,
            flags,
            body: blob.slice(body_start..body_start + size)?,
        });
    }
    Ok((out, r.pos()))
}

/// Inflate a zlib stream, expecting roughly `size_hint` bytes of output.
pub fn inflate(data: &[u8], size_hint: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(size_hint.min(256 << 20));
    ZlibDecoder::new(data)
        .read_to_end(&mut out)
        .context("zlib inflate failed")?;
    Ok(out)
}

/// Inflate only the first `n` bytes of a zlib stream (cheap header peeks).
pub fn inflate_prefix(data: &[u8], n: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(n);
    ZlibDecoder::new(data)
        .take(n as u64)
        .read_to_end(&mut out)
        .context("zlib inflate failed")?;
    Ok(out)
}

/// `decompressed_size: u32, compressed_size: u32, zlib[compressed_size]`
fn inflate_block(b: &[u8]) -> Result<Vec<u8>> {
    let mut r = ByteReader::new(b);
    let decomp = r.read_u32()? as usize;
    let comp = r.read_u32()? as usize;
    let data = r.read_bytes(comp.min(r.remaining()))?;
    inflate(data, decomp)
}

/// RC4 stream cipher state used by Fusion's chunk "encryption".
#[derive(Clone)]
struct Rc4 {
    sbox: [u8; 256],
}

impl Rc4 {
    fn new(key: &[u8]) -> Option<Self> {
        if key.is_empty() {
            return None;
        }
        let mut s = [0u8; 256];
        for (i, v) in s.iter_mut().enumerate() {
            *v = i as u8;
        }
        let mut j = 0u8;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }
        Some(Self { sbox: s })
    }

    fn apply(&self, data: &mut [u8]) {
        let mut s = self.sbox;
        let (mut i, mut j) = (0u8, 0u8);
        for b in data {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[i as usize]);
            s.swap(i as usize, j as usize);
            *b ^= s[s[i as usize].wrapping_add(s[j as usize]) as usize];
        }
    }
}

/// Builds the key bytes from the app strings: for every UTF-16 unit, its non-zero
/// low byte and non-zero high byte, concatenated.
pub fn key_bytes(parts: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in parts {
        for u in s.encode_utf16() {
            let (lo, hi) = ((u & 0xFF) as u8, (u >> 8) as u8);
            if lo != 0 {
                out.push(lo);
            }
            if hi != 0 {
                out.push(hi);
            }
        }
    }
    out
}

/// Decodes chunk bodies according to their flags.
pub struct ChunkDecoder {
    rc4: Option<Rc4>,
    build: u32,
    /// Multimedia Fusion 1.5 data: different compression.
    old: bool,
}

impl ChunkDecoder {
    pub fn new(build: u32, key: Option<&[u8]>) -> Self {
        Self {
            rc4: key.and_then(Rc4::new),
            build,
            old: false,
        }
    }

    pub fn with_old(mut self, old: bool) -> Self {
        self.old = old;
        self
    }

    pub fn has_key(&self) -> bool {
        self.rc4.is_some()
    }

    fn decrypt(&self, cid: u16, data: &mut [u8]) -> Result<()> {
        let Some(rc4) = &self.rc4 else {
            bail!("chunk is encrypted but no decryption key is available")
        };
        // Newer runtimes additionally scramble the first byte of odd-numbered chunks.
        if cid & 1 == 1 && self.build > 284 {
            if let Some(b) = data.first_mut() {
                *b ^= (cid & 0xFF) as u8 ^ (cid >> 8) as u8;
            }
        }
        rc4.apply(data);
        Ok(())
    }

    pub fn decode(&self, chunk: &RawChunk) -> Result<Blob> {
        let r = self.decode_parts(chunk.id, chunk.flags, &chunk.body);
        // Debug: FUSION_RT_DUMP_CHUNKS=<dir> writes every decoded chunk as <id>_<n>.bin.
        if let (Some(dir), Ok(b)) = (std::env::var_os("FUSION_RT_DUMP_CHUNKS"), &r) {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(
                std::path::Path::new(&dir).join(format!("{:04x}_{n:05}_f{}.bin", chunk.id, chunk.flags)),
                &b[..],
            );
        }
        r.with_context(|| {
                format!(
                    "decoding chunk {:#06x} ({})",
                    chunk.id,
                    chunk_name(chunk.id)
                )
            })
    }

    fn decode_parts(&self, cid: u16, flags: u16, body: &Blob) -> Result<Blob> {
        match flags {
            0 => Ok(body.clone()),
            1 if self.old => Ok(Blob::new(super::old_inflate::inflate_block(body)?.0)),
            1 => Ok(Blob::new(inflate_block(body)?)),
            2 => {
                let mut v = body.to_vec();
                self.decrypt(cid, &mut v)?;
                Ok(Blob::new(v))
            }
            3 => {
                let mut r = ByteReader::new(body);
                let decomp = r.read_u32()? as usize;
                let mut rest = r.rest().to_vec();
                self.decrypt(cid, &mut rest)?;
                let mut r = ByteReader::new(&rest);
                let comp = r.read_u32()? as usize;
                let data = r.read_bytes(comp.min(r.remaining()))?;
                Ok(Blob::new(
                    inflate(data, decomp).context("bad decryption key?")?,
                ))
            }
            f => bail!("unknown chunk flags {f:#x}"),
        }
    }
}
