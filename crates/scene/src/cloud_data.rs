//! Optional sky-cloud presentation data, separate from the save's world table.

use std::io::ErrorKind;
use std::path::Path;

use game::data::{Curve, SpriteId};
use serde::{Deserialize, Serialize};

use crate::sheet::Sheet;

pub const FILE_NAME: &str = "clouds.bin";
const HEADER: &[u8; 5] = b"KNLC\x01";

pub fn encode(def: &CloudsDef, world_id: u32) -> Result<Vec<u8>, String> {
    if !def.valid() {
        return Err("invalid sky cloud definitions".into());
    }
    let mut bytes = HEADER.to_vec();
    bytes.extend_from_slice(&world_id.to_le_bytes());
    bytes.extend(postcard::to_allocvec(def).map_err(|error| error.to_string())?);
    Ok(bytes)
}

pub fn decode(bytes: &[u8], world_id: u32, sheet: &Sheet) -> Result<CloudsDef, String> {
    let id = world_id.to_le_bytes();
    let len = HEADER.len() + id.len();
    if bytes.len() < len || !bytes.starts_with(HEADER) || bytes[HEADER.len()..len] != id {
        return Err("sky cloud data does not match this world pack".into());
    }
    let (def, tail): (CloudsDef, _) =
        postcard::take_from_bytes(&bytes[len..]).map_err(|error| error.to_string())?;
    if !tail.is_empty() || !def.valid() || def.sprites.iter().any(|name| sheet.id(name).is_none()) {
        return Err("invalid sky cloud definitions or missing sprites".into());
    }
    Ok(def)
}

pub fn load(path: &Path, world_id: u32, sheet: &Sheet) -> Result<Option<CloudsDef>, String> {
    match crate::authored::read_file(path) {
        Ok(bytes) => decode(&bytes, world_id, sheet).map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CloudsDef {
    pub stratus: CloudLayerDef,
    pub cumulus: CloudLayerDef,
    /// orig: `World.windChangeCooldown`.
    pub wind_change_cooldown: f32,
    /// Scene-local sprites resolved by atlas name.
    pub sprites: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CloudLayerDef {
    /// Controller origin in Unity world units. Cloud depth adds to Z.
    pub origin: [f32; 3],
    pub depth: f32,
    /// Distance from the camera-following origin to either horizontal wrap edge.
    pub width: f32,
    pub wind_multiplier: f32,
    pub lifetime: f32,
    pub fade_time: f32,
    pub vertical_motion: f32,
    pub max_clouds: u16,
    /// `RandomizeSprite.sprites`, in authored order, with the cloud shader's
    /// coverage baked into alpha. Sample these with nearest filtering.
    pub sprites: Vec<SpriteId>,
    pub prefab_active: bool,
    pub allow_flip: bool,
    pub y_curve: Curve,
    pub scale_curve: Curve,
    pub parallax_curve: Curve,
}

impl CloudsDef {
    pub fn sprite_ids(&self) -> impl Iterator<Item = SpriteId> + '_ {
        self.stratus
            .sprites
            .iter()
            .chain(&self.cumulus.sprites)
            .copied()
    }

    pub fn valid(&self) -> bool {
        let sprite_count = self.sprites.len();
        (1..=32).contains(&sprite_count)
            && self
                .sprites
                .iter()
                .enumerate()
                .all(|(i, name)| !name.is_empty() && !self.sprites[..i].contains(name))
            && self.wind_change_cooldown > 0.0
            && self.wind_change_cooldown <= 3600.0
            && self.stratus.valid(sprite_count)
            && self.cumulus.valid(sprite_count)
    }
}

impl CloudLayerDef {
    pub fn valid(&self, sprite_count: usize) -> bool {
        self.valid_with_parallax(sprite_count, 1.0)
    }

    pub fn valid_with_parallax(&self, sprite_count: usize, max_parallax: f32) -> bool {
        let curve_valid = |curve: &Curve, min, max| {
            (1..=16).contains(&curve.keys.len())
                && curve.keys.iter().all(|&[t, v, incoming, outgoing]| {
                    (0.0..=1.0).contains(&t)
                        && (min..=max).contains(&v)
                        && [incoming, outgoing]
                            .into_iter()
                            .all(|s| s.is_infinite() || s.abs() <= 100.0)
                })
                && curve.keys.windows(2).all(|k| k[0][0] < k[1][0])
        };
        self.origin.iter().all(|v| v.abs() <= 1000.0)
            && (0.01..=100.0).contains(&self.depth)
            && (0.01..=100.0).contains(&self.width)
            && self.wind_multiplier.abs() <= 10.0
            && (0.01..=60.0).contains(&self.fade_time)
            && (2.0 * self.fade_time..=7200.0).contains(&self.lifetime)
            && self.vertical_motion.abs() <= 10.0
            && (1..=256).contains(&self.max_clouds)
            && (1..=16).contains(&self.sprites.len())
            && self
                .sprites
                .iter()
                .all(|&id| usize::from(id) < sprite_count)
            && curve_valid(&self.y_curve, -100.0, 100.0)
            && curve_valid(&self.scale_curve, 0.01, 10.0)
            && curve_valid(&self.parallax_curve, 0.0, max_parallax)
    }
}
