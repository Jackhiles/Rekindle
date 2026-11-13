//! Rekindle library: a clean-room parser and runtime for Clickteam Fusion games.
//! Contains no game data; everything is loaded from user-supplied files at runtime.

/// Sound decoding (format-independent PCM / WAV conversion).
pub mod audio;
/// Format parser. Has no rendering dependencies.
pub mod ctf_loader;
/// The game library / launcher shown when no game is given.
pub mod launcher;
/// Replacement packs (mods) for images and sounds.
pub mod mods;
/// Log file next to the saves, and panic capture.
pub mod log;
/// Macroquad-based frame viewer.
pub mod render;
/// Object runtime and event interpreter (no rendering dependencies).
pub mod runtime;
