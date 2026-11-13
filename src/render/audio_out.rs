//! Macroquad audio backend: executes the runtime's queued audio commands.
//!
//! Sounds are decoded on worker threads the first time they're needed (decoding long
//! MP3 music can take a while) and played as soon as they are ready. The backend can
//! only play a sound from its start at a fixed pan, so positioned or panned playback
//! (resume after pause, "set position", stereo pan) plays a variant built from the
//! sound's samples. Those are decoded again on demand rather than kept in memory, since
//! variants are rare and music samples are large.

use crate::audio;
use crate::runtime::audio::AudioCommand;
use crate::runtime::Runtime;
use macroquad::audio::{load_sound_from_bytes, play_sound, set_sound_volume, stop_sound, PlaySoundParams, Sound};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

/// Decoded sounds kept in memory at most (roughly); the least recently played ones that
/// are not playing are dropped and decoded again if they are needed later.
const SOUND_BUDGET: usize = 160 << 20;

/// Bookkeeping for a decoded sound.
struct Use {
    /// Approximate memory the backend holds for it.
    bytes: usize,
    duration: Duration,
    /// When it was last started (or decoded).
    last: Instant,
    /// May still be audible: looping, or started less than its length ago.
    looping: bool,
}

impl Use {
    fn maybe_playing(&self, now: Instant) -> bool {
        self.looping || now.duration_since(self.last) < self.duration + Duration::from_millis(500)
    }
}

enum Slot {
    Decoding,
    Ready(Sound),
    Failed,
}

/// How to start a sound.
#[derive(Clone, Copy)]
struct Start {
    looped: bool,
    volume: f32,
    pan: f32,
    start_ms: f64,
}

impl Start {
    fn needs_variant(&self) -> bool {
        self.start_ms > 1.0 || self.pan.abs() >= 0.01
    }
}

/// What a worker thread produced.
enum Decoded {
    /// A WAV of the 44.1 kHz stereo samples, their count and the exact duration (first
    /// decode of a sound). Built on the worker so the samples aren't held twice here.
    Sound(u32, Result<(Vec<u8>, usize, f64), String>),
    /// A ready-to-load WAV for a positioned / panned start.
    Variant(u32, Start, Vec<u8>),
}

pub struct AudioOut {
    slots: HashMap<u32, Slot>,
    /// Sounds that should start once decoding finishes.
    pending: HashMap<u32, Start>,
    /// Variants being prepared (the latest request per sound wins).
    pending_variants: HashMap<u32, Start>,
    /// Positioned / panned variants currently playing (one per sound).
    variants: HashMap<u32, Sound>,
    uses: HashMap<u32, Use>,
    bytes: usize,
    tx: Sender<Decoded>,
    rx: Receiver<Decoded>,
    pub enabled: bool,
    /// Replacement sounds from mods.
    pub mods: std::sync::Arc<crate::mods::Mods>,
}

impl AudioOut {
    pub fn new(enabled: bool) -> Self {
        let (tx, rx) = channel();
        Self {
            slots: HashMap::new(),
            pending: HashMap::new(),
            pending_variants: HashMap::new(),
            variants: HashMap::new(),
            uses: HashMap::new(),
            bytes: 0,
            tx,
            rx,
            enabled,
            mods: Default::default(),
        }
    }

    fn decode_samples(&self, rt: &Runtime, handle: u32) -> Option<impl FnOnce() -> Result<(Vec<i16>, f64), String>> {
        let entry = rt.game.sounds.get(handle)?.clone();
        let replacement = self.mods.sound(handle);
        Some(move || {
            replacement
                .map(Ok)
                .unwrap_or_else(|| entry.data().map_err(|e| format!("{e:#}")))
                .and_then(|d| audio::decode(&d).map_err(|e| format!("{e:#}")))
                .map(|pcm| (audio::to_stereo_44k(&pcm), pcm.duration_ms()))
        })
    }

    fn start_decode(&mut self, rt: &Runtime, handle: u32) {
        let Some(job) = self.decode_samples(rt, handle) else {
            self.slots.insert(handle, Slot::Failed);
            return;
        };
        let tx = self.tx.clone();
        self.slots.insert(handle, Slot::Decoding);
        std::thread::spawn(move || {
            let r = job().map(|(samples, ms)| (audio::wav_from_stereo(&samples), samples.len(), ms));
            let _ = tx.send(Decoded::Sound(handle, r));
        });
    }

    fn request_variant(&mut self, rt: &Runtime, handle: u32, st: Start) {
        let first = self.pending_variants.insert(handle, st).is_none();
        if !first {
            return; // a worker is already on it; it will use the latest request
        }
        let Some(job) = self.decode_samples(rt, handle) else { return };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            if let Ok((samples, _)) = job() {
                let wav = audio::wav_from_stereo(&audio::stereo_variant(&samples, st.start_ms, st.pan));
                let _ = tx.send(Decoded::Variant(handle, st, wav));
            }
        });
    }

    fn stop(&mut self, handle: u32) {
        if let Some(Slot::Ready(s)) = self.slots.get(&handle) {
            stop_sound(s);
        }
        if let Some(u) = self.uses.get_mut(&handle) {
            u.looping = false;
            u.duration = Duration::ZERO;
        }
        if let Some(v) = self.variants.remove(&handle) {
            stop_sound(&v);
        }
    }

    fn start(&mut self, rt: &Runtime, handle: u32, st: Start) {
        self.stop(handle);
        if st.needs_variant() {
            self.request_variant(rt, handle, st);
            return;
        }
        if let Some(Slot::Ready(sound)) = self.slots.get(&handle) {
            play_sound(sound, PlaySoundParams { looped: st.looped, volume: st.volume });
            if let Some(u) = self.uses.get_mut(&handle) {
                u.last = Instant::now();
                u.looping = st.looped;
                u.duration = Duration::from_secs_f64(rt.audio.durations.get(&handle).copied().unwrap_or(0.0).max(0.0) / 1000.0);
            }
        }
    }

    /// Approximate memory held by decoded sounds.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Drop decoded sounds that aren't playing, least recently used first, while over budget.
    fn trim(&mut self) {
        if self.bytes <= SOUND_BUDGET {
            return;
        }
        let now = Instant::now();
        let mut idle: Vec<(Instant, u32)> = self
            .uses
            .iter()
            .filter(|(h, u)| !u.maybe_playing(now) && !self.pending.contains_key(h) && !self.variants.contains_key(h))
            .map(|(h, u)| (u.last, *h))
            .collect();
        idle.sort();
        for (_, h) in idle {
            if self.bytes <= SOUND_BUDGET * 3 / 4 {
                break;
            }
            if let Some(u) = self.uses.remove(&h) {
                self.bytes -= u.bytes;
                self.slots.remove(&h); // the Sound's drop frees the backend's buffer
            }
        }
    }

    /// Run queued commands and pick up finished decodes. Call once per rendered frame.
    pub async fn process(&mut self, rt: &mut Runtime) {
        let commands = std::mem::take(&mut rt.audio.commands);
        if !self.enabled {
            return;
        }
        for cmd in commands {
            match cmd {
                AudioCommand::Play { handle, looped, volume, pan, start_ms } => {
                    let st = Start { looped, volume, pan, start_ms };
                    self.pending_variants.remove(&handle);
                    match self.slots.get(&handle) {
                        Some(Slot::Ready(..)) => self.start(rt, handle, st),
                        Some(Slot::Failed) => {}
                        Some(Slot::Decoding) => {
                            self.pending.insert(handle, st);
                        }
                        None => {
                            self.pending.insert(handle, st);
                            self.start_decode(rt, handle);
                        }
                    }
                }
                AudioCommand::Stop { handle } => {
                    self.pending.remove(&handle);
                    self.pending_variants.remove(&handle);
                    self.stop(handle);
                }
                AudioCommand::StopAll => {
                    self.pending.clear();
                    self.pending_variants.clear();
                    let handles: Vec<u32> = self.slots.keys().copied().collect();
                    for h in handles {
                        self.stop(h);
                    }
                }
                AudioCommand::SetVolume { handle, volume } => {
                    if let Some(p) = self.pending.get_mut(&handle) {
                        p.volume = volume;
                    }
                    if let Some(p) = self.pending_variants.get_mut(&handle) {
                        p.volume = volume;
                    }
                    if let Some(Slot::Ready(s)) = self.slots.get(&handle) {
                        set_sound_volume(s, volume);
                    }
                    if let Some(v) = self.variants.get(&handle) {
                        set_sound_volume(v, volume);
                    }
                }
            }
        }

        while let Ok(done) = self.rx.try_recv() {
            match done {
                Decoded::Sound(handle, result) => {
                    let slot = match result {
                        Ok((wav, samples, ms)) => {
                            rt.audio.durations.insert(handle, ms);
                            // The backend keeps the samples as f32 in a vector grown by doubling.
                            let bytes = (samples * 4).next_power_of_two();
                            if let Some(old) = self.uses.insert(
                                handle,
                                Use { bytes, duration: Duration::ZERO, last: Instant::now(), looping: false },
                            ) {
                                self.bytes -= old.bytes;
                            }
                            self.bytes += bytes;
                            let loaded = load_sound_from_bytes(&wav).await;
                            drop(wav);
                            match loaded {
                                Ok(s) => Slot::Ready(s),
                                Err(e) => {
                                    eprintln!("sound {handle}: {e:?}");
                                    Slot::Failed
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("sound {handle}: {e}");
                            Slot::Failed
                        }
                    };
                    self.slots.insert(handle, slot);
                    if let Some(st) = self.pending.remove(&handle) {
                        self.start(rt, handle, st);
                    }
                }
                Decoded::Variant(handle, built_for, wav) => {
                    // Still wanted? (A newer request with a different start re-decodes.)
                    let Some(st) = self.pending_variants.remove(&handle) else { continue };
                    if (st.start_ms - built_for.start_ms).abs() > 50.0 || (st.pan - built_for.pan).abs() > 0.01 {
                        self.request_variant(rt, handle, st);
                        continue;
                    }
                    match load_sound_from_bytes(&wav).await {
                        Ok(v) => {
                            play_sound(&v, PlaySoundParams { looped: st.looped, volume: st.volume });
                            if let Some(old) = self.variants.insert(handle, v) {
                                stop_sound(&old);
                            }
                        }
                        Err(e) => eprintln!("sound {handle}: {e:?}"),
                    }
                }
            }
        }
        self.trim();
    }
}
