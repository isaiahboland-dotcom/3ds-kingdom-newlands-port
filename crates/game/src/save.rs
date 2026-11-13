//! Saving and loading: the whole island as it stands, and the unlocks kept
//! across runs, as bytes for the backend to store.
//!
//! orig: `PersistentManager` (the `AUTO` slot, `storage_v34_AUTO.dat`) and
//! `UnlockManager` (the `unlocks` file). The original saves each object's
//! `IPersistent` data and rebuilds the scene from prefabs; we snapshot the
//! simulation state instead, which resumes exactly where it left off.

use alloc::vec::Vec;

use crate::progress::Progress;
use crate::state::GameState;

/// Bumped whenever the saved state's layout changes.
pub const VERSION: u16 = 5;
const MAGIC: [u8; 4] = *b"KNL1";
const EXTENSIONS: [u8; 4] = *b"KNEX";
const PORTALS: [u8; 4] = *b"PORT";
const CATAPULTS: [u8; 4] = *b"CATA";

/// The slot names the backends store under. orig: `"AUTO"`, `"unlocks"`.
pub const GAME_SLOT: &str = "game";
pub const UNLOCKS_SLOT: &str = "unlocks";
/// Which game slot is in use (one byte).
pub const CURRENT_SLOT: &str = "slot";
/// Game save slots. The first keeps `GAME_SLOT`'s name, so older saves load.
pub const SLOTS: usize = 3;

/// The storage name of game slot `i` (0-based).
pub fn game_slot(i: usize) -> alloc::string::String {
    if i == 0 {
        GAME_SLOT.into()
    } else {
        alloc::format!("{GAME_SLOT}{}", i + 1)
    }
}

/// Why a save couldn't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Not a save, or from another version of the game.
    Format,
    /// Made with different game data (`world.bin`); ids wouldn't line up.
    World,
    /// Damaged. orig: "Save file corrupted".
    Corrupt,
}

/// A cheap fingerprint of the game data (FNV-1a over `world.bin`). Saves
/// keep it, since they refer to prototypes and sprites by index.
pub fn fingerprint(world_bin: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in world_bin {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

fn header(world: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&world.to_le_bytes());
    out
}

fn body(bytes: &[u8], world: Option<u32>) -> Result<(u16, &[u8]), LoadError> {
    if bytes.len() < 10 || bytes[..4] != MAGIC {
        return Err(LoadError::Format);
    }
    let stored = u16::from_le_bytes([bytes[4], bytes[5]]);
    if stored == 0 || stored > VERSION {
        return Err(LoadError::Format);
    }
    let w = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);
    if world.is_some_and(|x| x != w) {
        return Err(LoadError::World);
    }
    Ok((stored, &bytes[10..]))
}

/// orig: `PersistentManager.Save()`.
pub fn save_game(g: &GameState, world: u32) -> Vec<u8> {
    let mut out = header(world);
    // Serializing plain data into a Vec can't fail.
    out.extend(postcard::to_allocvec(g).expect("plain-data serialization cannot fail"));
    let recoil = g.siege.save_recoil();
    if !g.greed.portal_visuals().is_empty() || recoil.is_some() {
        out.extend_from_slice(&EXTENSIONS);
        if !g.greed.portal_visuals().is_empty() {
            write_extension(&mut out, PORTALS, g.greed.portal_visuals());
        }
        if let Some(recoil) = recoil {
            write_extension(&mut out, CATAPULTS, &recoil);
        }
    }
    out
}

fn write_extension<T: serde::Serialize + ?Sized>(out: &mut Vec<u8>, tag: [u8; 4], value: &T) {
    let payload = postcard::to_allocvec(value).expect("plain-data serialization cannot fail");
    let length = u32::try_from(payload.len()).expect("save extension exceeds 4 GiB");
    out.extend_from_slice(&tag);
    out.extend_from_slice(&length.to_le_bytes());
    out.extend(payload);
}

fn extension<T: serde::de::DeserializeOwned>(data: &[u8]) -> Result<T, LoadError> {
    let (value, rest) = postcard::take_from_bytes(data).map_err(|_| LoadError::Corrupt)?;
    if rest.is_empty() {
        Ok(value)
    } else {
        Err(LoadError::Corrupt)
    }
}

fn load_extensions(
    game: &mut GameState,
    world: &crate::data::WorldData,
    data: &[u8],
) -> Result<(), LoadError> {
    if data.is_empty() {
        return Ok(());
    }
    let mut rest = data
        .strip_prefix(&EXTENSIONS)
        .filter(|data| !data.is_empty())
        .ok_or(LoadError::Corrupt)?;
    let mut seen_portals = false;
    let mut seen_catapults = false;
    while !rest.is_empty() {
        let (header, data) = rest.split_at_checked(8).ok_or(LoadError::Corrupt)?;
        let length = u32::from_le_bytes(header[4..].try_into().expect("header size")) as usize;
        let (payload, next) = data.split_at_checked(length).ok_or(LoadError::Corrupt)?;
        match &header[..4] {
            tag if tag == PORTALS && !seen_portals => {
                let visuals = extension(payload)?;
                if !game
                    .greed
                    .restore_portal_visuals(visuals, world, &game.level)
                {
                    return Err(LoadError::Corrupt);
                }
                seen_portals = true;
            }
            tag if tag == CATAPULTS && !seen_catapults => {
                let (states, clocks) = extension(payload)?;
                if !game
                    .siege
                    .restore_recoil(states, clocks, world, &game.level)
                {
                    return Err(LoadError::Corrupt);
                }
                seen_catapults = true;
            }
            tag if tag == PORTALS || tag == CATAPULTS => return Err(LoadError::Corrupt),
            _ => return Err(LoadError::Format),
        }
        rest = next;
    }
    Ok(())
}

/// orig: `PersistentManager.Load()`.
pub fn load_game(
    bytes: &[u8],
    world_id: u32,
    world: &crate::data::WorldData,
) -> Result<GameState, LoadError> {
    let (stored, body) = body(bytes, Some(world_id))?;
    let look = postcard::to_allocvec(&crate::kingdom::Look::default())
        .expect("plain-data serialization cannot fail");
    let tutorial = postcard::to_allocvec(&crate::tutorial::Tutorial::default())
        .expect("plain-data serialization cannot fail");
    // Saved before the last fields were added: the look (a king with the
    // first pattern and emblem), and before that the tutorial (it sets
    // itself up on the first tick).
    let with = |tails: &[&[u8]]| {
        let mut old = body.to_vec();
        for t in tails {
            old.extend_from_slice(t);
        }
        extension(&old)
    };
    let (mut g, rest): (GameState, &[u8]) = if stored == VERSION {
        postcard::take_from_bytes(body).map_err(|_| LoadError::Corrupt)?
    } else {
        extension(body)
            .or_else(|_| with(&[&look]))
            .or_else(|_| with(&[&tutorial, &look]))
            .map(|g| (g, &[][..]))?
    };
    if !g.ids_valid(world) {
        return Err(LoadError::Corrupt);
    }
    load_extensions(&mut g, world, rest)?;
    Ok(g)
}

/// orig: `UnlockManager.SaveUnlocks()` (unlocks don't depend on the game data).
pub fn save_progress(p: &Progress) -> Vec<u8> {
    let mut out = header(0);
    out.extend(postcard::to_allocvec(p).expect("plain-data serialization cannot fail"));
    out
}

/// orig: `UnlockManager.LoadUnlocks()`: a damaged file starts over.
pub fn load_progress(bytes: &[u8]) -> Result<Progress, LoadError> {
    let (stored, body) = body(bytes, None)?;
    // Saved before `chosen` (None, a 0 byte), and before that
    // `tutorial_done` (false, another 0).
    let with = |zeros: usize| {
        let mut old = body.to_vec();
        old.resize(old.len() + zeros, 0);
        postcard::from_bytes(&old)
    };
    if stored == VERSION {
        postcard::from_bytes(body)
    } else {
        postcard::from_bytes(body)
            .or_else(|_| with(1))
            .or_else(|_| with(2))
    }
    .map_err(|_| LoadError::Corrupt)
}
