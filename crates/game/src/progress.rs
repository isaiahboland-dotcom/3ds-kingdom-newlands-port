//! What outlasts an island: the unlocks and furthest island reached (kept
//! across runs), and what a won island hands to the next one.
//!
//! orig: `UnlockManager`, `UnlockStorage`, `NextLandData`, `Signpost`.

use alloc::vec::Vec;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::data::{ProtoId, WorldData, UNLOCKABLES};
use crate::kingdom::CoatOfArms;
use crate::rng::Rng;

/// orig: `Land.Five`: winning it finishes the game; `Land.Six` is the challenge island.
pub const LAND_FIVE: usize = 4;
pub const LAND_SIX: usize = 5;
/// orig: `DoUnlock`: a new unlock is forced into the next three islands.
const FORCE_RECENT: u8 = 3;

/// orig: `UnlockStorage`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    /// `Unlockable` indices (see [`UNLOCKABLES`]).
    pub unlocked: Vec<u8>,
    pub recent: u8,
    /// Islands left that must include `recent`. orig: `forceRecentUnlock`.
    pub force_recent: u8,
    /// The furthest island reached (`Land` index).
    pub land: u8,
    pub game_finished: bool,
    pub challenge_finished: bool,
    /// The ghost's tutorial was finished once; it doesn't come back. Last,
    /// so older files load with it appended (`save::load_progress`).
    /// orig: `PrefsStore` `TutorialCompleted`.
    pub tutorial_done: bool,
    /// The coat of arms and monarch picked in the banner editor, for every
    /// run after (not in the original, which rolls them anew each run).
    /// Last, as `tutorial_done`.
    pub chosen: Option<(CoatOfArms, crate::kingdom::Look)>,
}

/// How a run ended, for [`Progress::finish`].
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ending {
    pub land: usize,
    pub won: bool,
    /// The map taken from the signpost. orig: `UnlockManager._queuedUnlock`.
    pub unlock: Option<u8>,
}

/// What changed, for the map screen. orig: `UnlockManager.hasRecentUnlock`,
/// `landWasUnlocked`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unveiled {
    pub unlock: Option<u8>,
    pub land: Option<u8>,
    /// Where to sail to (won only).
    pub next: Option<usize>,
}

impl Progress {
    /// Everything unlocked. orig: `UnlockManager.UnlockAll()` (debug).
    pub fn everything() -> Self {
        Progress {
            unlocked: (0..UNLOCKABLES.len() as u8).collect(),
            land: LAND_SIX as u8,
            game_finished: true,
            challenge_finished: true,
            ..Default::default()
        }
    }

    /// The block unlock ids for generating `land`, and the one forced in.
    /// orig: `LevelLayout.SetUnlocked()`.
    pub fn block_unlocks(&self, world: &WorldData, land: usize) -> Option<(Vec<i32>, Option<i32>)> {
        if world.levels.get(land).is_some_and(|c| c.force_all_unlocks) {
            return None;
        }
        let ids = self.unlocked.iter().map(|&u| u as i32).collect();
        let force = (self.force_recent > 0).then_some(self.recent as i32);
        Some((ids, force))
    }

    /// orig: `GetUnlockableOnCurrentLand()`.
    pub fn unlockable_on(&self, world: &WorldData, land: usize) -> Vec<u8> {
        world
            .unlockable_on_land
            .iter()
            .enumerate()
            .filter(|&(u, &l)| l as usize == land && !self.unlocked.contains(&(u as u8)))
            .map(|(u, _)| u as u8)
            .collect()
    }

    /// orig: `QueueRandomUnlock()` (the signpost's map).
    pub fn random_unlock(&self, world: &WorldData, land: usize, rng: &mut Rng) -> Option<u8> {
        let list = self.unlockable_on(world, land);
        if list.is_empty() {
            return None;
        }
        Some(list[rng.range_i32(0, list.len() as i32) as usize])
    }

    /// Folds in unlocks kept elsewhere (a file this session couldn't read at
    /// first), so neither side loses any: every unlock, the furthest
    /// island, what's been finished, and this side's banner over theirs. The
    /// forced recent unlock is whichever has more islands left to force it.
    pub fn merge(&mut self, other: &Progress) {
        for &u in &other.unlocked {
            if !self.unlocked.contains(&u) {
                self.unlocked.push(u);
            }
        }
        if other.force_recent > self.force_recent {
            (self.recent, self.force_recent) = (other.recent, other.force_recent);
        }
        self.land = self.land.max(other.land);
        self.game_finished |= other.game_finished;
        self.challenge_finished |= other.challenge_finished;
        self.tutorial_done |= other.tutorial_done;
        if self.chosen.is_none() {
            self.chosen = other.chosen;
        }
    }

    /// orig: `UnlockAndQueueNextLand()` (called from `Game.EndGame`).
    pub fn finish(&mut self, e: Ending) -> Unveiled {
        let mut out = Unveiled::default();
        self.force_recent = self.force_recent.saturating_sub(1);
        if let Some(u) = e.unlock {
            if !self.unlocked.contains(&u) {
                self.unlocked.push(u);
            }
            self.recent = u;
            self.force_recent = FORCE_RECENT;
            out.unlock = Some(u);
        }
        if e.won {
            let next = if e.land < LAND_FIVE {
                let next = e.land + 1;
                if (self.land as usize) < next {
                    self.land = next as u8;
                    out.land = Some(next as u8);
                }
                next
            } else if e.land == LAND_FIVE {
                out.land = Some(LAND_SIX as u8);
                self.game_finished = true;
                self.land = LAND_SIX as u8;
                0
            } else {
                self.challenge_finished = true;
                self.land = LAND_SIX as u8;
                0
            };
            out.next = Some(next);
        }
        out
    }

    /// The islands the map lets you pick, and the one it starts on.
    /// orig: `Map.Show()`: after a win (`Continue`) any revealed island but
    /// this one, starting on where the boat was headed; after a loss
    /// (`Restart`) any revealed island but Land Six, starting on this one.
    pub fn choices(&self, current: usize, won: bool, next: Option<usize>) -> (Vec<usize>, usize) {
        let lands = (LAND_SIX + 1).min(self.land as usize + 1);
        if won {
            let list: Vec<usize> = (0..lands).filter(|&l| l != current).collect();
            let pick = next.filter(|n| list.contains(n)).or(list.first().copied());
            (list, pick.unwrap_or(0))
        } else {
            let list: Vec<usize> = (0..lands).filter(|&l| l != LAND_SIX).collect();
            let pick = if current == LAND_SIX { 0 } else { current };
            (list, pick)
        }
    }

    /// orig: `MapLand.SetUnlocked()`: clouds hide islands not reached yet
    /// (and Land Six unless sailing on).
    pub fn revealed(&self, land: usize, won: bool) -> bool {
        self.is_unlocked(land) && (land != LAND_SIX || won)
    }

    /// orig: `IsUnlocked(Land)`.
    pub fn is_unlocked(&self, land: usize) -> bool {
        land <= self.land as usize
    }
}

/// orig: `NextLandData`: what sails to the next island.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Voyage {
    pub land: usize,
    pub reign: u32,
    pub coat: CoatOfArms,
    /// The steed the monarch rides. orig: `PlayerPreview.steedPrefabPath`.
    pub steed: u16,
    pub coins: u32,
    /// The hermit riding along. orig: `hermitPrefabPath`.
    pub hermit: Option<ProtoId>,
    /// orig: `hasDog`, `dogColor`.
    pub dog: Option<[f32; 3]>,
    pub archers: u8,
    pub workers: u8,
    pub knights: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[cfg(feature = "save")]
    #[test]
    fn unlocks_from_before_tutorial_done_and_chosen_still_load() {
        let p = Progress {
            land: 2,
            tutorial_done: true,
            ..Default::default()
        };
        let bytes = crate::save::save_progress(&p);
        // `chosen` is None (1 byte), `tutorial_done` true (1 byte).
        let mut bytes = bytes;
        bytes[4..6].copy_from_slice(&4u16.to_le_bytes());
        let before_chosen = crate::save::load_progress(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(before_chosen, p);
        let before_both = crate::save::load_progress(&bytes[..bytes.len() - 2]).unwrap();
        assert_eq!(before_both.land, 2);
        assert!(!before_both.tutorial_done);
    }

    #[test]
    fn the_map_offers_islands_reached() {
        let p = Progress {
            land: 3,
            ..Default::default()
        };
        // Sailing on from Land Two: back to One, or on to Three or Four.
        assert_eq!(p.choices(1, true, Some(2)), (vec![0, 2, 3], 2));
        // Lost on Land Three: start again anywhere reached, here by default.
        assert_eq!(p.choices(2, false, None), (vec![0, 1, 2, 3], 2));
        assert!(p.revealed(3, false) && !p.revealed(4, true));
        let done = Progress::everything();
        assert!(done.choices(4, true, Some(0)).0.contains(&LAND_SIX));
        assert!(!done.choices(4, false, None).0.contains(&LAND_SIX));
    }
}
