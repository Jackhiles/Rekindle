//! fusion-rt library: a clean-room parser and runtime for Clickteam Fusion 2.5 games.
//! Contains no game data; everything is loaded from user-supplied files at runtime.

/// Sound decoding (format-independent PCM / WAV conversion).
pub mod audio;
/// Format parser. Has no rendering dependencies.
pub mod ctf_loader;
/// Macroquad-based frame viewer.
pub mod render;
/// Object runtime and event interpreter (no rendering dependencies).
pub mod runtime;
