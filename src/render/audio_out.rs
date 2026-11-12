//! Macroquad audio backend: executes the runtime's queued audio commands.
//!
//! Sounds are decoded on worker threads the first time they're needed (decoding long
//! MP3 music can take a while) and played as soon as they are ready. The backend can
//! only play a sound from its start at a fixed pan, so positioned or panned playback
//! (resume after pause, "set position", stereo pan) plays a variant built from the
//! decoded samples.

use crate::audio;
use crate::runtime::audio::AudioCommand;
use crate::runtime::Runtime;
use macroquad::audio::{load_sound_from_bytes, play_sound, set_sound_volume, stop_sound, PlaySoundParams, Sound};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

enum Slot {
    Decoding,
    /// The plain sound, plus its 44.1 kHz stereo samples for variants.
    Ready(Sound, Arc<Vec<i16>>),
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

/// Decoded stereo samples and exact duration, or an error message.
type Decoded = (u32, Result<(Vec<i16>, f64), String>);

pub struct AudioOut {
    slots: HashMap<u32, Slot>,
    /// Sounds that should start once decoding finishes.
    pending: HashMap<u32, Start>,
    /// Positioned / panned variants currently playing (one per sound).
    variants: HashMap<u32, Sound>,
    tx: Sender<Decoded>,
    rx: Receiver<Decoded>,
    pub enabled: bool,
}

impl AudioOut {
    pub fn new(enabled: bool) -> Self {
        let (tx, rx) = channel();
        Self { slots: HashMap::new(), pending: HashMap::new(), variants: HashMap::new(), tx, rx, enabled }
    }

    fn start_decode(&mut self, rt: &Runtime, handle: u32) {
        let Some(entry) = rt.game.sounds.get(handle) else {
            self.slots.insert(handle, Slot::Failed);
            return;
        };
        let entry = entry.clone();
        let tx = self.tx.clone();
        self.slots.insert(handle, Slot::Decoding);
        std::thread::spawn(move || {
            let r = entry
                .data()
                .map_err(|e| format!("{e:#}"))
                .and_then(|d| audio::decode(&d).map_err(|e| format!("{e:#}")))
                .map(|pcm| (audio::to_stereo_44k(&pcm), pcm.duration_ms()));
            let _ = tx.send((handle, r));
        });
    }

    fn stop(&mut self, handle: u32) {
        if let Some(Slot::Ready(s, _)) = self.slots.get(&handle) {
            stop_sound(s);
        }
        if let Some(v) = self.variants.remove(&handle) {
            stop_sound(&v);
        }
    }

    async fn start(&mut self, handle: u32, st: Start) {
        self.stop(handle);
        let Some(Slot::Ready(sound, samples)) = self.slots.get(&handle) else { return };
        let params = PlaySoundParams { looped: st.looped, volume: st.volume };
        if st.start_ms <= 1.0 && st.pan.abs() < 0.01 {
            play_sound(sound, params);
            return;
        }
        // A one-off variant: from the requested position, with the pan baked in.
        let wav = audio::wav_from_stereo(&audio::stereo_variant(samples, st.start_ms, st.pan));
        match load_sound_from_bytes(&wav).await {
            Ok(v) => {
                play_sound(&v, params);
                self.variants.insert(handle, v);
            }
            Err(e) => eprintln!("sound {handle}: {e:?}"),
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
                    match self.slots.get(&handle) {
                        Some(Slot::Ready(..)) => self.start(handle, st).await,
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
                    self.stop(handle);
                }
                AudioCommand::StopAll => {
                    self.pending.clear();
                    let handles: Vec<u32> = self.slots.keys().copied().collect();
                    for h in handles {
                        self.stop(h);
                    }
                }
                AudioCommand::SetVolume { handle, volume } => {
                    if let Some(p) = self.pending.get_mut(&handle) {
                        p.volume = volume;
                    }
                    if let Some(Slot::Ready(s, _)) = self.slots.get(&handle) {
                        set_sound_volume(s, volume);
                    }
                    if let Some(v) = self.variants.get(&handle) {
                        set_sound_volume(v, volume);
                    }
                }
            }
        }

        while let Ok((handle, result)) = self.rx.try_recv() {
            let slot = match result {
                Ok((samples, ms)) => {
                    rt.audio.durations.insert(handle, ms);
                    match load_sound_from_bytes(&audio::wav_from_stereo(&samples)).await {
                        Ok(s) => Slot::Ready(s, Arc::new(samples)),
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
                // The sound has been decoding since the request: skip what already "played".
                self.start(handle, st).await;
            }
        }
    }
}
