//! The coat of arms' sprites and the monarch's rider, shared by the world
//! (banners, sails, the monarch) and the banner editor on the touch screen.
//!
//! orig: `CoatOfArms.GetBannerSprite()`, `GetEmblemSprite()`, `Banner`,
//! and the steed's `King` / `Queen` children.

use game::kingdom::Look;

use crate::sheet::Sheet;

/// Cloth and emblem sprites, by index (`Holder.patterns`, `Holder.emblems`).
pub const PATTERN: &str = "banner_1_pattern_";
pub const EMBLEM: &str = "banner_1_emblem_";
/// The border banner's icon takes the emblem color but keeps its sprite.
/// orig: Border prefab, `Banner.setEmblem` off.
pub const BORDER_ICON: &str = "border_banner_icon";

/// A `Banner`'s cloth: the castle banners' pattern, the border banner and
/// the boat's sail, in the main and trim colors.
pub fn is_cloth(sprite: &str) -> bool {
    sprite.starts_with(PATTERN) || sprite == "border_banner" || sprite.starts_with("boat_sail_")
}

pub fn pattern(sheet: &Sheet, look: Look) -> Option<u32> {
    sheet.id(&format!("{PATTERN}{}", look.pattern))
}

pub fn emblem(sheet: &Sheet, look: Look) -> Option<u32> {
    sheet.id(&format!("{EMBLEM}{}", look.emblem))
}

/// The steed's rider child: its clip track, its sprite when a clip doesn't
/// key one, the crown's track and the crown sprite.
/// orig: Horse Regular prefab, `King`/`Queen` SpriteRenderers and their
/// `Crown`s (`HatPreset.kingSprite` / `queenSprite`).
pub struct Rider {
    pub track: &'static str,
    pub default: &'static str,
    pub crown_track: &'static str,
    pub crown: &'static str,
}

pub fn rider(look: Look) -> Rider {
    if look.queen {
        Rider {
            track: "Queen",
            default: "player_queen_idle_10",
            crown_track: "Queen/Crown",
            crown: "crowns_2",
        }
    } else {
        Rider {
            track: "King",
            default: "player_king_eat_0",
            crown_track: "King/Crown",
            crown: "crowns_1",
        }
    }
}

/// The editor's color picker: 16 columns, 3 rows.
pub const PALETTE_COLS: usize = 16;
pub const PALETTE_ROWS: usize = 3;

/// Color `i` of the picker: two rows of hues at the original's two banner
/// shades (`ColorFunctions.RandomBannerColor`: 0.6/0.8 and 0.7/0.6), then
/// grays, its black, white and gold, and a few earth and pale tones.
pub fn palette(i: usize) -> [f32; 3] {
    let (row, col) = (i / PALETTE_COLS, i % PALETTE_COLS);
    let h = col as f32 / PALETTE_COLS as f32;
    match row {
        0 => game::units::hsb(h, 0.6, 0.8),
        1 => game::units::hsb(h, 0.7, 0.6),
        _ => [
            [0.1; 3],
            [0.25; 3],
            [0.4; 3],
            [0.55; 3],
            [0.7; 3],
            [0.85; 3],
            [0.95; 3],
            [1.0, 0.77, 0.3],
            [1.0, 0.89, 0.0],
            [0.55, 0.35, 0.2],
            [0.36, 0.22, 0.13],
            [0.82, 0.71, 0.55],
            [1.0, 0.75, 0.75],
            [0.75, 0.9, 1.0],
            [0.8, 1.0, 0.8],
            [0.9, 0.8, 1.0],
        ][col],
    }
}
