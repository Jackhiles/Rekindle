//! Clean-room loader for Clickteam Fusion 2.5 game data (.exe with embedded data, or .ccn).
//!
//! Entry point: [`GameData::load`]. Parsing is tolerant: problems in optional
//! sections are collected in [`GameData::warnings`] instead of aborting the load.

pub mod chunk;
pub mod events;
pub mod exe;
pub mod fonts;
pub mod frame;
pub mod icon;
pub mod header;
pub mod images;
pub mod objects;
pub mod old_inflate;
pub mod properties;
pub mod reader;
pub mod sounds;

use anyhow::{bail, Context, Result};
use chunk::{id, read_chunk_list, ChunkDecoder, RawChunk};
use reader::{decode_cstr, Blob, ByteReader};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use exe::{ContainerKind, PackFile};
pub use fonts::FontBank;
pub use frame::{Frame, Layer, ObjectInstance};
pub use header::AppHeader;
pub use images::{ImageBank, ImageEntry, RgbaImage};
pub use objects::{ExtensionInfo, Ink, ObjectInfo, ObjectKind};
pub use properties::ObjectProperties;
pub use sounds::{AudioFormat, SoundBank, SoundEntry};

#[derive(Debug, Clone)]
pub struct ChunkStat {
    pub id: u16,
    pub flags: u16,
    pub size: usize,
}

#[derive(Debug)]
pub struct GameData {
    pub path: PathBuf,
    pub container: ContainerKind,
    pub data_offset: usize,
    pub pack_files: Vec<PackFile>,

    /// "PAMU" header: strings are UTF-16LE.
    pub unicode: bool,
    pub runtime_version: u16,
    pub runtime_subversion: u16,
    pub product_version: u32,
    pub product_build: u32,
    /// Some chunks were encrypted and a key was derived to read them.
    pub encrypted: bool,
    /// Multimedia Fusion 1.5 data (older layouts and compression).
    pub old: bool,

    pub header: AppHeader,
    pub name: String,
    pub author: String,
    pub copyright: String,
    pub editor_filename: String,
    pub target_filename: String,
    pub about_text: String,

    pub extensions: Vec<ExtensionInfo>,
    pub objects: Vec<ObjectInfo>,
    object_index: HashMap<u16, usize>,
    pub frames: Vec<Frame>,
    /// Frame handle -> frame index table used by "jump to frame" actions.
    pub frame_handles: Vec<u16>,
    pub global_values: Vec<i32>,
    pub global_strings: Vec<String>,

    pub images: ImageBank,
    pub sounds: SoundBank,
    pub music: SoundBank,
    pub fonts: FontBank,

    pub chunks: Vec<ChunkStat>,
    pub warnings: Vec<String>,
    /// The executable's own icon (from its Windows resources), if it has one.
    pub icon: Option<icon::IconImage>,
}

impl GameData {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = Blob::map_file(path).with_context(|| format!("reading {}", path.display()))?;
        match Self::from_blob(file, path.to_path_buf()) {
            Ok(g) => Ok(g),
            Err(e) => {
                // "Separate data file" builds: a generic runtime .exe next to `<name>.dat`.
                let dat = path.with_extension("dat");
                if dat != path && dat.is_file() {
                    let file = Blob::map_file(&dat).with_context(|| format!("reading {}", dat.display()))?;
                    let mut g = Self::from_blob(file, dat.clone())?;
                    if g.icon.is_none() {
                        g.icon = std::fs::read(path).ok().and_then(|b| icon::from_pe(&b));
                    }
                    Ok(g)
                } else {
                    Err(e)
                }
            }
        }
    }

    pub fn object(&self, handle: u16) -> Option<&ObjectInfo> {
        self.object_index.get(&handle).map(|&i| &self.objects[i])
    }

    pub fn from_bytes(bytes: Vec<u8>, path: PathBuf) -> Result<Self> {
        Self::from_blob(Blob::new(bytes), path)
    }

    pub fn from_blob(file: Blob, path: PathBuf) -> Result<Self> {
        let container = exe::locate_game_data(&file)?;
        let base = container.data_offset;

        let mut r = ByteReader::new(&file);
        r.seek(base)?;
        let unicode = r.read_bytes(4)? == b"PAMU";
        let runtime_version = r.read_u16()?;
        let runtime_subversion = r.read_u16()?;
        let product_version = r.read_u32()?;
        let product_build = r.read_u32()?;

        let (chunks, _) =
            read_chunk_list(&file, base + 16).context("reading top-level chunk list")?;
        // Multimedia Fusion 1.5 (runtime 0x0301) uses older layouts and compression. Its
        // build field isn't meaningful, so none of the build-number rules apply.
        let old = runtime_version < 0x0302;
        let build = if old { 0 } else { product_build };
        let mut warnings = Vec::new();
        let find = |cid: u16| chunks.iter().find(|c| c.id == cid);

        // App strings are needed first: they seed the decryption key.
        let plain = ChunkDecoder::new(build, None).with_old(old);
        let string = |cid: u16, warnings: &mut Vec<String>| -> String {
            let Some(c) = find(cid) else {
                return String::new();
            };
            match plain.decode(c) {
                Ok(b) => decode_cstr(&b, unicode),
                Err(e) => {
                    warnings.push(format!("{e:#}"));
                    String::new()
                }
            }
        };
        let name = string(id::APP_NAME, &mut warnings);
        let author = string(id::APP_AUTHOR, &mut warnings);
        let copyright = string(id::COPYRIGHT, &mut warnings);
        let editor_filename = string(id::EDITOR_FILENAME, &mut warnings);
        let target_filename = string(id::TARGET_FILENAME, &mut warnings);
        let about_text = string(id::ABOUT_TEXT, &mut warnings);

        let key = if build > 284 {
            chunk::key_bytes(&[&name, &copyright, &editor_filename])
        } else {
            chunk::key_bytes(&[&editor_filename, &name, &copyright])
        };
        // The runtime uses at most 128 key bytes (long titles / paths are cut off).
        let mut key = key;
        key.truncate(128);
        let dec = ChunkDecoder::new(build, Some(&key)).with_old(old);
        let encrypted = chunks.iter().any(RawChunk::is_encrypted);

        let decode = |c: &RawChunk, warnings: &mut Vec<String>| match dec.decode(c) {
            Ok(b) => Some(b),
            Err(e) => {
                warnings.push(format!("{e:#}"));
                None
            }
        };
        let decode_id =
            |cid: u16, warnings: &mut Vec<String>| find(cid).and_then(|c| decode(c, warnings));

        let header = match decode_id(id::APP_HEADER, &mut warnings) {
            Some(b) if old => AppHeader::parse_old(&b).context("parsing application header")?,
            Some(b) => AppHeader::parse(&b).context("parsing application header")?,
            None => bail!("game data has no readable application header"),
        };

        let extensions = decode_id(id::EXTENSIONS, &mut warnings)
            .map(|b| {
                objects::parse_extensions(&b, unicode).unwrap_or_else(|e| {
                    warnings.push(format!("extension list: {e:#}"));
                    Vec::new()
                })
            })
            .unwrap_or_default();

        let objects = match decode_id(id::FRAME_ITEMS, &mut warnings) {
            Some(b) => objects::parse_frame_items(&b, unicode, build, old, &dec, &mut warnings)
                .unwrap_or_else(|e| {
                    warnings.push(format!("object definitions: {e:#}"));
                    Vec::new()
                }),
            None if find(id::OBJECT_HEADERS_2).is_some() => {
                let get = |cid: u16, w: &mut Vec<String>| decode_id(cid, w).map(|b| b.to_vec()).unwrap_or_default();
                let headers = get(id::OBJECT_HEADERS_2, &mut warnings);
                let names = get(id::OBJECT_NAMES_2, &mut warnings);
                let props = get(id::OBJECT_PROPERTIES_2, &mut warnings);
                let offsets = get(id::OBJECT_PROPERTY_OFFSETS_2, &mut warnings);
                objects::parse_split_objects(&headers, &names, &props, &offsets, unicode, build, &mut warnings)
                    .unwrap_or_else(|e| {
                        warnings.push(format!("object definitions: {e:#}"));
                        Vec::new()
                    })
            }
            None => {
                if find(id::FRAME_ITEMS_2).is_some() {
                    warnings.push("FrameItems2 (0x223F) object layout not supported yet".into());
                }
                Vec::new()
            }
        };
        let object_index = objects
            .iter()
            .enumerate()
            .map(|(i, o)| (o.handle, i))
            .collect();

        let mut frames = Vec::new();
        for c in chunks.iter().filter(|c| c.id == id::FRAME) {
            let index = frames.len();
            let Some(b) = decode(c, &mut warnings) else {
                continue;
            };
            match Frame::parse(&b, index, unicode, old, &dec, &mut warnings) {
                Ok(f) => frames.push(f),
                Err(e) => warnings.push(format!("frame #{index}: {e:#}")),
            }
        }

        let frame_handles = decode_id(id::FRAME_HANDLES, &mut warnings)
            .map(|b| {
                b.chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect()
            })
            .unwrap_or_default();

        let global_values = decode_id(id::GLOBAL_VALUES, &mut warnings)
            .and_then(|b| {
                let mut r = ByteReader::new(&b);
                let n = r.read_u16().ok()?;
                (0..n).map(|_| r.read_i32().ok()).collect()
            })
            .unwrap_or_default();
        let global_strings = decode_id(id::GLOBAL_STRINGS, &mut warnings)
            .and_then(|b| {
                let mut r = ByteReader::new(&b);
                let n = r.read_u32().ok()?;
                (0..n).map(|_| r.read_cstring(unicode).ok()).collect()
            })
            .unwrap_or_default();

        macro_rules! bank {
            ($cid:expr, $what:literal, $parse:expr) => {
                match decode_id($cid, &mut warnings) {
                    Some(b) => $parse(&b).unwrap_or_else(|e| {
                        warnings.push(format!(concat!($what, ": {:#}"), e));
                        Default::default()
                    }),
                    None => Default::default(),
                }
            };
        }
        let palette = frames.iter().find_map(|f| f.palette.clone());
        let images = bank!(id::IMAGE_BANK, "image bank", |b| ImageBank::parse_with(
            b, build, old, palette
        ));
        let sounds = bank!(id::SOUND_BANK, "sound bank", |b| if old {
            SoundBank::parse_old(b)
        } else {
            SoundBank::parse(b, unicode, build)
        });
        let music = bank!(id::MUSIC_BANK, "music bank", |b| if old {
            SoundBank::parse_old(b)
        } else {
            SoundBank::parse(b, unicode, build)
        });
        let fonts = bank!(id::FONT_BANK, "font bank", |b| if old {
            FontBank::parse_old(b)
        } else {
            FontBank::parse(b, unicode, build)
        });

        let chunk_stats = chunks
            .iter()
            .map(|c| ChunkStat {
                id: c.id,
                flags: c.flags,
                size: c.body.len(),
            })
            .collect();

        Ok(GameData {
            path,
            container: container.kind,
            data_offset: base,
            pack_files: container.pack_files,
            unicode,
            runtime_version,
            runtime_subversion,
            product_version,
            product_build,
            encrypted,
            old,
            header,
            name,
            author,
            copyright,
            editor_filename,
            target_filename,
            about_text,
            extensions,
            objects,
            object_index,
            frames,
            frame_handles,
            global_values,
            global_strings,
            images,
            sounds,
            music,
            fonts,
            chunks: chunk_stats,
            warnings,
            icon: icon::from_pe(&file),
        })
    }
}
