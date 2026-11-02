//! Makes the HOME Menu art for the `.cia` from your own game files: the
//! 48×48 icon (the crown), the 256×128 banner (the title logo over a night
//! sky) and the banner's jingle (the crown being picked back up). Like the
//! romfs, the output is never committed.
//!
//! `cargo run --release -p extract --bin ciaart -- reference/ripped/ExportedProject assets <out dir>`
//! writes `icon.png`, `banner.png` and `banner.wav`; `tools/cia/build.sh` runs it.

use std::path::Path;

use scene::adpcm::{self, Adp};

struct Img {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Img {
    fn new(w: u32, h: u32) -> Img {
        Img {
            w,
            h,
            px: vec![0; (w * h * 4) as usize],
        }
    }

    fn at(&self, x: u32, y: u32) -> [f32; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        let p = &self.px[i..i + 4];
        [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
    }

    /// Alpha-blends `c` (0–255, straight alpha) over pixel (x, y).
    fn blend(&mut self, x: u32, y: u32, c: [f32; 4]) {
        let i = ((y * self.w + x) * 4) as usize;
        let a = c[3] / 255.0;
        for (k, value) in c.iter().enumerate().take(3) {
            self.px[i + k] = (value * a + self.px[i + k] as f32 * (1.0 - a)).round() as u8;
        }
        self.px[i + 3] = 255;
    }

    /// The bounding box of the visible pixels within (x, y, w, h).
    fn content(&self, x: u32, y: u32, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for j in y..y + h {
            for i in x..x + w {
                if self.at(i, j)[3] > 8.0 {
                    x0 = x0.min(i);
                    y0 = y0.min(j);
                    x1 = x1.max(i + 1);
                    y1 = y1.max(j + 1);
                }
            }
        }
        if x0 == u32::MAX {
            (x, y, w, h)
        } else {
            (x0, y0, x1 - x0, y1 - y0)
        }
    }
}

fn load_png(path: &Path) -> Result<Img, String> {
    let dec = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?,
    ));
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; r.output_buffer_size().unwrap_or(0)];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba {
        return Err(format!("{}: expected RGBA", path.display()));
    }
    Ok(Img {
        w: info.width,
        h: info.height,
        px: buf,
    })
}

fn save_png(path: &Path, img: &Img) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.w, img.h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&img.px).map_err(|e| e.to_string())
}

/// A night sky: deep blue at the top to near black at the bottom.
fn sky(w: u32, h: u32) -> Img {
    let mut img = Img::new(w, h);
    let (top, bottom) = ([38.0, 52.0, 92.0], [12.0, 14.0, 26.0]);
    for y in 0..h {
        let t = y as f32 / (h - 1) as f32;
        for x in 0..w {
            let c: Vec<f32> = (0..3).map(|k| top[k] + (bottom[k] - top[k]) * t).collect();
            img.blend(x, y, [c[0], c[1], c[2], 255.0]);
        }
    }
    img
}

/// Draws `src`'s (sx, sy, sw, sh) into `dst` at (dx, dy), `scale` times the
/// size: whole scales stay pixel-sharp, others are area-averaged.
fn draw(
    dst: &mut Img,
    src: &Img,
    (sx, sy, sw, sh): (u32, u32, u32, u32),
    (dx, dy): (u32, u32),
    scale: f32,
) {
    let (w, h) = (
        (sw as f32 * scale).round() as u32,
        (sh as f32 * scale).round() as u32,
    );
    for y in 0..h {
        for x in 0..w {
            // The source area this pixel covers, sampled on a 4×4 grid.
            let mut sum = [0.0f32; 4];
            for j in 0..4 {
                for i in 0..4 {
                    let u = ((x as f32 + (i as f32 + 0.5) / 4.0) / scale) as u32;
                    let v = ((y as f32 + (j as f32 + 0.5) / 4.0) / scale) as u32;
                    let p = src.at(sx + u.min(sw - 1), sy + v.min(sh - 1));
                    // Premultiplied, so clear pixels don't darken the edges.
                    for k in 0..3 {
                        sum[k] += p[k] * p[3] / 255.0;
                    }
                    sum[3] += p[3];
                }
            }
            let a = sum[3] / 16.0;
            if a > 0.0 && dx + x < dst.w && dy + y < dst.h {
                let c = [
                    sum[0] / 16.0 * 255.0 / a,
                    sum[1] / 16.0 * 255.0 / a,
                    sum[2] / 16.0 * 255.0 / a,
                    a,
                ];
                dst.blend(dx + x, dy + y, c);
            }
        }
    }
}

/// A 16-bit mono WAV.
fn wav(rate: u32, pcm: &[i16]) -> Vec<u8> {
    let mut out = Vec::new();
    let data = (pcm.len() * 2) as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    for v in [1u16, 1] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    for v in [2u16, 16] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// The banner jingle may be at most 3 s.
const JINGLE_SECONDS: f32 = 2.8;
const JINGLE: &str = "player_recovers_crown";

fn run(ripped: &Path, assets: &Path, out: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let tex = ripped.join("Assets/Texture2D");

    // Icon: the crown (first frame of the splash-screen strip), as large as
    // a whole scale allows.
    let crown = load_png(&tex.join("crown_splashscreen.png"))?;
    let frame = crown.content(0, 0, crown.h.min(crown.w), crown.h);
    let mut icon = sky(48, 48);
    let scale = (44 / frame.2.max(frame.3)).max(1) as f32;
    let (w, h) = (frame.2 as f32 * scale, frame.3 as f32 * scale);
    draw(
        &mut icon,
        &crown,
        frame,
        (((48.0 - w) / 2.0) as u32, ((48.0 - h) / 2.0) as u32),
        scale,
    );
    save_png(&out.join("icon.png"), &icon)?;

    // Banner: the title logo filling the width, the crown above it.
    let logo = load_png(&tex.join("logo.png"))?;
    let area = logo.content(0, 0, logo.w, logo.h);
    let mut banner = sky(256, 128);
    let scale = (240.0 / area.2 as f32).min(84.0 / area.3 as f32);
    let (w, h) = (area.2 as f32 * scale, area.3 as f32 * scale);
    let top = 128.0 - h - 10.0;
    draw(
        &mut banner,
        &logo,
        area,
        (((256.0 - w) / 2.0) as u32, top as u32),
        scale,
    );
    let cw = frame.2 as f32;
    draw(
        &mut banner,
        &crown,
        frame,
        (
            ((256.0 - cw) / 2.0) as u32,
            (top - frame.3 as f32 - 2.0).max(0.0) as u32,
        ),
        1.0,
    );
    save_png(&out.join("banner.png"), &banner)?;

    // Jingle: the clip at its original level, faded out at the cut.
    let path = assets.join(format!("audio/{JINGLE}.adp"));
    let a = Adp::from_bytes(&std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
        .ok_or_else(|| format!("{}: not a clip", path.display()))?;
    let mut pcm = adpcm::decode(&a.channels[0], a.samples as usize);
    pcm.truncate((a.rate as f32 * JINGLE_SECONDS) as usize);
    let fade = (a.rate as f32 * 0.3) as usize;
    let n = pcm.len();
    for (i, s) in pcm.iter_mut().enumerate() {
        let f = ((n - i) as f32 / fade as f32).min(1.0);
        *s = (*s as f32 * f * a.gain.max(0.5)) as i16;
    }
    std::fs::write(out.join("banner.wav"), wav(a.rate, &pcm)).map_err(|e| e.to_string())?;
    println!(
        "ciaart: icon.png, banner.png, banner.wav in {}",
        out.display()
    );
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(ripped), Some(assets), Some(out)) = (args.get(1), args.get(2), args.get(3)) else {
        eprintln!("usage: ciaart <ripped ExportedProject> <assets dir> <out dir>");
        std::process::exit(2);
    };
    if let Err(e) = run(Path::new(ripped), Path::new(assets), Path::new(out)) {
        eprintln!("ciaart: {e}");
        std::process::exit(1);
    }
}
