//! Logical sound state: which samples play on which channels and until when.
//!
//! The runtime decides *what* plays and answers "is playing" conditions from this model;
//! a backend (the mixer in `render::audio_out`) turns the queued [`AudioCommand`]s into real sound.

use std::collections::HashMap;

pub const CHANNELS: usize = 48;

#[derive(Debug, Clone, PartialEq)]
pub enum AudioCommand {
    /// Start `handle` at `start_ms`. `looped` repeats until stopped; `pan` is -1 (left) .. 1 (right).
    Play { handle: u32, looped: bool, volume: f32, pan: f32, start_ms: f64 },
    Stop { handle: u32 },
    StopAll,
    SetVolume { handle: u32, volume: f32 },
}

#[derive(Debug, Clone)]
struct Playing {
    handle: u32,
    channel: Option<usize>,
    started_ms: f64,
    /// 0 = forever.
    loops: u32,
    paused: bool,
    /// Playback position when paused.
    paused_at: f64,
}

#[derive(Debug, Clone)]
pub struct AudioState {
    pub commands: Vec<AudioCommand>,
    playing: Vec<Playing>,
    /// Known sample lengths in ms (exact from the backend, or estimated).
    pub durations: HashMap<u32, f64>,
    pub main_volume: f64,
    pub channel_volume: [f64; CHANNELS + 1],
    pub sample_volume: HashMap<u32, f64>,
    /// Pans in -100 (left) .. 100 (right).
    pub main_pan: f64,
    pub channel_pan: [f64; CHANNELS + 1],
    pub sample_pan: HashMap<u32, f64>,
    /// Audio clock in ms; keeps running across frames.
    pub clock_ms: f64,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            playing: Vec::new(),
            durations: HashMap::new(),
            main_volume: 100.0,
            channel_volume: [100.0; CHANNELS + 1],
            sample_volume: HashMap::new(),
            main_pan: 0.0,
            channel_pan: [0.0; CHANNELS + 1],
            sample_pan: HashMap::new(),
            clock_ms: 0.0,
        }
    }
}

impl AudioState {
    fn ends_at(&self, p: &Playing) -> Option<f64> {
        if p.loops == 0 {
            return None;
        }
        let d = self.durations.get(&p.handle).copied().unwrap_or(0.0);
        Some(p.started_ms + d * p.loops as f64)
    }

    fn effective_volume(&self, handle: u32, channel: Option<usize>) -> f32 {
        let s = self.sample_volume.get(&handle).copied().unwrap_or(100.0);
        let c = channel.map_or(100.0, |c| self.channel_volume[c.min(CHANNELS)]);
        ((self.main_volume / 100.0) * (s / 100.0) * (c / 100.0)).clamp(0.0, 1.0) as f32
    }

    fn effective_pan(&self, handle: u32, channel: Option<usize>) -> f32 {
        let s = self.sample_pan.get(&handle).copied().unwrap_or(0.0);
        let c = channel.map_or(0.0, |c| self.channel_pan[c.min(CHANNELS)]);
        ((self.main_pan + s + c) / 100.0).clamp(-1.0, 1.0) as f32
    }

    fn play_command(&self, p: &Playing, start_ms: f64) -> AudioCommand {
        AudioCommand::Play {
            handle: p.handle,
            looped: p.loops != 1,
            volume: self.effective_volume(p.handle, p.channel),
            pan: self.effective_pan(p.handle, p.channel),
            start_ms,
        }
    }

    /// Drop finished sounds; called once per tick after the clock advanced.
    pub fn update(&mut self, dt_ms: f64) {
        self.clock_ms += dt_ms;
        let now = self.clock_ms;
        let mut finished = Vec::new();
        let durations = &self.durations;
        self.playing.retain(|p| {
            let done = !p.paused && p.loops > 0 && {
                let d = durations.get(&p.handle).copied().unwrap_or(0.0);
                now >= p.started_ms + d * p.loops as f64
            };
            if done {
                finished.push(p.handle);
            }
            !done
        });
        for h in finished {
            // Finite multi-loop sounds were started looped; stop them explicitly.
            if !self.playing.iter().any(|p| p.handle == h) {
                self.commands.push(AudioCommand::Stop { handle: h });
            }
        }
    }

    /// Play a sample, optionally on a 1-based channel. `loops` 0 = forever.
    pub fn play(&mut self, handle: u32, channel: Option<usize>, loops: u32) {
        if let Some(c) = channel {
            self.stop_channel(c);
        }
        // Restarting a sample replaces its previous playback.
        self.stop_sample(handle);
        let p = Playing { handle, channel, started_ms: self.clock_ms, loops, paused: false, paused_at: 0.0 };
        let cmd = self.play_command(&p, 0.0);
        self.playing.push(p);
        self.commands.push(cmd);
    }

    pub fn stop_sample(&mut self, handle: u32) {
        if self.playing.iter().any(|p| p.handle == handle) {
            self.playing.retain(|p| p.handle != handle);
            self.commands.push(AudioCommand::Stop { handle });
        }
    }

    pub fn stop_channel(&mut self, channel: usize) {
        let handles: Vec<u32> =
            self.playing.iter().filter(|p| p.channel == Some(channel)).map(|p| p.handle).collect();
        for h in handles {
            self.stop_sample(h);
        }
    }

    pub fn stop_all(&mut self) {
        self.playing.clear();
        self.commands.push(AudioCommand::StopAll);
    }

    /// Pausing stops the sound and remembers its position; resuming continues from there.
    pub fn set_paused(&mut self, filter: impl Fn(u32, Option<usize>) -> bool, paused: bool) {
        let now = self.clock_ms;
        let mut cmds = Vec::new();
        for i in 0..self.playing.len() {
            let p = &self.playing[i];
            if p.paused == paused || !filter(p.handle, p.channel) {
                continue;
            }
            if paused {
                let at = now - p.started_ms;
                cmds.push(AudioCommand::Stop { handle: p.handle });
                let p = &mut self.playing[i];
                p.paused = true;
                p.paused_at = at;
            } else {
                let at = p.paused_at;
                cmds.push(self.play_command(p, at));
                let p = &mut self.playing[i];
                p.paused = false;
                p.started_ms = now - at;
            }
        }
        self.commands.extend(cmds);
    }

    /// Restart affected sounds at their current position so new pans take effect.
    pub fn refresh_pans(&mut self, filter: impl Fn(u32, Option<usize>) -> bool) {
        let now = self.clock_ms;
        let cmds: Vec<AudioCommand> = self
            .playing
            .iter()
            .filter(|p| !p.paused && filter(p.handle, p.channel))
            .map(|p| self.play_command(p, now - p.started_ms))
            .collect();
        self.commands.extend(cmds);
    }

    /// Jump playing sounds to `ms`.
    pub fn set_position(&mut self, filter: impl Fn(u32, Option<usize>) -> bool, ms: f64) {
        let now = self.clock_ms;
        let mut cmds = Vec::new();
        for i in 0..self.playing.len() {
            let p = &self.playing[i];
            if p.paused || !filter(p.handle, p.channel) {
                continue;
            }
            cmds.push(self.play_command(p, ms));
            self.playing[i].started_ms = now - ms;
        }
        self.commands.extend(cmds);
    }

    pub fn is_sample_playing(&self, handle: u32) -> bool {
        self.playing
            .iter()
            .any(|p| p.handle == handle && !p.paused && self.ends_at(p).is_none_or(|e| self.clock_ms < e))
    }

    pub fn is_sample_paused(&self, handle: u32) -> bool {
        self.playing.iter().any(|p| p.handle == handle && p.paused)
    }

    pub fn is_channel_playing(&self, channel: usize) -> bool {
        self.playing.iter().any(|p| p.channel == Some(channel) && !p.paused)
    }

    pub fn is_channel_paused(&self, channel: usize) -> bool {
        self.playing.iter().any(|p| p.channel == Some(channel) && p.paused)
    }

    pub fn any_playing(&self) -> bool {
        self.playing.iter().any(|p| !p.paused)
    }

    pub fn channel_sample(&self, channel: usize) -> Option<u32> {
        self.playing.iter().find(|p| p.channel == Some(channel)).map(|p| p.handle)
    }

    pub fn position_ms(&self, handle: u32) -> f64 {
        self.playing.iter().find(|p| p.handle == handle).map_or(0.0, |p| {
            if p.paused {
                p.paused_at
            } else {
                self.clock_ms - p.started_ms
            }
        })
    }

    /// Re-send volumes after a volume change.
    pub fn refresh_volumes(&mut self) {
        let cmds: Vec<AudioCommand> = self
            .playing
            .iter()
            .map(|p| AudioCommand::SetVolume { handle: p.handle, volume: self.effective_volume(p.handle, p.channel) })
            .collect();
        self.commands.extend(cmds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_sample_finishes_after_its_duration() {
        let mut a = AudioState::default();
        a.durations.insert(1, 100.0);
        a.play(1, None, 1);
        assert!(a.is_sample_playing(1));
        a.update(50.0);
        assert!(a.is_sample_playing(1));
        a.update(60.0);
        assert!(!a.is_sample_playing(1));
    }

    #[test]
    fn looping_sample_keeps_playing_until_stopped() {
        let mut a = AudioState::default();
        a.durations.insert(1, 100.0);
        a.play(1, None, 0);
        a.update(10_000.0);
        assert!(a.is_sample_playing(1));
        a.stop_sample(1);
        assert!(!a.is_sample_playing(1));
    }

    #[test]
    fn channel_play_replaces_previous_sample() {
        let mut a = AudioState::default();
        a.durations.insert(1, 1000.0);
        a.durations.insert(2, 1000.0);
        a.play(1, Some(3), 1);
        a.play(2, Some(3), 1);
        assert!(!a.is_sample_playing(1));
        assert!(a.is_sample_playing(2));
        assert_eq!(a.channel_sample(3), Some(2));
    }

    #[test]
    fn resume_continues_where_it_paused() {
        let mut a = AudioState::default();
        a.durations.insert(1, 10_000.0);
        a.play(1, Some(2), 1);
        a.update(1_500.0);
        a.set_paused(|_, c| c == Some(2), true);
        a.update(5_000.0);
        a.commands.clear();
        a.set_paused(|_, c| c == Some(2), false);
        match a.commands.last() {
            Some(AudioCommand::Play { start_ms, .. }) => assert!((start_ms - 1_500.0).abs() < 1.0),
            other => panic!("expected a play command, got {other:?}"),
        }
        assert!((a.position_ms(1) - 1_500.0).abs() < 1.0);
    }

    #[test]
    fn channel_pan_reaches_play_command() {
        let mut a = AudioState::default();
        a.durations.insert(1, 1000.0);
        a.channel_pan[4] = -100.0;
        a.play(1, Some(4), 1);
        match a.commands.last() {
            Some(AudioCommand::Play { pan, .. }) => assert_eq!(*pan, -1.0),
            other => panic!("expected a play command, got {other:?}"),
        }
    }
}
