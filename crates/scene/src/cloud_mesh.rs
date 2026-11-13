//! Authored Unity cloud sprite geometry, kept separately from the packed sheet.

use serde::{Deserialize, Serialize};

use crate::sheet::Sprite;

pub const FILE_NAME: &str = "cloud-meshes.bin";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloudMesh {
    /// Pixel offsets from the authored pivot, with Y down.
    /// Add the sheet sprite's pivot to obtain its local atlas coordinates.
    pub vertices: Vec<[f32; 2]>,
    /// Unity m_IndexBuffer triangle order.
    pub indices: Vec<u16>,
}

impl CloudMesh {
    pub(crate) fn valid_for(&self, sprite: &Sprite) -> bool {
        self.valid_geometry(false) && self.fits(sprite)
    }

    pub(crate) fn fits(&self, sprite: &Sprite) -> bool {
        self.vertices.iter().all(|[x, y]| {
            (0.0..=f32::from(sprite.w)).contains(&(x + sprite.px))
                && (0.0..=f32::from(sprite.h)).contains(&(y + sprite.py))
        })
    }

    pub(crate) fn valid_geometry(&self, allow_collinear: bool) -> bool {
        let mut visible = false;
        (3..=64).contains(&self.vertices.len())
            && (3..=192).contains(&self.indices.len())
            && self.indices.len().is_multiple_of(3)
            && self.vertices.iter().flatten().all(|v| v.is_finite())
            && self.indices.as_chunks::<3>().0.iter().all(|&triangle| {
                let [Some(a), Some(b), Some(c)] =
                    triangle.map(|i| self.vertices.get(usize::from(i)))
                else {
                    return false;
                };
                let has_area = (b[0] - a[0]) * (c[1] - a[1]) != (b[1] - a[1]) * (c[0] - a[0]);
                visible |= has_area;
                let [i, j, k] = triangle;
                has_area || (allow_collinear && i != j && j != k && k != i)
            })
            && visible
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudMeshes {
    /// Atlas names, stable across desktop extraction and 3DS packing.
    pub entries: Vec<(String, CloudMesh)>,
}

const HEADER: &[u8; 5] = b"KNSM\x01";

impl CloudMeshes {
    pub fn encode(&self, world_id: u32) -> Result<Vec<u8>, String> {
        let mut bytes = HEADER.to_vec();
        bytes.extend_from_slice(&world_id.to_le_bytes());
        bytes.extend(postcard::to_allocvec(self).map_err(|e| e.to_string())?);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8], world_id: u32) -> Result<Self, String> {
        let len = HEADER.len() + 4;
        if bytes.len() < len
            || !bytes.starts_with(HEADER)
            || bytes[HEADER.len()..len] != world_id.to_le_bytes()
        {
            return Err("cloud mesh data does not match this world pack".into());
        }
        let (meshes, tail): (Self, _) =
            postcard::take_from_bytes(&bytes[len..]).map_err(|e| e.to_string())?;
        if !tail.is_empty() || meshes.entries.len() > 32 {
            return Err("invalid cloud mesh data".into());
        }
        Ok(meshes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::Sheet;

    #[test]
    fn validates_full_mesh_set_and_keeps_legacy_sheet_encoding() {
        let mut sheet = Sheet::default();
        sheet.names = vec!["cumulus".into(), "stratus".into()];
        sheet.sprites = vec![
            Sprite {
                page: 0,
                x: 10,
                y: 20,
                w: 10,
                h: 12,
                px: 3.0,
                py: 4.0,
                masks: [None; 2]
            };
            2
        ];
        sheet.link(&[]);
        let original = postcard::to_allocvec(&sheet).unwrap();
        let stratus = CloudMesh {
            vertices: vec![[-3., 0.], [7., -4.], [5., 8.]],
            indices: vec![0, 2, 1],
        };
        let cumulus = CloudMesh {
            vertices: vec![[-2., 1.], [6., -3.], [5., 7.]],
            indices: vec![1, 2, 0],
        };
        let names: Vec<String> = vec!["stratus".into(), "cumulus".into()];
        let mut meshes = CloudMeshes {
            entries: vec![
                (names[0].clone(), stratus.clone()),
                (names[1].clone(), cumulus.clone()),
            ],
        };
        sheet
            .link_cloud_meshes(&names, 7, &meshes.encode(7).unwrap())
            .unwrap();
        assert_eq!(sheet.cloud_mesh(0), Some(&cumulus));
        assert_eq!(sheet.cloud_mesh(1), Some(&stratus));
        assert_eq!(postcard::to_allocvec(&sheet).unwrap(), original);
        assert!(sheet
            .link_cloud_meshes(&names, 8, &meshes.encode(7).unwrap())
            .is_err());
        meshes.entries[0].1.indices[0] = 3;
        assert!(sheet
            .link_cloud_meshes(&names, 7, &meshes.encode(7).unwrap())
            .is_err());
        meshes.entries.pop();
        assert!(sheet
            .link_cloud_meshes(&names, 7, &meshes.encode(7).unwrap())
            .is_err());
        assert_eq!(sheet.cloud_mesh(0), Some(&cumulus));
    }
}
