//! Sound decoding: anything in a sound bank (WAV PCM/ADPCM, OGG Vorbis, MP3), either a
//! block at a time ([`Stream`], for playback) or whole (`decode`, for tools and tests).

use anyhow::{anyhow, Context, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub const OUTPUT_RATE: u32 = 44_100;

/// Interleaved 16-bit PCM.
pub struct Pcm {
    pub rate: u32,
    pub channels: usize,
    pub samples: Vec<i16>,
}

impl Pcm {
    pub fn duration_ms(&self) -> f64 {
        if self.rate == 0 || self.channels == 0 {
            return 0.0;
        }
        self.samples.len() as f64 / self.channels as f64 / self.rate as f64 * 1000.0
    }
}

/// An audio file decoded a block at a time, so long music never has to be held as PCM.
pub struct Stream {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    duration_ms: Option<f64>,
}

impl Stream {
    /// Open an audio file of any supported format (WAV PCM/ADPCM, OGG Vorbis, MP3).
    pub fn open(bytes: std::sync::Arc<[u8]>) -> Result<Self> {
        let mut hint = Hint::new();
        match &bytes[..] {
            [b'R', b'I', b'F', b'F', ..] => hint.with_extension("wav"),
            [b'O', b'g', b'g', b'S', ..] => hint.with_extension("ogg"),
            _ => hint.with_extension("mp3"),
        };
        let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
            .context("unrecognised audio format")?;
        let format = probed.format;
        let track = format.default_track().ok_or_else(|| anyhow!("no audio track"))?;
        let params = &track.codec_params;
        let duration_ms = match (params.n_frames, params.sample_rate) {
            (Some(n), Some(r)) if r > 0 && n > 0 => Some(n as f64 / r as f64 * 1000.0),
            _ => None,
        };
        let decoder = symphonia::default::get_codecs()
            .make(params, &DecoderOptions::default())
            .context("unsupported codec")?;
        Ok(Self { track_id: track.id, format, decoder, duration_ms })
    }

    /// Exact length from the file's headers, when it states one.
    pub fn duration_ms(&self) -> Option<f64> {
        self.duration_ms
    }

    /// The next decoded block: (sample rate, channels, interleaved samples); None at the end.
    pub fn next_block(&mut self) -> Result<Option<(u32, usize, Vec<f32>)>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(_)) | Err(SymError::ResetRequired) => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buf) => {
                    let spec = *buf.spec();
                    let mut sb = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
                    sb.copy_interleaved_ref(buf);
                    return Ok(Some((spec.rate, spec.channels.count(), sb.samples().to_vec())));
                }
                Err(SymError::DecodeError(_)) => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// Linear resampling of a block stream to interleaved stereo at `out_rate`, keeping its
/// position across blocks (and across loops of a looping sound).
pub struct Resampler {
    out_rate: u32,
    pos: f64,
    prev: [f32; 2],
}

impl Resampler {
    pub fn new(out_rate: u32) -> Self {
        Self { out_rate, pos: 0.0, prev: [0.0; 2] }
    }

    /// Append the resampled stereo frames of one block to `out`.
    pub fn push(&mut self, rate: u32, channels: usize, block: &[f32], out: &mut Vec<f32>) {
        let ch = channels.max(1);
        let n = block.len() / ch;
        if n == 0 {
            return;
        }
        let frame = |i: usize| -> [f32; 2] { [block[i * ch], block[i * ch + (ch > 1) as usize]] };
        let step = rate.max(1) as f64 / self.out_rate.max(1) as f64;
        // Position -1 is the previous block's last frame.
        while self.pos + 1.0 < n as f64 {
            let i = self.pos.floor();
            let t = (self.pos - i) as f32;
            let a = if i < 0.0 { self.prev } else { frame(i as usize) };
            let b = frame((i + 1.0) as usize);
            out.push(a[0] + (b[0] - a[0]) * t);
            out.push(a[1] + (b[1] - a[1]) * t);
            self.pos += step;
        }
        self.pos -= n as f64;
        self.prev = frame(n - 1);
    }
}

/// Decode an audio file of any supported format.
pub fn decode(bytes: &[u8]) -> Result<Pcm> {
    let mut stream = Stream::open(bytes.into())?;
    let mut pcm = Pcm { rate: OUTPUT_RATE, channels: 0, samples: Vec::new() };
    while let Some((rate, channels, block)) = stream.next_block()? {
        pcm.rate = rate;
        pcm.channels = channels;
        pcm.samples.extend(block.iter().map(|v| (v * 32767.0).round().clamp(-32768.0, 32767.0) as i16));
    }
    if pcm.channels == 0 {
        return Err(anyhow!("no audio decoded"));
    }
    Ok(pcm)
}

/// Resample (linear) to 44.1 kHz stereo and wrap in a WAV container.
pub fn to_wav(pcm: &Pcm) -> Vec<u8> {
    wav_from_stereo(&to_stereo_44k(pcm))
}

/// Resample (linear) to interleaved 44.1 kHz stereo.
pub fn to_stereo_44k(pcm: &Pcm) -> Vec<i16> {
    let ch = pcm.channels.max(1);
    let frames_in = pcm.samples.len() / ch;
    let frames_out = (frames_in as u64 * OUTPUT_RATE as u64 / pcm.rate.max(1) as u64) as usize;
    let step = pcm.rate as f64 / OUTPUT_RATE as f64;
    let sample = |frame: usize, c: usize| -> f64 {
        let f = frame.min(frames_in.saturating_sub(1));
        pcm.samples.get(f * ch + c.min(ch - 1)).copied().unwrap_or(0) as f64
    };
    let mut out = Vec::with_capacity(frames_out * 2);
    for i in 0..frames_out {
        let pos = i as f64 * step;
        let (f0, t) = (pos as usize, pos.fract());
        for c in 0..2 {
            out.push((sample(f0, c) * (1.0 - t) + sample(f0 + 1, c) * t).round() as i16);
        }
    }
    out
}

/// Wrap interleaved 44.1 kHz stereo samples in a WAV container.
pub fn wav_from_stereo(samples: &[i16]) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&OUTPUT_RATE.to_le_bytes());
    out.extend_from_slice(&(OUTPUT_RATE * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for v in samples {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Cheap duration estimate from headers alone (used when nothing decodes the sound).
pub fn estimate_duration_ms(bytes: &[u8]) -> Option<f64> {
    if bytes.starts_with(b"RIFF") {
        // Walk the chunks for fmt (byte rate) and data (size).
        let mut pos = 12;
        let mut byte_rate = 0u32;
        while pos + 8 <= bytes.len() {
            let id = &bytes[pos..pos + 4];
            let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
            if id == b"fmt " && pos + 16 <= bytes.len() {
                byte_rate = u32::from_le_bytes(bytes[pos + 16..pos + 20].try_into().ok()?);
            }
            if id == b"data" && byte_rate > 0 {
                return Some(size.min(bytes.len() - pos - 8) as f64 / byte_rate as f64 * 1000.0);
            }
            pos += 8 + size + (size & 1);
        }
        return None;
    }
    if bytes.starts_with(b"OggS") {
        // Last page's granule position = total samples; rate from the Vorbis id header.
        let rate = bytes
            .windows(7)
            .position(|w| w == b"\x01vorbis")
            .and_then(|p| {
                bytes
                    .get(p + 12..p + 16)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            })?;
        let last = bytes.windows(4).rposition(|w| w == b"OggS")?;
        let granule = u64::from_le_bytes(bytes.get(last + 6..last + 14)?.try_into().ok()?);
        return Some(granule as f64 / rate.max(1) as f64 * 1000.0);
    }
    // MP3: assume a constant bitrate from the first frame header.
    let mut i = 0;
    if bytes.starts_with(b"ID3") && bytes.len() > 10 {
        let s = &bytes[6..10];
        i = 10
            + ((s[0] as usize) << 21
                | (s[1] as usize) << 14
                | (s[2] as usize) << 7
                | s[3] as usize);
    }
    while i + 4 <= bytes.len() {
        if bytes[i] == 0xFF && bytes[i + 1] & 0xE0 == 0xE0 {
            let version = (bytes[i + 1] >> 3) & 3; // 3 = MPEG1
            let idx = (bytes[i + 2] >> 4) as usize;
            const V1: [u32; 16] = [
                0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
            ];
            const V2: [u32; 16] = [
                0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
            ];
            let kbps = if version == 3 { V1[idx] } else { V2[idx] };
            if kbps > 0 {
                return Some((bytes.len() - i) as f64 * 8.0 / kbps as f64);
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_length_across_blocks() {
        let mut r = Resampler::new(44_100);
        let mut out = Vec::new();
        let block: Vec<f32> = (0..2205).map(|i| i as f32 / 2205.0).collect(); // 0.1 s mono at 22.05 kHz
        for _ in 0..10 {
            r.push(22_050, 1, &block, &mut out);
        }
        let frames = out.len() / 2;
        assert!((frames as i64 - 44_100).abs() <= 2, "{frames} frames for 1 s");
        assert_eq!(out[0], out[1], "mono goes to both channels");
    }

    #[test]
    fn stream_reports_wav_duration() {
        let pcm = Pcm { rate: 22_050, channels: 1, samples: vec![0; 22_050] };
        let s = Stream::open(to_wav(&pcm).into()).unwrap();
        assert!((s.duration_ms().unwrap() - 1000.0).abs() < 1.0);
    }

    #[test]
    fn wav_round_trip_keeps_duration() {
        // 0.5 s of 22.05 kHz mono silence with a ramp.
        let samples: Vec<i16> = (0..11_025).map(|i| (i % 1000) as i16).collect();
        let pcm = Pcm {
            rate: 22_050,
            channels: 1,
            samples,
        };
        let wav = to_wav(&pcm);
        let est = estimate_duration_ms(&wav).unwrap();
        assert!((est - 500.0).abs() < 1.0, "estimated {est}");
        let back = decode(&wav).unwrap();
        assert_eq!(back.rate, OUTPUT_RATE);
        assert_eq!(back.channels, 2);
        assert!((back.duration_ms() - 500.0).abs() < 1.0);
    }
}
