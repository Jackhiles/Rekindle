//! Audio output: executes the runtime's queued audio commands on a small mixer.
//!
//! Nothing is decoded ahead of time. Each playing sound has a decoder thread that streams
//! it (resampled to the device rate) a fraction of a second ahead of the mixer, so even
//! long music costs only its compressed bytes plus a short buffer. Start offsets, pan and
//! volume are applied while streaming / mixing.

use crate::audio::{Resampler, Stream};
use crate::runtime::audio::AudioCommand;
use crate::runtime::Runtime;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::HashSet;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};

/// Decoded blocks buffered per sound (a block is ~1k frames, so this is ~0.3 s).
const AHEAD: usize = 12;

/// A sound being mixed.
struct Voice {
    handle: u32,
    rx: Receiver<Vec<f32>>,
    /// Current block (interleaved stereo) and the read position in it.
    buf: Vec<f32>,
    pos: usize,
    volume: f32,
    pan: f32,
}

#[derive(Default)]
struct Mix {
    voices: Vec<Voice>,
    /// The mixed stereo frames of the current callback.
    scratch: Vec<f32>,
}

impl Mix {
    /// Mix all voices into `scratch` (interleaved stereo); drops voices that ended.
    fn render(&mut self, frames: usize) {
        self.scratch.clear();
        self.scratch.resize(frames * 2, 0.0);
        let out = &mut self.scratch;
        self.voices.retain_mut(|v| {
            let (gl, gr) = (v.volume * (1.0 - v.pan.max(0.0)), v.volume * (1.0 + v.pan.min(0.0)));
            let mut i = 0;
            while i < frames {
                if v.pos >= v.buf.len() {
                    match v.rx.try_recv() {
                        Ok(b) => {
                            v.buf = b;
                            v.pos = 0;
                            continue;
                        }
                        Err(TryRecvError::Empty) => return true, // decoder is behind; catch up next time
                        Err(TryRecvError::Disconnected) => return false, // finished
                    }
                }
                let n = ((v.buf.len() - v.pos) / 2).min(frames - i);
                for k in 0..n {
                    out[(i + k) * 2] += v.buf[v.pos + k * 2] * gl;
                    out[(i + k) * 2 + 1] += v.buf[v.pos + k * 2 + 1] * gr;
                }
                v.pos += n * 2;
                i += n;
            }
            true
        });
    }
}

/// What decoder threads report back.
enum Report {
    /// Exact length of a sound in ms.
    Duration(u32, f64),
    Failed(u32, String),
}

pub struct AudioOut {
    mix: Arc<Mutex<Mix>>,
    /// The device stream (None when muted or no output device is available).
    stream: Option<cpal::Stream>,
    rate: u32,
    tx: Sender<Report>,
    rx: Receiver<Report>,
    /// Sounds that could not be decoded (not retried).
    failed: HashSet<u32>,
    pub enabled: bool,
    /// Replacement sounds from mods.
    pub mods: Arc<crate::mods::Mods>,
}

fn open_device(mix: Arc<Mutex<Mix>>) -> anyhow::Result<(cpal::Stream, u32)> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no audio output device"))?;
    let supported = device.default_output_config()?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let rate = config.sample_rate.0;
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, mix)?,
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, mix)?,
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, mix)?,
        other => anyhow::bail!("unsupported output sample format {other:?}"),
    };
    stream.play()?;
    Ok((stream, rate))
}

fn build<T>(device: &cpal::Device, config: &cpal::StreamConfig, mix: Arc<Mutex<Mix>>) -> anyhow::Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let ch = config.channels.max(1) as usize;
    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = data.len() / ch;
            let Ok(mut m) = mix.lock() else { return };
            m.render(frames);
            for (f, out) in data.chunks_exact_mut(ch).enumerate() {
                let (l, r) = (m.scratch[f * 2].clamp(-1.0, 1.0), m.scratch[f * 2 + 1].clamp(-1.0, 1.0));
                if ch == 1 {
                    out[0] = T::from_sample((l + r) * 0.5);
                } else {
                    out[0] = T::from_sample(l);
                    out[1] = T::from_sample(r);
                    for o in &mut out[2..] {
                        *o = T::from_sample(0.0f32);
                    }
                }
            }
        },
        |e| eprintln!("audio output: {e}"),
        None,
    )?)
}

/// Decoder thread body: stream `bytes` into `out` from `start_ms`, repeating if `looped`.
fn feed(handle: u32, bytes: Arc<[u8]>, rate: u32, start_ms: f64, looped: bool, out: SyncSender<Vec<f32>>, report: Sender<Report>) {
    let mut resampler = Resampler::new(rate);
    let mut skip = (start_ms.max(0.0) * rate as f64 / 1000.0) as usize * 2;
    let mut first = true;
    loop {
        let mut stream = match Stream::open(bytes.clone()) {
            Ok(s) => s,
            Err(e) => {
                let _ = report.send(Report::Failed(handle, format!("{e:#}")));
                return;
            }
        };
        if first {
            if let Some(ms) = stream.duration_ms() {
                let _ = report.send(Report::Duration(handle, ms));
            }
        }
        let (mut src_frames, mut src_rate, mut any) = (0u64, 0u32, false);
        loop {
            let (r, ch, samples) = match stream.next_block() {
                Ok(Some(b)) => b,
                Ok(None) => break,
                Err(e) => {
                    if !any {
                        let _ = report.send(Report::Failed(handle, format!("{e:#}")));
                        return;
                    }
                    break;
                }
            };
            any = true;
            let ch = ch.max(1);
            src_rate = r;
            src_frames += (samples.len() / ch) as u64;
            let mut buf = Vec::with_capacity((samples.len() / ch) * 2 * rate as usize / r.max(1) as usize + 4);
            resampler.push(r, ch, &samples, &mut buf);
            if skip > 0 {
                let n = skip.min(buf.len());
                buf.drain(..n);
                skip -= n;
            }
            if !buf.is_empty() && out.send(buf).is_err() {
                return; // stopped
            }
        }
        if !any {
            let _ = report.send(Report::Failed(handle, "no audio decoded".into()));
            return;
        }
        if first && src_rate > 0 {
            let _ = report.send(Report::Duration(handle, src_frames as f64 / src_rate as f64 * 1000.0));
        }
        first = false;
        if !looped {
            return;
        }
    }
}

impl AudioOut {
    pub fn new(enabled: bool) -> Self {
        let (tx, rx) = channel();
        let mix = Arc::new(Mutex::new(Mix::default()));
        let (stream, rate) = if enabled {
            match open_device(mix.clone()) {
                Ok((s, r)) => (Some(s), r),
                Err(e) => {
                    eprintln!("audio disabled: {e:#}");
                    (None, crate::audio::OUTPUT_RATE)
                }
            }
        } else {
            (None, crate::audio::OUTPUT_RATE)
        };
        Self { mix, enabled: stream.is_some(), stream, rate, tx, rx, failed: HashSet::new(), mods: Default::default() }
    }

    fn with_mix(&self, f: impl FnOnce(&mut Mix)) {
        if let Ok(mut m) = self.mix.lock() {
            f(&mut m);
        }
    }

    fn play(&mut self, rt: &Runtime, handle: u32, looped: bool, volume: f32, pan: f32, start_ms: f64) {
        self.with_mix(|m| m.voices.retain(|v| v.handle != handle));
        if self.failed.contains(&handle) {
            return;
        }
        let Some(entry) = rt.game.sounds.get(handle).cloned() else { return };
        let replacement = self.mods.sound(handle);
        let (out, rx) = sync_channel(AHEAD);
        self.with_mix(|m| m.voices.push(Voice { handle, rx, buf: Vec::new(), pos: 0, volume, pan }));
        let (report, rate) = (self.tx.clone(), self.rate);
        std::thread::spawn(move || {
            let bytes = match replacement.map(Ok).unwrap_or_else(|| entry.data()) {
                Ok(b) => Arc::<[u8]>::from(b),
                Err(e) => {
                    let _ = report.send(Report::Failed(handle, format!("{e:#}")));
                    return;
                }
            };
            feed(handle, bytes, rate, start_ms, looped, out, report);
        });
    }

    /// Run queued commands and pick up decoder reports. Call once per rendered frame.
    pub async fn process(&mut self, rt: &mut Runtime) {
        let commands = std::mem::take(&mut rt.audio.commands);
        if !self.enabled || self.stream.is_none() {
            return;
        }
        for cmd in commands {
            match cmd {
                AudioCommand::Play { handle, looped, volume, pan, start_ms } => self.play(rt, handle, looped, volume, pan, start_ms),
                AudioCommand::Stop { handle } => self.with_mix(|m| m.voices.retain(|v| v.handle != handle)),
                AudioCommand::StopAll => self.with_mix(|m| m.voices.clear()),
                AudioCommand::SetVolume { handle, volume } => self.with_mix(|m| {
                    for v in m.voices.iter_mut().filter(|v| v.handle == handle) {
                        v.volume = volume;
                    }
                }),
            }
        }
        while let Ok(r) = self.rx.try_recv() {
            match r {
                Report::Duration(handle, ms) => {
                    rt.audio.durations.insert(handle, ms);
                }
                Report::Failed(handle, e) => {
                    if self.failed.insert(handle) {
                        eprintln!("sound {handle}: {e}");
                    }
                }
            }
        }
    }
}
