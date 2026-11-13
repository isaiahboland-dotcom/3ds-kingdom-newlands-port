//! Deterministic gameplay regression and original-game parity framework.
//! See `docs/regression/ARCHITECTURE.md`.
//!
//! Gameplay fidelity comes before optimization: an optimization must leave
//! these scenarios' outcomes as they were.

pub mod golden;
pub mod meta;
pub mod orig;
pub mod reference;
pub mod replay;
pub mod scenario;
pub mod scenarios;
pub mod script;
pub mod sim;
pub mod snapshot;
pub mod tod;
pub mod worlds;

use game::data::WorldData;

/// Loads a converted `world.bin` and its fingerprint.
pub fn load_world(assets: &std::path::Path) -> Result<(WorldData, u32), String> {
    let path = assets.join("world.bin");
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let id = game::save::fingerprint(&bytes);
    let mut world = postcard::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let path = assets.join(scene::authored::FILENAME);
    match scene::authored::read_file(&path) {
        Ok(data) => scene::authored::AssetAnimation::install_gameplay(&data, id, &mut world)
            .map_err(|e| format!("{}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    Ok((world, id))
}
