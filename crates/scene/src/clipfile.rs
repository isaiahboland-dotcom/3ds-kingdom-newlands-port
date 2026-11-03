//! Sound files → clips at the game's rate, levelled the way `tools/extract`
//! stores the original's (normalized, driven under a limiter, with the gain
//! that undoes it). Used by the asset extractor. OGG Vorbis and PCM WAV.

use std::path::Path;

use crate::adpcm::{self, Adp};

/// The rate every clip is stored at.
pub const RATE: u32 = 22050;

/// A clip ready to play or encode: one buffer per channel, and the gain
/// that brings it back to its source level.
pub struct Pcm {
    pub rate: u32,
    pub channels: Vec<Vec<i16>>,
    pub gain: f32,
}

impl Pcm {
    /// As DSP-ADPCM (what both backends play from disk).
    pub fn to_adp(&self) -> Adp {
        Adp {
            rate: self.rate,
            samples: self.channels.first().map_or(0, Vec::len) as u32,
            channels: self.channels.iter().map(|c| adpcm::encode(c)).collect(),
            gain: self.gain,
        }
    }
}

/// Decodes `path` and levels it: up to two channels when `stereo` (the
/// soundtrack), else mixed down to one (effects are placed by the mixer).
pub fn prepare(path: &Path, stereo: bool) -> Result<Pcm, String> {
    let (rate, chans) = decode(path)?;
    if chans.first().is_none_or(Vec::is_empty) {
        return Err(format!("{}: no samples", path.display()));
    }
    let chans: Vec<Vec<f32>> = if stereo || chans.len() == 1 {
        chans.into_iter().take(2).collect()
    } else {
        let n = chans.len() as f32;
        vec![(0..chans[0].len())
            .map(|k| chans.iter().map(|c| c[k]).sum::<f32>() / n)
            .collect()]
    };
    let mut chans: Vec<Vec<f32>> = chans.iter().map(|c| resample(c, rate, RATE)).collect();
    // Stored at full scale (the 3DS can't amplify a voice past 1), played
    // back at `gain`.
    let peak = chans.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
    let norm = if peak > 0.0 {
        (NORMALIZED / peak).clamp(1.0, MAX_NORMALIZE)
    } else {
        1.0
    };
    limit(&mut chans, norm * DRIVE, RATE);
    Ok(Pcm {
        rate: RATE,
        channels: chans.iter().map(|c| to_i16(c)).collect(),
        gain: 1.0 / (norm * DRIVE),
    })
}

/// Decoded audio: rate and one buffer per channel.
fn decode(path: &Path) -> Result<(u32, Vec<Vec<f32>>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    match path.extension().and_then(|e| e.to_str()) {
        Some("ogg") => {
            let mut r = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(bytes))
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let n = r.ident_hdr.audio_channels as usize;
            let rate = r.ident_hdr.audio_sample_rate;
            let mut ch = vec![Vec::new(); n];
            while let Some(pkt) = r
                .read_dec_packet()
                .map_err(|e| format!("{}: {e}", path.display()))?
            {
                for (c, s) in ch.iter_mut().zip(pkt) {
                    c.extend(s.into_iter().map(|v| v as f32 / 32768.0));
                }
            }
            Ok((rate, ch))
        }
        _ => wav(&bytes).ok_or_else(|| format!("{}: unsupported WAV", path.display())),
    }
}

/// PCM WAV (8/16/24-bit or 32-bit float).
fn wav(b: &[u8]) -> Option<(u32, Vec<Vec<f32>>)> {
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let u16_ = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_ = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let (mut fmt, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= b.len() {
        let len = u32_(at + 4) as usize;
        let body = at + 8..(at + 8 + len).min(b.len());
        let end = body.end;
        match &b[at..at + 4] {
            b"fmt " => {
                if body.len() < 16 {
                    return None;
                }
                fmt = Some(body.start);
            }
            b"data" => data = Some(body),
            _ => {}
        }
        at = end + (len & 1);
    }
    let (fmt, data) = (fmt?, data?);
    let (tag, n, rate, bits) = (
        u16_(fmt),
        u16_(fmt + 2) as usize,
        u32_(fmt + 4),
        u16_(fmt + 14),
    );
    let bytes = (bits / 8) as usize;
    if n == 0 || bytes == 0 {
        return None;
    }
    let frames = (data.end - data.start) / (bytes * n);
    let mut ch = vec![Vec::with_capacity(frames); n];
    for i in 0..frames {
        for (c, out) in ch.iter_mut().enumerate() {
            let p = data.start + (i * n + c) * bytes;
            let v = match (tag, bits) {
                (3, 32) => f32::from_le_bytes(b[p..p + 4].try_into().unwrap()),
                (_, 8) => (b[p] as f32 - 128.0) / 128.0,
                (_, 16) => i16::from_le_bytes([b[p], b[p + 1]]) as f32 / 32768.0,
                (_, 24) => {
                    (i32::from_le_bytes([0, b[p], b[p + 1], b[p + 2]]) >> 8) as f32 / 8388608.0
                }
                (_, 32) => {
                    i32::from_le_bytes(b[p..p + 4].try_into().unwrap()) as f32 / 2147483648.0
                }
                _ => return None,
            };
            out.push(v);
        }
    }
    Some((rate, ch))
}

/// Windowed-sinc resampling (low-passed below the new Nyquist).
fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return x.to_vec();
    }
    const TAPS: i64 = 16;
    let ratio = from as f64 / to as f64;
    let cutoff = (to as f64 / from as f64).min(1.0) * 0.92;
    let n = ((x.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(n);
    let span = (TAPS as f64 / cutoff).ceil() as i64;
    for i in 0..n {
        let t = i as f64 * ratio;
        let centre = t.floor() as i64;
        let (mut acc, mut wsum) = (0.0f64, 0.0f64);
        for k in centre - span + 1..=centre + span {
            let d = t - k as f64;
            let arg = d * cutoff;
            let sinc = if arg.abs() < 1e-9 {
                1.0
            } else {
                (arg * std::f64::consts::PI).sin() / (arg * std::f64::consts::PI)
            };
            // Blackman window over the kernel's span.
            let w = 0.5 * (1.0 + (std::f64::consts::PI * d / span as f64).cos());
            let wk = sinc * w;
            wsum += wk;
            if k >= 0 && (k as usize) < x.len() {
                acc += x[k as usize] as f64 * wk;
            }
        }
        out.push((acc / wsum.max(1e-9)) as f32);
    }
    out
}

/// Peak level clips are normalized to (headroom for ADPCM overshoot).
const NORMALIZED: f32 = 0.9;
/// Quiet clips are raised at most this much (no amplified noise floors).
const MAX_NORMALIZE: f32 = 16.0;

/// How much hotter clips are stored than their normalized level, held
/// under `NORMALIZED` by a limiter: the 3DS speakers need the loudness and
/// the DSP can't amplify past a voice's full scale. Playback gains divide
/// it back out, so the mix keeps the original's balance.
pub const DRIVE: f32 = 4.0;
/// Limiter look-ahead and release (seconds).
const LOOKAHEAD: f32 = 0.003;
const RELEASE: f32 = 0.08;

/// Drives `chans` by `drive` through a look-ahead peak limiter (linked
/// across channels) with a ceiling of `NORMALIZED`.
fn limit(chans: &mut [Vec<f32>], drive: f32, rate: u32) {
    let n = chans.first().map_or(0, Vec::len);
    let ahead = ((LOOKAHEAD * rate as f32) as usize).max(1);
    let release = 1.0 - (-1.0 / (RELEASE * rate as f32)).exp();
    // The gain each sample needs, then the lowest one coming up within the
    // look-ahead (a sliding minimum, so peaks are caught before they land).
    let need: Vec<f32> = (0..n)
        .map(|k| {
            let peak = chans.iter().fold(0.0f32, |m, c| m.max(c[k].abs())) * drive;
            if peak > NORMALIZED {
                NORMALIZED / peak
            } else {
                1.0
            }
        })
        .collect();
    let mut window = std::collections::VecDeque::new();
    let mut ahead_min = vec![1.0f32; n];
    for k in (0..n).rev() {
        while window.back().is_some_and(|&j: &usize| need[j] >= need[k]) {
            window.pop_back();
        }
        window.push_back(k);
        while window.front().is_some_and(|&j| j > k + ahead) {
            window.pop_front();
        }
        ahead_min[k] = need[*window.front().unwrap()];
    }
    // Falls at once, recovers over `RELEASE`; a ramp across the look-ahead
    // keeps the drop from clicking.
    let mut g = 1.0f32;
    let attack = 1.0 / ahead as f32;
    for k in 0..n {
        let target = ahead_min[k];
        g = if target < g {
            (g - attack).max(target)
        } else {
            g + (target - g) * release
        };
        let g = g.min(need[k]);
        for c in chans.iter_mut() {
            c[k] *= drive * g;
        }
    }
}

fn to_i16(x: &[f32]) -> Vec<i16> {
    x.iter()
        .map(|v| (v * 32767.0).round().clamp(-32768.0, 32767.0) as i16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16-bit stereo WAV of a quarter second of 440 Hz at 44.1 kHz.
    fn tone() -> Vec<u8> {
        let frames = 44100 / 4;
        let mut data = Vec::new();
        for i in 0..frames {
            let v = ((i as f32 * 440.0 / 44100.0 * std::f32::consts::TAU).sin() * 8000.0) as i16;
            data.extend_from_slice(&v.to_le_bytes());
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        for v in [1u16, 2] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&44100u32.to_le_bytes());
        b.extend_from_slice(&(44100u32 * 4).to_le_bytes());
        for v in [4u16, 16] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend(data);
        b
    }

    #[test]
    fn a_wav_becomes_a_levelled_clip_at_the_game_rate() {
        let path = std::env::temp_dir().join(format!("clipfile-test-{}.wav", std::process::id()));
        std::fs::write(&path, tone()).unwrap();
        let effect = prepare(&path, false).unwrap();
        let music = prepare(&path, true).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(effect.rate, RATE);
        assert_eq!(effect.channels.len(), 1);
        assert_eq!(music.channels.len(), 2);
        assert!((effect.channels[0].len() as i64 - RATE as i64 / 4).abs() <= 1);
        // Stored hot, with the gain that leaves it `DRIVE` under the source's
        // ~0.24 peak, as the game's own clips are.
        let peak = effect.channels[0]
            .iter()
            .map(|v| v.unsigned_abs())
            .max()
            .unwrap() as f32
            / 32768.0;
        assert!(peak > 0.8 && peak <= 0.91, "{peak}");
        let level = peak * effect.gain * DRIVE;
        assert!((level - 8000.0 / 32768.0).abs() < 0.01, "{level}");
        let adp = effect.to_adp();
        assert_eq!(adp.samples as usize, effect.channels[0].len());
    }
}
