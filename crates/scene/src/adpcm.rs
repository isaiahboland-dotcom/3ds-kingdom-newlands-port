//! Nintendo DSP-ADPCM, the format the 3DS DSP plays without the CPU, and
//! the `.adp` files `tools/extract` writes (one per clip).
//!
//! Each channel is a run of 8-byte frames of 14 samples: a header byte
//! (predictor pair in the high nibble, scale exponent in the low one) and
//! 14 signed 4-bit residuals. A sample is
//! `(residual << scale << 11) + 1024 + c1 * h1 + c2 * h2 >> 11` with the
//! channel's 8 coefficient pairs (11 fractional bits) and the last two
//! samples.
//!
//! `.adp`: `KADP`, then little-endian u32 version, rate, samples per
//! channel and channel count, and an f32 playback gain (clips are stored
//! peak-normalized; play them at `gain` for the original level); per channel 16 coefficients (i16) and its
//! data length (u32); then each channel's frames, one channel after another.

pub const FRAME_SAMPLES: usize = 14;
pub const FRAME_BYTES: usize = 8;
const MAGIC: &[u8; 4] = b"KADP";
const VERSION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Channel {
    pub coefs: [i16; 16],
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Adp {
    pub rate: u32,
    /// Samples per channel.
    pub samples: u32,
    pub channels: Vec<Channel>,
    /// Scale back to the source's level (the samples are normalized).
    pub gain: f32,
}

/// Byte size of the header for `channels` channels.
pub fn header_len(channels: usize) -> usize {
    24 + channels * 36
}

/// Bytes of frame data for `samples` samples.
pub fn data_len(samples: usize) -> usize {
    samples.div_ceil(FRAME_SAMPLES) * FRAME_BYTES
}

impl Adp {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        for v in [VERSION, self.rate, self.samples, self.channels.len() as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.gain.to_le_bytes());
        for c in &self.channels {
            for k in c.coefs {
                out.extend_from_slice(&k.to_le_bytes());
            }
            out.extend_from_slice(&(c.data.len() as u32).to_le_bytes());
        }
        for c in &self.channels {
            out.extend_from_slice(&c.data);
        }
        out
    }

    /// Reads the header (`bytes` may stop there: channel data is left empty).
    pub fn header(bytes: &[u8]) -> Option<Adp> {
        if bytes.len() < 24 || &bytes[..4] != MAGIC {
            return None;
        }
        let u = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        if u(4) != VERSION {
            return None;
        }
        let (rate, samples, n, gain) = (u(8), u(12), u(16) as usize, f32::from_bits(u(20)));
        if n == 0 || n > 2 || bytes.len() < header_len(n) {
            return None;
        }
        let channels = (0..n)
            .map(|c| {
                let at = 24 + c * 36;
                let mut coefs = [0i16; 16];
                for (k, v) in coefs.iter_mut().enumerate() {
                    *v = i16::from_le_bytes([bytes[at + k * 2], bytes[at + k * 2 + 1]]);
                }
                Channel {
                    coefs,
                    data: Vec::new(),
                }
            })
            .collect();
        Some(Adp {
            rate,
            samples,
            channels,
            gain,
        })
    }

    /// Reads a whole file.
    pub fn from_bytes(bytes: &[u8]) -> Option<Adp> {
        let mut a = Adp::header(bytes)?;
        let len = data_len(a.samples as usize);
        let mut at = header_len(a.channels.len());
        for c in &mut a.channels {
            c.data = bytes.get(at..at + len)?.to_vec();
            at += len;
        }
        Some(a)
    }
}

/// The decoder's running state: the last two samples.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct History(pub i32, pub i32);

/// Decodes one frame into `out` (14 samples).
pub fn decode_frame(coefs: &[i16; 16], frame: &[u8], h: &mut History, out: &mut [i16]) {
    let ps = frame[0];
    let pair = ((ps >> 4) & 7) as usize;
    let scale = 1i32 << (ps & 15);
    let (c1, c2) = (coefs[pair * 2] as i32, coefs[pair * 2 + 1] as i32);
    for (i, o) in out.iter_mut().enumerate().take(FRAME_SAMPLES) {
        let byte = frame[1 + i / 2];
        let nibble = if i % 2 == 0 { byte >> 4 } else { byte & 15 };
        let r = (((nibble as i8) << 4) >> 4) as i32;
        let s = ((((r * scale) << 11) + 1024 + c1 * h.0 + c2 * h.1) >> 11).clamp(-32768, 32767);
        h.1 = h.0;
        h.0 = s;
        *o = s as i16;
    }
}

/// Decodes a whole channel.
pub fn decode(c: &Channel, samples: usize) -> Vec<i16> {
    let mut out = vec![0i16; data_len(samples) / FRAME_BYTES * FRAME_SAMPLES];
    let mut h = History::default();
    for (frame, o) in c.data.chunks(FRAME_BYTES).zip(out.chunks_mut(FRAME_SAMPLES)) {
        decode_frame(&c.coefs, frame, &mut h, o);
    }
    out.truncate(samples);
    out
}

/// Encodes one channel.
pub fn encode(pcm: &[i16]) -> Channel {
    let coefs = design(pcm);
    let pairs: Vec<(i32, i32)> = (0..8)
        .map(|k| (coefs[k * 2] as i32, coefs[k * 2 + 1] as i32))
        .collect();
    let mut data = Vec::with_capacity(data_len(pcm.len()));
    let mut h = History::default();
    for chunk in pcm.chunks(FRAME_SAMPLES) {
        let mut x = [0i32; FRAME_SAMPLES];
        for (d, s) in x.iter_mut().zip(chunk) {
            *d = *s as i32;
        }
        // Best (pair, scale) by squared error, decoding as the DSP will.
        let mut best: Option<(i64, u8, [i8; FRAME_SAMPLES], History)> = None;
        for (k, &(c1, c2)) in pairs.iter().enumerate() {
            // Smallest scale the open-loop residual fits.
            let mut peak = 0i32;
            let (mut p1, mut p2) = (h.0, h.1);
            for &s in &x {
                let pred = (c1 * p1 + c2 * p2 + 1024) >> 11;
                peak = peak.max((s - pred).abs());
                p2 = p1;
                p1 = s;
            }
            let mut s0 = 0u8;
            while s0 < 12 && peak > 7 << s0 {
                s0 += 1;
            }
            for scale in s0.saturating_sub(1)..=(s0 + 1).min(12) {
                let (err, nibbles, after) = try_frame(&x, c1, c2, scale, h);
                if best.as_ref().is_none_or(|b| err < b.0) {
                    best = Some((err, (k as u8) << 4 | scale, nibbles, after));
                }
            }
        }
        let (_, ps, nibbles, after) = best.expect("8 predictors tried");
        h = after;
        data.push(ps);
        for pair in nibbles.chunks(2) {
            data.push(((pair[0] as u8 & 15) << 4) | (pair[1] as u8 & 15));
        }
    }
    Channel { coefs, data }
}

fn try_frame(
    x: &[i32; FRAME_SAMPLES],
    c1: i32,
    c2: i32,
    scale: u8,
    mut h: History,
) -> (i64, [i8; FRAME_SAMPLES], History) {
    let step = (1i64 << scale) << 11;
    let mut nibbles = [0i8; FRAME_SAMPLES];
    let mut err = 0i64;
    for (i, &s) in x.iter().enumerate() {
        let pred = (c1 * h.0 + c2 * h.1) as i64;
        let want = (s as i64) * 2048 - pred;
        let r = ((want as f64) / step as f64).round().clamp(-8.0, 7.0) as i32;
        let out = ((((r << scale) as i64) << 11) + 1024 + pred) >> 11;
        let out = out.clamp(-32768, 32767) as i32;
        err += ((s - out) as i64).pow(2);
        nibbles[i] = r as i8;
        h.1 = h.0;
        h.0 = out;
    }
    (err, nibbles, h)
}

/// Eight predictor pairs for `pcm`: each 14-sample frame's best 2-tap
/// predictor, clustered (k-means, weighted by loudness).
fn design(pcm: &[i16]) -> [i16; 16] {
    let x: Vec<f64> = pcm.iter().map(|&s| s as f64).collect();
    let mut points: Vec<(f64, f64, f64)> = Vec::new();
    for start in (2..x.len().max(2)).step_by(FRAME_SAMPLES) {
        let end = (start + 2 * FRAME_SAMPLES).min(x.len());
        let (mut r11, mut r22, mut r12, mut b1, mut b2, mut e) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for n in start..end {
            let (a, b, c) = (x[n], x[n - 1], x[n - 2]);
            r11 += b * b;
            r22 += c * c;
            r12 += b * c;
            b1 += a * b;
            b2 += a * c;
            e += a * a;
        }
        let det = r11 * r22 - r12 * r12;
        if e < 1.0 || det.abs() < 1e-9 * (r11 * r22).max(1.0) {
            continue;
        }
        let a1 = (b1 * r22 - b2 * r12) / det;
        let a2 = (b2 * r11 - b1 * r12) / det;
        let (a1, a2) = stable(a1, a2);
        points.push((a1, a2, e.sqrt()));
    }
    // Starting set: silence, hold, linear and smoother extrapolations.
    let mut centers: Vec<(f64, f64)> = vec![
        (0.0, 0.0),
        (1.0, 0.0),
        (1.99, -0.99),
        (1.8, -0.85),
        (1.5, -0.6),
        (0.9, 0.0),
        (1.2, -0.3),
        (0.5, 0.0),
    ];
    for _ in 0..12 {
        let mut sums = [(0.0f64, 0.0f64, 0.0f64); 8];
        for &(a1, a2, w) in &points {
            let k = nearest(&centers, a1, a2);
            sums[k].0 += a1 * w;
            sums[k].1 += a2 * w;
            sums[k].2 += w;
        }
        for (c, s) in centers.iter_mut().zip(sums) {
            if s.2 > 0.0 {
                *c = stable(s.0 / s.2, s.1 / s.2);
            }
        }
    }
    let mut out = [0i16; 16];
    for (k, (a1, a2)) in centers.into_iter().enumerate() {
        out[k * 2] = (a1 * 2048.0).round().clamp(-32768.0, 32767.0) as i16;
        out[k * 2 + 1] = (a2 * 2048.0).round().clamp(-32768.0, 32767.0) as i16;
    }
    out
}

fn nearest(centers: &[(f64, f64)], a1: f64, a2: f64) -> usize {
    let d = |c: &(f64, f64)| (c.0 - a1).powi(2) + (c.1 - a2).powi(2);
    (0..centers.len())
        .min_by(|&i, &j| d(&centers[i]).total_cmp(&d(&centers[j])))
        .unwrap_or(0)
}

/// Pulls a predictor into the stable triangle (|a2| < 1, |a1| < 1 - a2).
fn stable(a1: f64, a2: f64) -> (f64, f64) {
    let a2 = a2.clamp(-0.995, 0.995);
    let lim = (1.0 - a2) * 0.999;
    (a1.clamp(-lim, lim), a2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snr(a: &[i16], b: &[i16]) -> f64 {
        let s: f64 = a.iter().map(|&v| (v as f64).powi(2)).sum();
        let n: f64 = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum();
        10.0 * (s / n.max(1.0)).log10()
    }

    #[test]
    fn round_trips_a_tone_and_noise() {
        let mut seed = 1u32;
        let pcm: Vec<i16> = (0..22050)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = ((seed >> 16) as i32 - 32768) as f64 / 32768.0;
                let t = i as f64 / 22050.0;
                ((t * 440.0 * std::f64::consts::TAU).sin() * 12000.0
                    + (t * 3000.0 * std::f64::consts::TAU).sin() * 4000.0
                    + noise * 800.0) as i16
            })
            .collect();
        let c = encode(&pcm);
        assert_eq!(c.data.len(), data_len(pcm.len()));
        let back = decode(&c, pcm.len());
        let q = snr(&pcm, &back);
        assert!(q > 30.0, "SNR {q:.1} dB");
        let file = Adp {
            rate: 22050,
            samples: pcm.len() as u32,
            channels: vec![c],
            gain: 0.5,
        };
        assert_eq!(Adp::from_bytes(&file.to_bytes()), Some(file));
    }
}
