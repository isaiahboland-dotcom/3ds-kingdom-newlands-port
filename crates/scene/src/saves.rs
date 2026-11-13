//! Keeping saves safe: what each slot holds, when the run may write over
//! it, and the pause menu's questions before anything is replaced or
//! deleted.
//!
//! The rule: the run writes a file without asking only when the file is
//! missing, or holds exactly the bytes this run last loaded from it or wrote
//! to it. Anything else (another run's save, a save this build can't read, a
//! file that failed to read) is kept until the player chooses to replace or
//! delete it. A failed read never sticks: the next save checks again.

use game::progress::Progress;
use game::save::{self, LoadError};
use game::state::GameState;
use platform::Storage;

use crate::session::{SaveNotice, Session, SlotInfo};

/// What a save slot holds, as the pause menu shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SlotState {
    #[default]
    Empty,
    Saved(SlotInfo),
    /// A save this build can't load. It loads again with the build (and
    /// game data) that made it.
    Incompatible(Incompatible),
    /// Not a save this build can make sense of; the backup the backend
    /// kept, if it loads.
    Damaged(Option<SlotInfo>),
    /// There, but reading it failed (perhaps only this once); the backup,
    /// if it loads.
    Unreadable(Option<SlotInfo>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Incompatible {
    /// Made with other game data (`world.bin` changed since).
    OtherData,
    /// Made by a newer (or unknown) version of the game.
    OtherVersion,
}

impl SlotState {
    /// Holds something this run can't load: it's only replaced or deleted
    /// when the player says so.
    pub fn protected(&self) -> bool {
        !matches!(self, SlotState::Empty | SlotState::Saved(_))
    }

    pub fn backup(&self) -> Option<&SlotInfo> {
        match self {
            SlotState::Damaged(b) | SlotState::Unreadable(b) => b.as_ref(),
            _ => None,
        }
    }
}

/// Why a saved game couldn't be loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedGameError {
    Read,
    Invalid(LoadError),
}

/// What's wrong with the unlocks file. Until it's fixed the unlocks earned
/// are kept in memory but not written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnlocksProblem {
    pub kind: Problem,
    /// The backend kept a previous copy that loads.
    pub backup: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    Unreadable,
    Damaged,
    OtherVersion,
}

impl SavedUnlocks {
    pub fn problem(&self) -> Option<UnlocksProblem> {
        self.problem
    }
}

/// The unlocks as read at startup, for [`Session::new`] and then
/// [`Session::use_unlocks`]. One read gives both, so what the run plays
/// with is what it may later write over.
#[derive(Clone, Debug, Default)]
pub struct SavedUnlocks {
    pub progress: Progress,
    bytes: Option<Vec<u8>>,
    problem: Option<UnlocksProblem>,
}

/// A question the pause menu asks before anything is lost. Each holds what
/// was shown, so a file that changes meanwhile is asked about again.
#[derive(Clone, Debug, PartialEq)]
pub enum Prompt {
    /// Write this run over slot `i`, which holds another save or a
    /// protected file.
    Overwrite(usize, SlotState),
    /// What to do about a protected slot.
    Fix(usize, SlotState),
    /// Delete slot `i` for good.
    Delete(usize, SlotState),
    /// What to do about the unlocks file.
    Unlocks(UnlocksProblem),
    /// Write this session's unlocks over the unlocks file.
    ReplaceUnlocks(UnlocksProblem),
}

/// A button in a [`Prompt`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Overwrite,
    Retry,
    Restore,
    Delete,
    Replace,
    Cancel,
}

impl Choice {
    pub fn label(self) -> &'static str {
        match self {
            Choice::Overwrite => "OVERWRITE",
            Choice::Retry => "RETRY",
            Choice::Restore => "RESTORE",
            Choice::Delete => "DELETE",
            Choice::Replace => "REPLACE",
            Choice::Cancel => "CANCEL",
        }
    }
}

impl Prompt {
    pub fn choices(&self) -> Vec<Choice> {
        let mut out = Vec::new();
        match self {
            Prompt::Overwrite(..) => out.push(Choice::Overwrite),
            Prompt::Fix(_, st) => {
                if matches!(st, SlotState::Unreadable(_)) {
                    out.push(Choice::Retry);
                }
                if st.backup().is_some() {
                    out.push(Choice::Restore);
                }
                out.push(Choice::Delete);
            }
            Prompt::Delete(..) => out.push(Choice::Delete),
            Prompt::Unlocks(p) => {
                out.push(Choice::Retry);
                if p.backup {
                    out.push(Choice::Restore);
                }
                out.push(Choice::Replace);
            }
            Prompt::ReplaceUnlocks(_) => out.push(Choice::Replace),
        }
        out.push(Choice::Cancel);
        out
    }
}

/// How a save went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Saved,
    /// Reading or writing failed; the next save tries again.
    Failed,
    /// The slot holds something this run mustn't replace unasked.
    Blocked,
}

impl Session {
    /// Slot `i`'s file: `Ok(None)` when there's none (or no storage).
    fn game_file(&self, i: usize) -> Result<Option<Vec<u8>>, platform::ReadError> {
        self.storage
            .as_deref()
            .map_or(Ok(None), |st| st.read(&save::game_slot(i)))
    }

    fn load_bytes(&self, bytes: &[u8]) -> Result<GameState, LoadError> {
        save::load_game(bytes, self.world_id, &self.world)
    }

    fn info(&self, g: &GameState) -> SlotInfo {
        SlotInfo {
            island: self
                .world
                .levels
                .get(g.level_index)
                .map_or(String::new(), |l| l.name.clone()),
            day: g.days.day,
            coins: g.wallet.coins(),
        }
    }

    fn backup_info(&self, i: usize) -> Option<SlotInfo> {
        let st = self.storage.as_deref()?;
        let bytes = st.read_backup(&save::game_slot(i)).ok()??;
        Some(self.info(&self.load_bytes(&bytes).ok()?))
    }

    /// What slot `i` holds (reads and decodes it).
    pub fn slot_state(&self, i: usize) -> SlotState {
        match self.game_file(i) {
            Ok(None) => SlotState::Empty,
            Ok(Some(bytes)) => match self.load_bytes(&bytes) {
                Ok(g) => SlotState::Saved(self.info(&g)),
                Err(LoadError::World) => SlotState::Incompatible(Incompatible::OtherData),
                Err(LoadError::Version) => SlotState::Incompatible(Incompatible::OtherVersion),
                Err(LoadError::Format | LoadError::Corrupt) => {
                    SlotState::Damaged(self.backup_info(i))
                }
            },
            Err(_) => SlotState::Unreadable(self.backup_info(i)),
        }
    }

    pub(crate) fn read_slots(&mut self) {
        for i in 0..save::SLOTS {
            self.slots[i] = self.slot_state(i);
        }
    }

    /// Whether the run may write slot `i` without asking: it's empty, or
    /// it's the run's slot and holds exactly what the run last loaded or
    /// wrote there. A byte comparison, so autosaves never decode.
    fn may_write(&self, i: usize) -> Result<bool, platform::ReadError> {
        Ok(match self.game_file(i)? {
            None => true,
            Some(bytes) => i == self.slot && self.slot_bytes.as_deref() == Some(&bytes[..]),
        })
    }

    /// Whether the run's slot holds exactly what the run last wrote there.
    fn holds_this_run(&self) -> bool {
        matches!(self.game_file(self.slot), Ok(Some(b)) if self.slot_bytes.as_ref() == Some(&b))
    }

    /// Writes the run to slot `i`, which becomes the run's slot once the
    /// file holds it. Returns whether everything was written.
    fn write_slot(&mut self, i: usize) -> bool {
        let bytes = save::save_game(&self.game, self.world_id);
        let Some(st) = self.storage.as_mut() else {
            return false;
        };
        let name = save::game_slot(i);
        let ok = st.write(&name, &bytes);
        // A write that reports failure may still have landed; whatever the
        // file holds now decides what the run may write over next.
        let landed = ok || st.read(&name).ok().flatten().as_deref() == Some(&bytes[..]);
        if landed {
            self.slot = i;
            self.slot_bytes = Some(bytes);
        }
        ok && st.write(save::CURRENT_SLOT, &[i as u8])
    }

    /// Saves the run to its slot unless the slot holds something else.
    /// `None` with nothing to save (saving off, or the run over).
    pub(crate) fn write_save(&mut self) -> Option<Outcome> {
        self.storage.as_ref()?;
        if self.game.game_over || self.game.won {
            return None;
        }
        Some(match self.may_write(self.slot) {
            // Couldn't check: keep the run in memory and try next time.
            Err(_) => Outcome::Failed,
            Ok(false) => Outcome::Blocked,
            Ok(true) if self.write_slot(self.slot) => Outcome::Saved,
            Ok(true) => Outcome::Failed,
        })
    }

    /// orig: `PersistentManager.Save("AUTO")`. Returns false if it failed.
    pub fn save(&mut self) -> bool {
        let out = self.write_save();
        self.save_notice = out.map(SaveNotice::result);
        out.is_none_or(|o| o == Outcome::Saved)
    }

    /// Saves the run into slot `i`, which autosaves use from now on. A slot
    /// holding anything but this run asks first (`prompt`).
    pub fn save_to(&mut self, i: usize) -> bool {
        let i = i.min(save::SLOTS - 1);
        if self.storage.is_none() {
            self.slot = i;
            return true;
        }
        if self.game.game_over || self.game.won {
            return true;
        }
        let ok = match self.may_write(i) {
            Ok(true) => {
                let ok = self.write_slot(i);
                self.save_notice = Some(SaveNotice::result(if ok {
                    Outcome::Saved
                } else {
                    Outcome::Failed
                }));
                ok
            }
            _ => {
                self.prompt = Some(Prompt::Overwrite(i, self.slot_state(i)));
                false
            }
        };
        self.read_slots();
        ok
    }

    /// Picks up the saved run in the current slot at startup. A slot that
    /// can't be loaded stays as it is: the menu opens on what to do.
    pub fn open_saved(&mut self) -> Result<bool, SavedGameError> {
        let err = match self.game_file(self.slot) {
            Ok(None) => return Ok(false),
            Ok(Some(bytes)) => match self.load_bytes(&bytes) {
                Ok(g) => {
                    self.resume(g);
                    self.slot_bytes = Some(bytes);
                    return Ok(true);
                }
                Err(e) => SavedGameError::Invalid(e),
            },
            Err(_) => SavedGameError::Read,
        };
        self.set_paused(true);
        self.prompt = Some(Prompt::Fix(self.slot, self.slots[self.slot].clone()));
        Err(err)
    }

    /// Slot `i` for [`Session::load_from`]: its bytes and run, `Ok(None)`
    /// when it's empty, or `Err` (and the menu asks what to do).
    pub(crate) fn take_slot(&mut self, i: usize) -> Result<Option<(Vec<u8>, GameState)>, ()> {
        if let Ok(file) = self.game_file(i) {
            let Some(bytes) = file else {
                return Ok(None);
            };
            if let Ok(g) = self.load_bytes(&bytes) {
                return Ok(Some((bytes, g)));
            }
        }
        self.set_paused(true);
        self.prompt = Some(Prompt::Fix(i, self.slots[i].clone()));
        Err(())
    }

    /// A lost run's save goes (orig: `DeleteSavedGame`), but only if it's
    /// still this run's.
    pub(crate) fn delete_lost_run(&mut self) {
        if self.holds_this_run() {
            if let Some(st) = &mut self.storage {
                st.delete(&save::game_slot(self.slot));
            }
            self.slot_bytes = None;
        }
    }

    /// The prompt's button `c`.
    pub fn choose_prompt(&mut self, c: Choice) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        if c == Choice::Cancel {
            self.read_slots();
            return;
        }
        if let Prompt::Overwrite(i, shown) | Prompt::Fix(i, shown) | Prompt::Delete(i, shown) =
            &prompt
        {
            let (i, now) = (*i, self.slot_state(*i));
            if now != *shown {
                // The file changed since the question (a retry that now
                // reads lands here too): show what's there now, and ask
                // again only if it's still in the way.
                self.read_slots();
                self.prompt = match prompt {
                    Prompt::Overwrite(..) => {
                        (now != SlotState::Empty).then_some(Prompt::Overwrite(i, now))
                    }
                    _ => now.protected().then_some(Prompt::Fix(i, now)),
                };
                return;
            }
        }
        match prompt {
            Prompt::Overwrite(i, _) => {
                if !(self.game.game_over || self.game.won) {
                    let ok = self.write_slot(i);
                    self.save_notice = Some(SaveNotice::result(if ok {
                        Outcome::Saved
                    } else {
                        Outcome::Failed
                    }));
                }
                self.read_slots();
            }
            Prompt::Fix(i, shown) => match c {
                Choice::Retry => {
                    self.read_slots();
                    let now = self.slots[i].clone();
                    if now.protected() {
                        self.prompt = Some(Prompt::Fix(i, now));
                    }
                }
                Choice::Restore => {
                    let ok = self
                        .storage
                        .as_mut()
                        .is_some_and(|st| st.restore_backup(&save::game_slot(i)));
                    if ok {
                        self.load_from(i);
                    } else {
                        self.save_notice = Some(SaveNotice::result(Outcome::Failed));
                        self.read_slots();
                        self.prompt = Some(Prompt::Fix(i, self.slots[i].clone()));
                    }
                }
                Choice::Delete => self.prompt = Some(Prompt::Delete(i, shown)),
                _ => self.prompt = Some(Prompt::Fix(i, shown)),
            },
            Prompt::Delete(i, _) => {
                if let Some(st) = &mut self.storage {
                    st.delete(&save::game_slot(i));
                }
                if i == self.slot {
                    self.slot_bytes = None;
                }
                self.read_slots();
            }
            Prompt::Unlocks(p) => match c {
                Choice::Retry => self.retry_unlocks(),
                Choice::Restore => {
                    let ok = self
                        .storage
                        .as_mut()
                        .is_some_and(|st| st.restore_backup(save::UNLOCKS_SLOT));
                    if ok {
                        self.retry_unlocks();
                    } else {
                        self.prompt = Some(Prompt::Unlocks(p));
                    }
                }
                Choice::Replace => self.prompt = Some(Prompt::ReplaceUnlocks(p)),
                _ => self.prompt = Some(Prompt::Unlocks(p)),
            },
            Prompt::ReplaceUnlocks(_) => self.write_unlocks(true),
        }
    }

    fn retry_unlocks(&mut self) {
        self.write_unlocks(false);
        if let Some(p) = self.unlocks {
            self.prompt = Some(Prompt::Unlocks(p));
        }
    }

    /// Opens the unlocks question (the pause menu's warning button).
    pub fn ask_unlocks(&mut self) {
        if let Some(p) = self.unlocks {
            self.prompt = Some(Prompt::Unlocks(p));
        }
    }

    /// The slot the last session used (stored with each save).
    pub fn last_slot(st: &dyn Storage) -> usize {
        st.read(save::CURRENT_SLOT)
            .ok()
            .flatten()
            .and_then(|b| b.first().copied())
            .map_or(0, |b| (b as usize).min(save::SLOTS - 1))
    }

    /// Slot `slot`'s saved run, read without a session (the desktop's
    /// read-only `--load`).
    pub fn saved_game(
        st: &dyn Storage,
        slot: usize,
        world_id: u32,
        world: &game::data::WorldData,
    ) -> Result<Option<GameState>, SavedGameError> {
        st.read(&save::game_slot(slot))
            .map_err(|_| SavedGameError::Read)?
            .as_deref()
            .map(|bytes| save::load_game(bytes, world_id, world))
            .transpose()
            .map_err(SavedGameError::Invalid)
    }

    /// Reads the unlocks: what to play with, and the file's problem if it
    /// can't be used (then the defaults, until it's fixed).
    pub fn saved_unlocks(st: &dyn Storage) -> SavedUnlocks {
        let backup = || {
            st.read_backup(save::UNLOCKS_SLOT)
                .ok()
                .flatten()
                .is_some_and(|b| save::load_progress(&b).is_ok())
        };
        match st.read(save::UNLOCKS_SLOT) {
            Ok(None) => SavedUnlocks::default(),
            Ok(Some(bytes)) => match save::load_progress(&bytes) {
                Ok(progress) => SavedUnlocks {
                    progress,
                    bytes: Some(bytes),
                    problem: None,
                },
                Err(e) => SavedUnlocks {
                    problem: Some(UnlocksProblem {
                        kind: problem(e),
                        backup: backup(),
                    }),
                    ..Default::default()
                },
            },
            Err(_) => SavedUnlocks {
                problem: Some(UnlocksProblem {
                    kind: Problem::Unreadable,
                    backup: backup(),
                }),
                ..Default::default()
            },
        }
    }

    /// Keeps what [`Session::saved_unlocks`] read (call once storage is set).
    pub fn use_unlocks(&mut self, u: SavedUnlocks) {
        self.unlocks_bytes = u.bytes;
        self.unlocks = u.problem;
    }

    pub(crate) fn write_progress(&mut self) {
        self.write_unlocks(false);
    }

    /// Writes the unlocks. The file is written over only when it's missing
    /// or holds what this session read or wrote; a file that has loadable
    /// unlocks this session didn't see is merged in first (so a file that
    /// failed to read at startup is never replaced by the defaults); one
    /// that can't be read is kept, unless `replace` (the player chose to).
    fn write_unlocks(&mut self, replace: bool) {
        let Some(st) = self.storage.as_mut() else {
            return;
        };
        let backup = |st: &dyn Storage| {
            st.read_backup(save::UNLOCKS_SLOT)
                .ok()
                .flatten()
                .is_some_and(|b| save::load_progress(&b).is_ok())
        };
        match st.read(save::UNLOCKS_SLOT) {
            Ok(None) => {}
            Ok(Some(bytes)) if self.unlocks_bytes.as_ref() == Some(&bytes) => {}
            Ok(Some(bytes)) => match save::load_progress(&bytes) {
                Ok(disk) => self.progress.merge(&disk),
                Err(_) if replace => {}
                Err(e) => {
                    self.unlocks = Some(UnlocksProblem {
                        kind: problem(e),
                        backup: backup(st.as_ref()),
                    });
                    return;
                }
            },
            Err(_) if replace => {}
            Err(_) => {
                self.unlocks = Some(UnlocksProblem {
                    kind: Problem::Unreadable,
                    backup: backup(st.as_ref()),
                });
                return;
            }
        }
        let bytes = save::save_progress(&self.progress);
        if st.write(save::UNLOCKS_SLOT, &bytes) {
            self.unlocks_bytes = Some(bytes);
            self.unlocks = None;
        }
    }
}

fn problem(e: LoadError) -> Problem {
    match e {
        LoadError::Version => Problem::OtherVersion,
        _ => Problem::Damaged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SAVE_NOTICE_TIME;
    use game::controls::RawInput;
    use game::data::{DayDef, DirectorDef, LevelConfigDef, WorldData};
    use std::{
        cell::RefCell,
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
    };

    /// Storage in memory, shared with the session under test. Reads of
    /// `unreadable` slots fail; `backups` is what the 3DS keeps beside a
    /// slot; `lie` makes writes land but report failure.
    #[derive(Clone, Default)]
    struct Memory(Rc<RefCell<Files>>);

    #[derive(Clone, Debug, Default, PartialEq)]
    struct Files {
        bytes: BTreeMap<String, Vec<u8>>,
        backups: BTreeMap<String, Vec<u8>>,
        unreadable: BTreeSet<String>,
        lie: bool,
    }

    impl Storage for Memory {
        fn read(&self, slot: &str) -> Result<Option<Vec<u8>>, platform::ReadError> {
            let f = self.0.borrow();
            if f.unreadable.contains(slot) {
                return Err(platform::ReadError);
            }
            Ok(f.bytes.get(slot).cloned())
        }

        fn read_backup(&self, slot: &str) -> Result<Option<Vec<u8>>, platform::ReadError> {
            Ok(self.0.borrow().backups.get(slot).cloned())
        }

        fn restore_backup(&mut self, slot: &str) -> bool {
            let mut f = self.0.borrow_mut();
            let Some(b) = f.backups.get(slot).cloned() else {
                return false;
            };
            f.unreadable.remove(slot);
            f.bytes.insert(slot.into(), b);
            true
        }

        fn write(&mut self, slot: &str, data: &[u8]) -> bool {
            let mut f = self.0.borrow_mut();
            f.bytes.insert(slot.into(), data.to_vec());
            !f.lie
        }

        fn delete(&mut self, slot: &str) {
            let mut f = self.0.borrow_mut();
            f.bytes.remove(slot);
            f.backups.remove(slot);
        }
    }

    impl Memory {
        fn put(&self, slot: &str, data: &[u8]) {
            self.0.borrow_mut().bytes.insert(slot.into(), data.to_vec());
        }
        fn backup(&self, slot: &str, data: &[u8]) {
            self.0
                .borrow_mut()
                .backups
                .insert(slot.into(), data.to_vec());
        }
        fn unreadable(&self, slot: &str, on: bool) {
            let mut f = self.0.borrow_mut();
            if on {
                f.unreadable.insert(slot.into());
            } else {
                f.unreadable.remove(slot);
            }
        }
        fn get(&self, slot: &str) -> Option<Vec<u8>> {
            self.0.borrow().bytes.get(slot).cloned()
        }
        fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
            self.0.borrow().bytes.clone()
        }
    }

    const WORLD: u32 = 42;

    fn world() -> WorldData {
        WorldData {
            days: vec![DayDef {
                name: "day".into(),
                seasons: 7,
                weathers: 1,
                tracks: vec![],
            }],
            director: DirectorDef {
                regular_days: vec![0],
                boss_days: vec![0],
                recovery_days: vec![0],
                num_regular_days: (3, 4),
                ..Default::default()
            },
            levels: vec![LevelConfigDef {
                name: "island".into(),
                autumn_start: (40, 40),
                winter_start: (60, 60),
                days_before_game_over: -1,
                starting_coins: 5,
                min_level_width: 0,
                group_counts: vec![],
                waves: Default::default(),
                wave_timed_multiplier: 1.0,
                wave_multiplier_timeframe: (0, -1),
                defence_waves: [None; 3],
                retaliation_waves: [None; 3],
                season_change_days: 3.0,
                income_multiplier: 1.0,
                free_boat_parts: 0,
                wave_boat_multiplier: 1.0,
                force_all_unlocks: false,
            }],
            steeds: vec![Default::default()],
            ..Default::default()
        }
    }

    /// A session on `m` as a backend starts one: unlocks read, then the
    /// last slot opened.
    fn start(m: &Memory) -> (Session, Result<bool, SavedGameError>) {
        let u = Session::saved_unlocks(m);
        let mut s = Session::new(world(), 0, 17, u.progress.clone());
        s.world_id = WORLD;
        s.storage = Some(Box::new(m.clone()));
        s.slot = Session::last_slot(m);
        s.use_unlocks(u);
        let opened = s.open_saved();
        (s, opened)
    }

    /// A valid save with `coins`, made with game data `world`.
    fn run_with(coins: u32, world: u32) -> Vec<u8> {
        let mut g = Session::new(self::world(), 0, 3, Progress::default()).game;
        g.wallet.set(coins);
        save::save_game(&g, world)
    }

    fn coins_in(m: &Memory, slot: usize) -> u32 {
        let bytes = m.get(&save::game_slot(slot)).expect("a save");
        save::load_game(&bytes, WORLD, &world())
            .expect("loads")
            .wallet
            .coins()
    }

    fn info(coins: u32) -> SlotInfo {
        SlotInfo {
            island: "island".into(),
            day: 1,
            coins,
        }
    }

    #[test]
    fn world_data_update_keeps_old_saves_until_the_player_replaces_or_deletes_them() {
        let m = Memory::default();
        let old = run_with(27, 7);
        for i in 0..save::SLOTS {
            m.put(&save::game_slot(i), &old);
        }
        m.put(save::CURRENT_SLOT, &[0]);
        let before = m.snapshot();

        let (mut s, opened) = start(&m);
        assert_eq!(opened, Err(SavedGameError::Invalid(LoadError::World)));
        assert!(s.paused);
        let other = SlotState::Incompatible(Incompatible::OtherData);
        assert_eq!(s.prompt, Some(Prompt::Fix(0, other.clone())));
        assert_eq!(s.slots, [other.clone(), other.clone(), other.clone()]);
        assert_eq!(
            s.prompt.as_ref().unwrap().choices(),
            [Choice::Delete, Choice::Cancel]
        );

        // Nothing is replaced unasked: autosave, game over, a tap on SAVE.
        assert!(!s.save());
        assert_eq!(s.save_notice, Some(SaveNotice::Blocked(SAVE_NOTICE_TIME)));
        assert!(!s.save_to(1));
        assert_eq!(s.prompt, Some(Prompt::Overwrite(1, other.clone())));
        s.choose_prompt(Choice::Cancel);
        assert_eq!(m.snapshot(), before);

        // Asked and confirmed: this run goes to slot 2, and saves there from now on.
        s.game.wallet.set(12);
        s.save_to(1);
        s.choose_prompt(Choice::Overwrite);
        assert_eq!(s.slot, 1);
        assert_eq!(coins_in(&m, 1), 12);
        assert_eq!(m.get(save::CURRENT_SLOT), Some(vec![1]));
        s.game.wallet.set(30);
        assert!(s.save());
        assert_eq!(coins_in(&m, 1), 30);

        // Deleting asks twice; cancelling keeps the file.
        s.prompt = Some(Prompt::Fix(0, s.slots[0].clone()));
        s.choose_prompt(Choice::Delete);
        assert_eq!(s.prompt, Some(Prompt::Delete(0, other.clone())));
        s.choose_prompt(Choice::Cancel);
        assert_eq!(m.get(save::GAME_SLOT), Some(old.clone()));
        s.prompt = Some(Prompt::Fix(0, s.slots[0].clone()));
        s.choose_prompt(Choice::Delete);
        s.choose_prompt(Choice::Delete);
        assert_eq!(m.get(save::GAME_SLOT), None);
        assert_eq!(s.slots[0], SlotState::Empty);
        assert_eq!(m.get(&save::game_slot(2)), Some(old));
    }

    #[test]
    fn a_newer_versions_save_is_told_apart_from_a_damaged_one() {
        let m = Memory::default();
        let mut newer = run_with(5, WORLD);
        newer[4..6].copy_from_slice(&(save::VERSION + 1).to_le_bytes());
        m.put(save::GAME_SLOT, &newer);
        m.put(&save::game_slot(1), b"existing data");
        let mut truncated = run_with(5, WORLD);
        truncated.truncate(truncated.len() - 3);
        m.put(&save::game_slot(2), &truncated);
        let (s, opened) = start(&m);
        assert_eq!(opened, Err(SavedGameError::Invalid(LoadError::Version)));
        assert_eq!(
            s.slots,
            [
                SlotState::Incompatible(Incompatible::OtherVersion),
                SlotState::Damaged(None),
                SlotState::Damaged(None),
            ]
        );
    }

    #[test]
    fn a_failed_read_during_play_keeps_the_run_and_saves_once_it_reads() {
        let m = Memory::default();
        let (mut s, opened) = start(&m);
        assert_eq!(opened, Ok(false));
        s.game.wallet.set(27);
        assert!(s.save());

        // Progress made while the card won't read stays in the run.
        m.unreadable(save::GAME_SLOT, true);
        s.game.wallet.set(40);
        assert!(!s.save());
        assert_eq!(s.save_notice, Some(SaveNotice::Failed(SAVE_NOTICE_TIME)));
        s.save_notice = Some(SaveNotice::Saving(0.0, None));
        s.step_ticks(RawInput::default(), 1);
        assert_eq!(
            s.save_notice
                .map(|n| matches!(n, SaveNotice::Saving(_, Some(Outcome::Failed)))),
            Some(true)
        );
        let coins = s.game.wallet.coins();
        m.unreadable(save::GAME_SLOT, false);
        assert_eq!(coins_in(&m, 0), 27);
        m.unreadable(save::GAME_SLOT, true);

        // The menu shows it; a retry that still fails changes nothing.
        s.set_paused(true);
        assert_eq!(s.slots[0], SlotState::Unreadable(None));
        s.prompt = Some(Prompt::Fix(0, s.slots[0].clone()));
        assert_eq!(
            s.prompt.as_ref().unwrap().choices(),
            [Choice::Retry, Choice::Delete, Choice::Cancel]
        );
        s.choose_prompt(Choice::Retry);
        assert_eq!(s.prompt, Some(Prompt::Fix(0, SlotState::Unreadable(None))));

        // Once it reads, retry clears it without loading over the run, and
        // nothing stays locked: the next save writes the run as it is now.
        m.unreadable(save::GAME_SLOT, false);
        s.choose_prompt(Choice::Retry);
        assert_eq!(s.prompt, None);
        assert_eq!(s.slots[0], SlotState::Saved(info(27)));
        assert_eq!(s.game.wallet.coins(), coins);
        s.set_paused(false);
        s.save_notice = Some(SaveNotice::Saving(0.0, None));
        s.step_ticks(RawInput::default(), 1);
        assert_eq!(
            s.save_notice
                .map(|n| matches!(n, SaveNotice::Saving(_, Some(Outcome::Saved)))),
            Some(true)
        );
        let saved = save::load_game(&m.get(save::GAME_SLOT).unwrap(), WORLD, &world()).unwrap();
        // The coins picked up while the card wouldn't read are in the file.
        assert_eq!(saved.wallet.coins(), 40);
        assert_eq!(coins, 40);
    }

    #[test]
    fn a_failed_read_at_startup_never_overwrites_the_slot_and_loads_on_retry() {
        let m = Memory::default();
        let saved = run_with(27, WORLD);
        m.put(save::GAME_SLOT, &saved);
        m.unreadable(save::GAME_SLOT, true);
        let (mut s, opened) = start(&m);
        assert_eq!(opened, Err(SavedGameError::Read));
        assert_eq!(s.prompt, Some(Prompt::Fix(0, SlotState::Unreadable(None))));

        // The fresh run in memory isn't the slot's: neither while the read
        // fails nor once it works may it write or delete it.
        assert!(!s.save());
        m.unreadable(save::GAME_SLOT, false);
        assert!(!s.save());
        assert_eq!(s.save_notice, Some(SaveNotice::Blocked(SAVE_NOTICE_TIME)));
        s.game.game_over = true;
        s.end_run();
        assert_eq!(m.get(save::GAME_SLOT), Some(saved.clone()));
        s.next_island();

        s.set_paused(true);
        assert_eq!(s.slots[0], SlotState::Saved(info(27)));
        s.load_from(0);
        assert!(!s.paused);
        assert_eq!(s.game.wallet.coins(), 27);
        s.game.wallet.set(28);
        assert!(s.save());
        assert_eq!(coins_in(&m, 0), 28);
    }

    #[test]
    fn a_damaged_slot_restores_its_backup() {
        let m = Memory::default();
        m.put(save::GAME_SLOT, b"existing data");
        m.backup(save::GAME_SLOT, &run_with(27, WORLD));
        m.put(&save::game_slot(1), b"existing data");
        let (mut s, opened) = start(&m);
        assert_eq!(opened, Err(SavedGameError::Invalid(LoadError::Format)));
        let damaged = SlotState::Damaged(Some(info(27)));
        assert_eq!(s.prompt, Some(Prompt::Fix(0, damaged)));
        assert_eq!(
            s.prompt.as_ref().unwrap().choices(),
            [Choice::Restore, Choice::Delete, Choice::Cancel]
        );
        assert_eq!(s.slots[1], SlotState::Damaged(None));
        s.choose_prompt(Choice::Restore);
        assert!(!s.paused);
        assert_eq!(s.game.wallet.coins(), 27);
        s.game.wallet.set(31);
        assert!(s.save());
        assert_eq!(coins_in(&m, 0), 31);
    }

    #[test]
    fn game_over_deletes_only_this_runs_own_save() {
        // The run's own save goes.
        let m = Memory::default();
        let (mut s, _) = start(&m);
        assert!(s.save());
        s.game.game_over = true;
        s.end_run();
        assert_eq!(m.get(save::GAME_SLOT), None);

        // One put there since (a restored file, another copy of the game) stays,
        // and so does a damaged one.
        for replaced in [run_with(99, WORLD), b"existing data".to_vec()] {
            let m = Memory::default();
            let (mut s, _) = start(&m);
            assert!(s.save());
            m.put(save::GAME_SLOT, &replaced);
            s.game.game_over = true;
            s.end_run();
            assert_eq!(m.get(save::GAME_SLOT), Some(replaced));
        }
    }

    #[test]
    fn saving_over_another_game_asks_and_asks_again_if_the_file_changes() {
        let m = Memory::default();
        let (mut s, _) = start(&m);
        s.game.wallet.set(27);
        assert!(s.save());
        m.put(&save::game_slot(1), &run_with(99, WORLD));
        let before = m.snapshot();

        assert!(!s.save_to(1));
        assert_eq!(
            s.prompt,
            Some(Prompt::Overwrite(1, SlotState::Saved(info(99))))
        );
        s.choose_prompt(Choice::Cancel);
        assert_eq!((s.slot, m.snapshot()), (0, before));

        // The other save is replaced by a damaged file while the question is up.
        s.save_to(1);
        m.put(&save::game_slot(1), b"existing data");
        s.choose_prompt(Choice::Overwrite);
        assert_eq!(
            s.prompt,
            Some(Prompt::Overwrite(1, SlotState::Damaged(None)))
        );
        assert_eq!(m.get(&save::game_slot(1)), Some(b"existing data".to_vec()));
        s.choose_prompt(Choice::Overwrite);
        assert_eq!(s.slot, 1);
        assert_eq!(coins_in(&m, 1), 27);

        // An empty slot needs no question; NEW starts and saves a run there.
        s.load_from(2);
        assert_eq!((s.slot, s.paused, s.prompt.clone()), (2, false, None));
        assert_eq!(coins_in(&m, 2), 5);
        assert_eq!(Session::last_slot(&m), 2);
    }

    #[test]
    fn a_write_that_reports_failure_but_lands_still_counts_as_this_runs() {
        let m = Memory::default();
        let (mut s, _) = start(&m);
        m.0.borrow_mut().lie = true;
        assert!(!s.save());
        m.0.borrow_mut().lie = false;
        s.game.wallet.set(8);
        assert!(s.save());
        assert_eq!(coins_in(&m, 0), 8);
    }

    fn progress_with(unlocked: &[u8], land: u8) -> Progress {
        Progress {
            unlocked: unlocked.to_vec(),
            land,
            ..Default::default()
        }
    }

    fn unlocks_on(m: &Memory) -> Progress {
        save::load_progress(&m.get(save::UNLOCKS_SLOT).unwrap()).unwrap()
    }

    #[test]
    fn unlocks_that_failed_to_read_at_startup_are_merged_not_replaced() {
        let m = Memory::default();
        m.put(
            save::UNLOCKS_SLOT,
            &save::save_progress(&progress_with(&[1, 2], 3)),
        );
        m.unreadable(save::UNLOCKS_SLOT, true);
        let (mut s, _) = start(&m);
        assert_eq!(s.progress, Progress::default());
        let unreadable = UnlocksProblem {
            kind: Problem::Unreadable,
            backup: false,
        };
        assert_eq!(s.unlocks, Some(unreadable));

        // Still failing: nothing is written; the unlock earned stays in memory.
        s.progress.unlocked.push(5);
        s.write_progress();
        m.unreadable(save::UNLOCKS_SLOT, false);
        assert_eq!(unlocks_on(&m), progress_with(&[1, 2], 3));

        // It reads now: the file's unlocks are kept and the new one added.
        s.write_progress();
        assert_eq!(s.unlocks, None);
        assert_eq!(unlocks_on(&m), progress_with(&[5, 1, 2], 3));
        assert_eq!(s.progress, unlocks_on(&m));
    }

    #[test]
    fn damaged_unlocks_are_kept_until_restored_or_replaced() {
        let m = Memory::default();
        m.put(save::UNLOCKS_SLOT, b"existing progress");
        let (mut s, _) = start(&m);
        let damaged = UnlocksProblem {
            kind: Problem::Damaged,
            backup: false,
        };
        assert_eq!(s.unlocks, Some(damaged));
        s.progress.unlocked.push(4);
        s.write_progress();
        assert_eq!(
            m.get(save::UNLOCKS_SLOT),
            Some(b"existing progress".to_vec())
        );

        s.set_paused(true);
        s.ask_unlocks();
        assert_eq!(
            s.prompt.as_ref().unwrap().choices(),
            [Choice::Retry, Choice::Replace, Choice::Cancel]
        );
        s.choose_prompt(Choice::Replace);
        assert_eq!(s.prompt, Some(Prompt::ReplaceUnlocks(damaged)));
        s.choose_prompt(Choice::Cancel);
        assert_eq!(
            m.get(save::UNLOCKS_SLOT),
            Some(b"existing progress".to_vec())
        );
        s.ask_unlocks();
        s.choose_prompt(Choice::Replace);
        s.choose_prompt(Choice::Replace);
        assert_eq!((s.unlocks, s.prompt.clone()), (None, None));
        assert_eq!(unlocks_on(&m), progress_with(&[4], 0));

        // With a backup: restoring brings its unlocks back alongside the new ones.
        let m = Memory::default();
        m.put(save::UNLOCKS_SLOT, b"existing progress");
        m.backup(
            save::UNLOCKS_SLOT,
            &save::save_progress(&progress_with(&[2], 2)),
        );
        let (mut s, _) = start(&m);
        assert_eq!(
            s.unlocks,
            Some(UnlocksProblem {
                kind: Problem::Damaged,
                backup: true
            })
        );
        s.progress.unlocked.push(4);
        s.set_paused(true);
        s.ask_unlocks();
        s.choose_prompt(Choice::Restore);
        assert_eq!((s.unlocks, s.prompt.clone()), (None, None));
        assert_eq!(unlocks_on(&m), progress_with(&[4, 2], 2));
    }

    #[test]
    fn unpausing_closes_a_question_without_acting_on_it() {
        let m = Memory::default();
        m.put(save::GAME_SLOT, &run_with(27, 7));
        let (mut s, _) = start(&m);
        assert!(s.prompt.is_some());
        s.set_paused(false);
        assert_eq!(s.prompt, None);
        assert_eq!(m.get(save::GAME_SLOT), Some(run_with(27, 7)));
    }
}
