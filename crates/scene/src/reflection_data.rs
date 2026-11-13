//! Optional renderers on the PC reflection camera's extra layer.

use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::sheet::Sheet;

pub const FILE_NAME: &str = "reflection.bin";
const HEADER: &[u8; 5] = b"KNRE\x01";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReflectionPart {
    pub sprite: String,
    pub basis: [f32; 4],
    pub position: [f32; 3],
    pub tint: [f32; 4],
    pub flip_x: bool,
    pub flip_y: bool,
    pub sort_layer: i16,
    pub sort_order: i16,
    pub lit: bool,
    pub scaffold: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReflectionParts {
    /// Sparse entries indexed by the existing world prototype IDs.
    pub entries: Vec<(u16, Vec<ReflectionPart>)>,
}

impl ReflectionParts {
    pub fn encode(&self, world_id: u32) -> Result<Vec<u8>, String> {
        let mut bytes = HEADER.to_vec();
        bytes.extend_from_slice(&world_id.to_le_bytes());
        bytes.extend(postcard::to_allocvec(self).map_err(|e| e.to_string())?);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8], world_id: u32, proto_count: usize) -> Result<Self, String> {
        let len = HEADER.len() + 4;
        if bytes.len() < len
            || !bytes.starts_with(HEADER)
            || bytes[HEADER.len()..len] != world_id.to_le_bytes()
        {
            return Err("reflection data does not match this world pack".into());
        }
        let (def, tail): (Self, _) =
            postcard::take_from_bytes(&bytes[len..]).map_err(|e| e.to_string())?;
        if !tail.is_empty()
            || !def.entries.windows(2).all(|e| e[0].0 < e[1].0)
            || !def.entries.iter().all(|(id, parts)| {
                usize::from(*id) < proto_count
                    && (1..=64).contains(&parts.len())
                    && parts.iter().all(|part| {
                        !part.sprite.is_empty()
                            && part
                                .basis
                                .iter()
                                .chain(&part.position)
                                .all(|n| n.is_finite())
                            && part.tint.iter().all(|n| (0.0..=1.0).contains(n))
                    })
            })
        {
            return Err("invalid reflection parts".into());
        }
        Ok(def)
    }

    pub fn validate_sheet(&self, sheet: &Sheet) -> Result<(), String> {
        for (_, parts) in &self.entries {
            for part in parts {
                if sheet.id(&part.sprite).is_none() {
                    return Err(format!("missing reflection sprite {}", part.sprite));
                }
            }
        }
        Ok(())
    }

    pub fn load(
        path: &Path,
        world_id: u32,
        proto_count: usize,
        sheet: &Sheet,
    ) -> Result<Option<Self>, String> {
        match crate::authored::read_file(path) {
            Ok(bytes) => {
                let def = Self::decode(&bytes, world_id, proto_count)?;
                def.validate_sheet(sheet)?;
                Ok(Some(def))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub(crate) fn for_proto(&self, id: u16) -> &[ReflectionPart] {
        self.entries
            .binary_search_by_key(&id, |entry| entry.0)
            .map_or(&[], |index| &self.entries[index].1)
    }
}
