//! The game's pixel font (`Resources/fonts/Kingdom`): its baked glyph
//! texture cut into one sprite per character, plus metrics in `font.json`.
//! orig: Unity `Font` (`m_CharacterRects`, `m_LineSpacing`) and its Alpha8 texture.

use std::path::Path;

use serde::Serialize;

use crate::sprites::{Image, Sprite};

#[derive(Serialize)]
pub struct FontOut {
    /// orig: `m_FontSize`.
    pub size: f32,
    /// orig: `m_LineSpacing`.
    pub line: f32,
    /// (character, sprite name, advance in pixels).
    pub glyphs: Vec<(char, String, f32)>,
}

fn after<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let i = text.find(key)? + key.len();
    Some(text[i..].trim_start())
}

fn num(text: &str, key: &str) -> f32 {
    after(text, key)
        .and_then(|v| v.split(['\n', ',', '}']).next())
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0.0)
}

/// Cuts `font` (e.g. `Kingdom`) out of `fonts_dir`.
pub fn extract(
    fonts_dir: &Path,
    font: &str,
    texture: &str,
) -> Result<(FontOut, Vec<Sprite>), String> {
    let read = |name: &str| {
        std::fs::read_to_string(fonts_dir.join(name)).map_err(|e| format!("{name}: {e}"))
    };
    let f = read(&format!("{font}.asset"))?;
    let t = read(&format!("{texture}.texture2D"))?;
    let (tw, th) = (num(&t, "m_Width:") as u32, num(&t, "m_Height:") as u32);
    if num(&t, "m_TextureFormat:") as u32 != 1 {
        return Err(format!("{texture}: expected an Alpha8 texture"));
    }
    let hex = after(&t, "_typelessdata:").ok_or("font texture has no data")?;
    let hex = hex.split_whitespace().next().unwrap_or("");
    let alpha: Vec<u8> = (0..hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
        .collect();
    if alpha.len() < (tw * th) as usize {
        return Err(format!("{texture}: short image data"));
    }
    let mut out = FontOut {
        size: num(&f, "m_FontSize:"),
        line: num(&f, "m_LineSpacing:"),
        glyphs: Vec::new(),
    };
    let mut sprites = Vec::new();
    let rects = f.split("- serializedVersion: 2\n    index:").skip(1);
    for r in rects {
        let index = r
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u32>().ok());
        let Some(ch) = index.and_then(char::from_u32) else {
            continue;
        };
        let (uv, rest) = r.split_once("vert:").unwrap_or((r, ""));
        let (ux, uy, uw, uh) = (
            num(uv, "x:"),
            num(uv, "y:"),
            num(uv, "width:"),
            num(uv, "height:"),
        );
        let (vx, vy) = (num(rest, "x:"), num(rest, "y:"));
        let advance = num(rest, "advance:");
        let name = format!("font_{}", ch as u32);
        // uv y is from the bottom; a negative height runs downward.
        let x0 = (ux * tw as f32).round() as i64;
        let w = (uw * tw as f32).round().abs() as u32;
        let (ya, yb) = (uy * th as f32, (uy + uh) * th as f32);
        let (bottom, top) = (ya.min(yb).round() as i64, ya.max(yb).round() as i64);
        let h = (top - bottom).max(0) as u32;
        let mut px = vec![0u8; (w * h * 4) as usize];
        for row in 0..h {
            // The glyph rect runs downward from its uv top (bottom + row).
            let ty = bottom + row as i64;
            for col in 0..w {
                let tx = x0 + col as i64;
                if tx < 0 || ty < 0 || tx >= tw as i64 || ty >= th as i64 {
                    continue;
                }
                let a = alpha[(ty as u32 * tw + tx as u32) as usize];
                let o = ((row * w + col) * 4) as usize;
                px[o..o + 4].copy_from_slice(&[255, 255, 255, a]);
            }
        }
        // orig: CharacterInfo.flipped: stored turned a quarter in the atlas.
        let (w, h, px) = if r.contains("flipped: 1") {
            let mut t = vec![0u8; px.len()];
            for row in 0..h {
                for col in 0..w {
                    let (src, dst) = (
                        ((row * w + col) * 4) as usize,
                        ((col * h + row) * 4) as usize,
                    );
                    t[dst..dst + 4].copy_from_slice(&px[src..src + 4]);
                }
            }
            (h, w, t)
        } else {
            (w, h, px)
        };
        out.glyphs.push((ch, name.clone(), advance));
        if w == 0 || h == 0 {
            continue;
        }
        sprites.push(Sprite {
            name,
            img: Image { w, h, px },
            // vert.x/y: the glyph's top-left from the pen position, y up.
            pivot: (-vx, vy),
            ppu: 1.0,
        });
    }
    Ok((out, sprites))
}
