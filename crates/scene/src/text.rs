//! Text in the game's pixel font, drawn glyph by glyph.

use crate::render::{Color, Draw, Renderer};
use crate::sheet::Sheet;

/// Whole-pixel scale for a requested size (the font is drawn crisp).
fn scale(sheet: &Sheet, size: f32) -> f32 {
    (size / sheet.font.size.max(1.0)).round().max(1.0)
}

/// (width, height) of `s` at `size`.
pub fn measure(sheet: &Sheet, s: &str, size: f32) -> (f32, f32) {
    let k = scale(sheet, size);
    let w: f32 = s
        .chars()
        .map(|c| sheet.glyph(c).map_or(sheet.font.size * 0.4, |g| g.1))
        .sum();
    (w * k, sheet.font.size * 0.6 * k)
}

/// Draws `s` with its baseline at `y`, starting at `x`.
pub fn draw(r: &mut impl Renderer, sheet: &Sheet, s: &str, x: f32, y: f32, size: f32, c: Color) {
    let k = scale(sheet, size);
    let mut pen = x.round();
    for ch in s.chars() {
        let Some((sprite, advance)) = sheet.glyph(ch) else {
            pen += sheet.font.size * 0.4 * k;
            continue;
        };
        if let Some(id) = sprite {
            r.sprite(id, (pen, y.round()), Draw::tint(c).scale(k));
        }
        pen += advance * k;
    }
}
