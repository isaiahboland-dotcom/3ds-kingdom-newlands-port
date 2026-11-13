//! Optional presentation data. Its header identifies the legacy world whose
//! prototype and part IDs it extends. Saves keep using that world's bytes.

use game::animation::{Library, Pose};
use game::data::WorldData;
use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use std::path::Path;

use crate::sheet::Sheet;

pub const FILENAME: &str = "animation.bin";
const MAGIC: [u8; 4] = *b"KNAF";
const VERSION: u16 = 3;
const HEADER: usize = 10;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_DEPTH: usize = 64;

/// Bound the optional file read before allocating for its payload.
pub fn read_file(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AssetAnimation {
    pub prefabs: Vec<PrefabVisual>,
    pub clips: Library,
    pub catapults: Vec<(u16, game::siege::CatapultPhysics)>,
    pub boss_throw: Option<game::data::BossThrowDef>,
    pub boulder_walls: Vec<(u16, Vec<game::data::BoulderWallPolygon>)>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PrefabVisual {
    pub nodes: Vec<TransformNode>,
    pub parts: Vec<PartVisual>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransformNode {
    pub path: String,
    pub parent: Option<u16>,
    pub local: Pose,
    pub animator: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartVisual {
    pub node: u16,
    pub flip_x: bool,
    pub flip_y: bool,
}

pub use game::animation::Affine2;

impl PrefabVisual {
    pub fn sampled_transform(
        &self,
        mut index: u16,
        mut sample: impl FnMut(u16, &TransformNode) -> Pose,
    ) -> Option<(Affine2, Pose)> {
        let mut chain = [0; MAX_DEPTH];
        let mut count = 0;
        loop {
            *chain.get_mut(count)? = index;
            count += 1;
            let Some(parent) = self.nodes.get(index as usize)?.parent else {
                break;
            };
            index = parent;
        }
        let mut transform = Affine2::IDENTITY;
        let mut pose = Pose::default();
        for &i in chain[..count].iter().rev() {
            pose = sample(i, &self.nodes[i as usize]);
            transform = transform.compose(Affine2::from_pose(pose));
        }
        transform.finite().then_some((transform, pose))
    }

    fn validate(&self, part_count: usize) -> bool {
        if self.parts.len() != part_count
            || self
                .parts
                .iter()
                .any(|p| p.node as usize >= self.nodes.len())
        {
            return false;
        }
        let mut rest: Vec<(Affine2, usize)> = Vec::with_capacity(self.nodes.len());
        for (i, node) in self.nodes.iter().enumerate() {
            let (parent, depth) = match node.parent {
                Some(parent) if (parent as usize) < i => rest[parent as usize],
                None if i == 0 && node.path.is_empty() => (Affine2::IDENTITY, 0),
                _ => return false,
            };
            let p = node.local;
            let transform = parent.compose(Affine2::from_pose(p));
            if depth >= MAX_DEPTH
                || !transform.finite()
                || !p
                    .position
                    .into_iter()
                    .chain(p.rotation)
                    .chain(p.scale)
                    .chain(p.color)
                    .all(f32::is_finite)
            {
                return false;
            }
            rest.push((transform, depth + 1));
        }
        true
    }
}

/// Animator clip paths are relative to the GameObject carrying the Animator.
pub(crate) fn animation_path<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    if root.is_empty() {
        Some(path)
    } else if path == root {
        Some("")
    } else {
        path.strip_prefix(root)?.strip_prefix('/')
    }
}

impl AssetAnimation {
    pub fn encode(&self, world_id: u32) -> Result<Vec<u8>, String> {
        let body = postcard::to_allocvec(self).map_err(|e| e.to_string())?;
        if body.len() > MAX_BYTES - HEADER {
            return Err("animation data exceeds the size limit".into());
        }
        let mut bytes = Vec::with_capacity(HEADER + body.len());
        bytes.extend(MAGIC);
        bytes.extend(VERSION.to_le_bytes());
        bytes.extend(world_id.to_le_bytes());
        bytes.extend(body);
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8], world_id: u32) -> Result<Self, String> {
        if !(HEADER..=MAX_BYTES).contains(&bytes.len()) || bytes[..4] != MAGIC {
            return Err("unsupported animation data format".into());
        }
        if bytes[6..10] != world_id.to_le_bytes() {
            return Err("animation data belongs to a different world.bin".into());
        }
        let (authored, rest) = match u16::from_le_bytes([bytes[4], bytes[5]]) {
            1 => {
                let ((prefabs, clips), rest) =
                    postcard::take_from_bytes(&bytes[HEADER..]).map_err(|e| e.to_string())?;
                (
                    Self {
                        prefabs,
                        clips,
                        ..Self::default()
                    },
                    rest,
                )
            }
            2 => {
                let ((prefabs, clips, catapults), rest) =
                    postcard::take_from_bytes(&bytes[HEADER..]).map_err(|e| e.to_string())?;
                (
                    Self {
                        prefabs,
                        clips,
                        catapults,
                        ..Self::default()
                    },
                    rest,
                )
            }
            VERSION => postcard::take_from_bytes(&bytes[HEADER..]).map_err(|e| e.to_string())?,
            _ => return Err("unsupported animation data version".into()),
        };
        if !rest.is_empty() {
            return Err("trailing bytes in animation data".into());
        }
        Ok(authored)
    }

    /// Install optional simulation data only after checking its world identity
    /// and prototype references. Used by headless runners with no sprite sheet.
    pub fn install_gameplay(bytes: &[u8], id: u32, world: &mut WorldData) -> Result<(), String> {
        let data = Self::decode(bytes, id)?;
        data.validate_gameplay(world)?;
        world.catapult_physics = data.catapults;
        world.boss_throw = data.boss_throw;
        world.boulder_walls = data.boulder_walls;
        world.boss_bash_impact = data
            .clips
            .clip("boss_bash_right")
            .and_then(|clip| clip.first_event_time(game::animation::EventKind::BashImpact));
        Ok(())
    }

    fn validate_gameplay(&self, world: &WorldData) -> Result<(), String> {
        if self.catapults.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || self.catapults.iter().any(|(id, def)| {
                !def.valid()
                    || world
                        .protos
                        .get(*id as usize)
                        .is_none_or(|proto| proto.catapult.is_none())
            })
        {
            return Err("invalid authored catapult data".into());
        }
        if self.boss_throw.as_ref().is_some_and(|def| !def.valid())
            || self
                .boulder_walls
                .windows(2)
                .any(|pair| pair[0].0 >= pair[1].0)
            || self.boulder_walls.iter().any(|(id, polygons)| {
                world
                    .protos
                    .get(*id as usize)
                    .is_none_or(|proto| proto.wall_def.is_none())
                    || polygons.is_empty()
                    || polygons.iter().any(|polygon| {
                        polygon.len() < 3
                            || polygon
                                .iter()
                                .flatten()
                                .any(|v| v.abs() > 100.0 || !v.is_finite())
                    })
            })
        {
            return Err("invalid authored boss boulder data".into());
        }
        Ok(())
    }

    pub(crate) fn validate(&self, world: &WorldData, sheet: &Sheet) -> Result<(), String> {
        if self.prefabs.len() != world.protos.len() {
            return Err("animation data has the wrong number of prototypes".into());
        }
        if let Some((i, _)) = self
            .prefabs
            .iter()
            .zip(&world.protos)
            .enumerate()
            .find(|(_, (visual, proto))| !visual.validate(proto.parts.len()))
        {
            return Err(format!("invalid authored hierarchy for prototype {i}"));
        }
        if !self.clips.validate()
            || self.clips.clips.iter().any(|(_, clip)| {
                clip.sprites.iter().any(|track| {
                    track
                        .keys
                        .iter()
                        .filter_map(|(_, name)| name.as_deref())
                        .any(|name| sheet.id(name).is_none())
                })
            })
        {
            return Err("animation data contains an invalid clip or sprite reference".into());
        }
        game::greed::validate_portal_graph(&self.clips)?;
        self.validate_gameplay(world)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::{Clip, Sprite};
    use game::animation::SpriteTrack;

    #[test]
    fn optional_data_preserves_sheet_bytes_and_rejects_stale_or_bad_payloads() {
        let world = WorldData::default();
        let mut sheet = Sheet::default();
        sheet.names.push("old".into());
        sheet.sprites.push(Sprite {
            page: 0,
            x: 0,
            y: 0,
            w: 2,
            h: 2,
            px: 1.0,
            py: 1.0,
            masks: [None, None],
        });
        sheet.clips.push((
            "motion".into(),
            Clip {
                length: 1.0,
                sprites: vec![("".into(), vec![(0.0, 0)])],
                ..Default::default()
            },
        ));
        sheet.link(&[]);
        assert_eq!(sheet.frame("motion", "", 0.75), Some(Some(0)));
        let legacy = postcard::to_allocvec(&sheet).unwrap();

        let mut extension = AssetAnimation::default();
        extension.clips.clips.push((
            "motion".into(),
            game::animation::Clip {
                length: 1.0,
                sprites: vec![SpriteTrack {
                    path: "".into(),
                    keys: vec![(0.0, Some("old".into())), (0.5, None)],
                }],
                ..Default::default()
            },
        ));
        let data = extension.encode(17).unwrap();
        sheet.install_authored(&data, 17, &world).unwrap();
        assert_eq!(sheet.frame("motion", "", 0.25), Some(Some(0)));
        assert_eq!(sheet.frame("motion", "", 0.75), Some(None));
        assert_eq!(postcard::to_allocvec(&sheet).unwrap(), legacy);
        assert!(sheet.install_authored(&data, 19, &world).is_err());
        assert_eq!(sheet.frame("motion", "", 0.75), Some(Some(0)));

        let mut wrong_version = data.clone();
        wrong_version[4] += 1;
        let mut trailing = data.clone();
        trailing.push(0);
        for malformed in [&wrong_version[..], &trailing[..], &data[..data.len() - 1]] {
            assert!(sheet.install_authored(malformed, 17, &world).is_err());
            assert!(sheet.authored().is_none());
        }

        extension.clips.clips[0].1.sprites[0].keys[0].1 = Some("other-pack".into());
        assert!(sheet
            .install_authored(&extension.encode(17).unwrap(), 17, &world)
            .is_err());
        assert_eq!(sheet.frame("motion", "", 0.75), Some(Some(0)));
    }

    #[test]
    fn hierarchy_validation_rejects_invalid_nodes_before_rendering() {
        let mut graph = PrefabVisual {
            nodes: vec![
                TransformNode {
                    path: "".into(),
                    parent: None,
                    local: Pose::default(),
                    animator: None,
                },
                TransformNode {
                    path: "Part".into(),
                    parent: Some(0),
                    local: Pose::default(),
                    animator: None,
                },
            ],
            parts: vec![PartVisual {
                node: 1,
                flip_x: false,
                flip_y: false,
            }],
        };
        assert!(graph.validate(1));
        assert!(!graph.validate(0));
        graph.nodes[1].parent = Some(1);
        assert!(!graph.validate(1));
        graph.nodes[1].parent = Some(0);
        graph.nodes[1].local.scale[0] = f32::NAN;
        assert!(!graph.validate(1));
        graph.nodes[1].local = Pose::default();
        graph.parts[0].node = 2;
        assert!(!graph.validate(1));
    }
}
