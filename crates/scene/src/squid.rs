//! Optional authored squid metadata indexed by the unchanged world.bin.
use game::data::WorldData;
use game::flight::AssetData;
use std::io::Read;
use std::path::Path;

pub const FILENAME: &str = "squid.bin";
const MAGIC: &[u8; 4] = b"KNSQ";
const VERSION: u16 = 1;
const HEADER: usize = 10;
const MAX_BYTES: usize = 256 * 1024;

pub fn encode(world: &WorldData, id: u32) -> Result<Vec<u8>, String> {
    let data = world.squid.as_ref().ok_or("no squid metadata")?;
    if !data.valid(world.protos.len()) {
        return Err("invalid squid metadata".into());
    }
    let payload = postcard::to_allocvec(data).map_err(|e| e.to_string())?;
    if payload.len() > MAX_BYTES - HEADER {
        return Err("squid metadata exceeds size limit".into());
    }
    let mut bytes = Vec::with_capacity(HEADER + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

fn decode(bytes: &[u8], world: &WorldData, id: u32) -> Result<AssetData, String> {
    if bytes.len() < HEADER
        || bytes.len() > MAX_BYTES
        || &bytes[..4] != MAGIC
        || bytes[4..6] != VERSION.to_le_bytes()
        || bytes[6..HEADER] != id.to_le_bytes()
    {
        return Err("incompatible squid metadata header".into());
    }
    let (data, rest): (AssetData, _) =
        postcard::take_from_bytes(&bytes[HEADER..]).map_err(|e| e.to_string())?;
    if !rest.is_empty() || !data.valid(world.protos.len()) {
        return Err("invalid squid metadata".into());
    }
    Ok(data)
}

/// A missing file is an older pack. Invalid data is reported to the caller
/// while the runtime can still use the original pack behavior.
fn read(dir: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match std::fs::File::open(dir.join(FILENAME)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(Some(bytes))
}

pub fn install(dir: &Path, world: &mut WorldData, id: u32) -> Result<(), String> {
    if let Some(bytes) = read(dir)? {
        world.squid = Some(decode(&bytes, world, id)?);
    }
    Ok(())
}

pub fn packed_bytes(dir: &Path, world: &WorldData, id: u32) -> Result<Option<Vec<u8>>, String> {
    let bytes = read(dir)?;
    if let Some(bytes) = &bytes {
        decode(bytes, world, id)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use game::data::ObstacleDef;

    #[test]
    fn squid_sidecar_checks_identity_and_preserves_legacy_world_bytes() {
        let mut world = WorldData {
            protos: vec![Default::default()],
            ..Default::default()
        };
        let legacy = postcard::to_allocvec(&world).unwrap();
        let id = game::save::fingerprint(&legacy);
        let legacy_world: WorldData = postcard::from_bytes(&legacy).unwrap();
        assert!(legacy_world.squid.is_none());
        world.squid = Some(AssetData {
            obstacles: vec![vec![ObstacleDef {
                origin: [1.0, 0.0],
                points: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            }]],
            ..Default::default()
        });
        let mut bytes = encode(&world, id).unwrap();
        assert_eq!(postcard::to_allocvec(&world).unwrap(), legacy);
        assert_eq!(
            decode(&bytes, &legacy_world, id).unwrap(),
            world.squid.unwrap()
        );
        assert!(decode(&bytes, &legacy_world, id.wrapping_add(1)).is_err());
        assert!(decode(&bytes[..bytes.len() - 1], &legacy_world, id).is_err());
        bytes.push(0);
        assert!(decode(&bytes, &legacy_world, id).is_err());
    }
}
