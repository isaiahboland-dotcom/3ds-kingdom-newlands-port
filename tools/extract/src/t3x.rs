//! A small `.t3x` writer: what devkitPro's `tex3ds` makes for one texture,
//! so packing the 3DS assets needs no devkitPro install.
//!
//! The file is libctru's `Tex3DS` layout: a 5-byte header (sub-texture
//! count, log2 size, format, mipmap levels), one 12-byte sub-texture
//! covering the whole image, then the pixels as the GPU wants them (8×8
//! Morton-ordered tiles, top row first; the sub-texture's flipped v range
//! makes it upright) under an LZ11 stream.

/// Pixel formats the port uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Rgba8,
    Rgba5551,
}

/// `rgba` is `w * h * 4` bytes, top row first. `w` and `h` must be powers of
/// two between 8 and 1024.
pub fn encode(w: u32, h: u32, rgba: &[u8], format: Format) -> Vec<u8> {
    assert!(
        w.is_power_of_two()
            && h.is_power_of_two()
            && (8..=1024).contains(&w)
            && (8..=1024).contains(&h)
    );
    assert_eq!(rgba.len(), (w * h * 4) as usize);
    let log2 = |n: u32| (n.trailing_zeros() - 3) as u8;
    let mut out = Vec::new();
    out.extend_from_slice(&1u16.to_le_bytes()); // one sub-texture
    out.push(log2(w) | (log2(h) << 3));
    out.push(match format {
        Format::Rgba8 => 0,
        Format::Rgba5551 => 2,
    });
    out.push(0); // mipmap levels
    for v in [w as u16, h as u16, 0, 1024, 1024, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&lz11(&swizzle(w, h, rgba, format)));
    out
}

/// Tiles of 8×8 pixels in Morton order, tile rows from the top of the image.
fn swizzle(w: u32, h: u32, rgba: &[u8], format: Format) -> Vec<u8> {
    let bpp = match format {
        Format::Rgba8 => 4,
        Format::Rgba5551 => 2,
    };
    let mut out = Vec::with_capacity((w * h) as usize * bpp);
    for ty in 0..h / 8 {
        for tx in 0..w / 8 {
            for m in 0..64u32 {
                // De-interleave the Morton index: x gets the even bits, y the odd.
                let x = (m & 1) | ((m >> 1) & 2) | ((m >> 2) & 4);
                let y = ((m >> 1) & 1) | ((m >> 2) & 2) | ((m >> 3) & 4);
                let px = tx * 8 + x;
                let py = ty * 8 + y;
                let mut p = [0u8; 4];
                p.copy_from_slice(&rgba[((py * w + px) * 4) as usize..][..4]);
                // Like tex3ds, fully transparent pixels lose their colour.
                if p[3] == 0 {
                    p = [0; 4];
                }
                match format {
                    Format::Rgba8 => out.extend_from_slice(&[p[3], p[2], p[1], p[0]]),
                    Format::Rgba5551 => {
                        let c5 = |c: u8| (c >> 3) as u16;
                        let v = (c5(p[0]) << 11)
                            | (c5(p[1]) << 6)
                            | (c5(p[2]) << 1)
                            | (p[3] >= 128) as u16;
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
    }
    out
}

/// Nintendo LZ11 (header byte 0x11, 24-bit size) with a greedy hash-chain match finder.
fn lz11(data: &[u8]) -> Vec<u8> {
    const WINDOW: usize = 0x1000;
    const MIN: usize = 3;
    const MAX: usize = 0x10110;
    const CHAIN: usize = 64;
    let n = data.len();
    let mut out = vec![0x11, n as u8, (n >> 8) as u8, (n >> 16) as u8];
    if n > 0xFF_FFFF {
        out[0] = 0x11;
        out.splice(1..4, [0, 0, 0]);
        out.extend_from_slice(&(n as u32).to_le_bytes());
    }
    let hash = |i: usize| {
        ((data[i] as usize) << 8 ^ (data[i + 1] as usize) << 4 ^ data[i + 2] as usize) & 0xFFFF
    };
    let mut head = vec![usize::MAX; 0x10000];
    let mut prev = vec![usize::MAX; n];
    let insert = |i: usize, head: &mut Vec<usize>, prev: &mut Vec<usize>| {
        if i + MIN <= n {
            let h = hash(i);
            prev[i] = head[h];
            head[h] = i;
        }
    };

    let mut i = 0;
    let mut flag_at = out.len();
    let mut bit = 8;
    while i < n {
        if bit == 8 {
            flag_at = out.len();
            out.push(0);
            bit = 0;
        }
        // Longest match in the window.
        let (mut best_len, mut best_dist) = (0, 0);
        if i + MIN <= n {
            let max = (n - i).min(MAX);
            let mut cand = head[hash(i)];
            let mut tries = CHAIN;
            while cand != usize::MAX && i - cand <= WINDOW && tries > 0 {
                let mut l = 0;
                while l < max && data[cand + l] == data[i + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_dist = i - cand;
                    if l == max {
                        break;
                    }
                }
                cand = prev[cand];
                tries -= 1;
            }
        }
        if best_len >= MIN {
            out[flag_at] |= 0x80 >> bit;
            let l = best_len;
            let d = best_dist - 1;
            if l <= 0x10 {
                out.push((((l - 1) << 4) | (d >> 8)) as u8);
            } else if l <= 0x110 {
                let l = l - 0x11;
                out.push((l >> 4) as u8);
                out.push((((l & 0xF) << 4) | (d >> 8)) as u8);
            } else {
                let l = l - 0x111;
                out.push(0x10 | (l >> 12) as u8);
                out.push((l >> 4) as u8);
                out.push((((l & 0xF) << 4) | (d >> 8)) as u8);
            }
            out.push(d as u8);
            for k in 0..best_len {
                insert(i + k, &mut head, &mut prev);
            }
            i += best_len;
        } else {
            out.push(data[i]);
            insert(i, &mut head, &mut prev);
            i += 1;
        }
        bit += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference LZ11 decoder.
    fn unlz11(src: &[u8]) -> Vec<u8> {
        assert_eq!(src[0], 0x11);
        let mut size = src[1] as usize | (src[2] as usize) << 8 | (src[3] as usize) << 16;
        let mut p = 4;
        if size == 0 && src.len() > 4 {
            size = u32::from_le_bytes(src[4..8].try_into().unwrap()) as usize;
            p = 8;
        }
        let mut out = Vec::with_capacity(size);
        while out.len() < size {
            let flags = src[p];
            p += 1;
            for b in 0..8 {
                if out.len() >= size {
                    break;
                }
                if flags & (0x80 >> b) == 0 {
                    out.push(src[p]);
                    p += 1;
                    continue;
                }
                let b1 = src[p] as usize;
                let (len, disp) = match b1 >> 4 {
                    0 => {
                        let l = (((b1 & 0xF) << 4) | (src[p + 1] as usize >> 4)) + 0x11;
                        let d = ((src[p + 1] as usize & 0xF) << 8) | src[p + 2] as usize;
                        p += 3;
                        (l, d)
                    }
                    1 => {
                        let l = (((b1 & 0xF) << 12)
                            | ((src[p + 1] as usize) << 4)
                            | (src[p + 2] as usize >> 4))
                            + 0x111;
                        let d = ((src[p + 2] as usize & 0xF) << 8) | src[p + 3] as usize;
                        p += 4;
                        (l, d)
                    }
                    n => {
                        let d = ((b1 & 0xF) << 8) | src[p + 1] as usize;
                        p += 2;
                        (n + 1, d)
                    }
                };
                let from = out.len() - disp - 1;
                for k in 0..len {
                    out.push(out[from + k]);
                }
            }
        }
        out
    }

    #[test]
    fn lz11_round_trips() {
        let mut data = Vec::new();
        let mut s = 12345u32;
        for i in 0..200_000usize {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            // Runs, repeats and noise, so every match length class shows up.
            data.push(match (i / 5000) % 4 {
                0 => 7,
                1 => (i % 13) as u8,
                2 => (s >> 24) as u8,
                _ => ((i / 3) % 251) as u8,
            });
        }
        assert_eq!(unlz11(&lz11(&data)), data);
        assert_eq!(unlz11(&lz11(&[])), Vec::<u8>::new());
        assert_eq!(unlz11(&lz11(&[1, 2])), vec![1, 2]);
    }

    /// Same pixels as devkitPro's `tex3ds`, when it's installed.
    #[test]
    fn matches_tex3ds() {
        let tool = std::path::Path::new("/opt/devkitpro/tools/bin/tex3ds");
        if !tool.exists() {
            eprintln!("tex3ds not installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("t3x-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (w, h) in [(64u32, 32u32), (1024, 16)] {
            let mut rgba = Vec::new();
            let mut s = 99u32;
            for _ in 0..w * h {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                let a = match (s >> 20) & 3 {
                    0 => 0,
                    1 => (s >> 4) as u8,
                    _ => 255,
                };
                rgba.extend_from_slice(&[(s >> 8) as u8, (s >> 16) as u8, (s >> 24) as u8, a]);
            }
            let png_path = dir.join("in.png");
            let mut enc = png::Encoder::new(std::fs::File::create(&png_path).unwrap(), w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&rgba).unwrap();
            for (name, format) in [("rgba8", Format::Rgba8), ("rgba5551", Format::Rgba5551)] {
                let out = dir.join("ref.t3x");
                let ok = std::process::Command::new(tool)
                    .args(["-f", name, "-z", "none", "-o"])
                    .arg(&out)
                    .arg(&png_path)
                    .status()
                    .unwrap()
                    .success();
                assert!(ok);
                let reference = std::fs::read(&out).unwrap();
                let mine = encode(w, h, &rgba, format);
                assert_eq!(&mine[..17], &reference[..17], "header {w}x{h} {name}");
                // tex3ds -z none: a 0x00 type byte and 24-bit size, then the raw pixels.
                assert_eq!(reference[17], 0);
                let got = unlz11(&mine[17..]);
                let want = &reference[21..];
                assert_eq!(got.len(), want.len());
                if let Some(i) = (0..got.len()).find(|&i| got[i] != want[i]) {
                    panic!(
                        "pixels {w}x{h} {name}: first diff at byte {i}: {:02x?} vs {:02x?}",
                        &got[i & !3..][..4],
                        &want[i & !3..][..4]
                    );
                }
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
