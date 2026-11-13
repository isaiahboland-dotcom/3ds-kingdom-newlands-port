//! A play session: the island being played, the unlocks kept across runs,
//! the map between islands, and saving. The backends feed it input and
//! draw it; everything else is the same on desktop and 3DS.

use game::controls::{ControlMapper, RawInput};
use game::data::WorldData;
use game::monarch::Action;
use game::progress::{Ending, Progress, Unveiled};
use game::save;
use game::state::{GameEvent, GameState, TICK};
use game::time;
use platform::Storage;

use crate::touch::MapChoice;

pub struct Session {
    pub game: GameState,
    pub world: WorldData,
    pub mapper: ControlMapper,
    pub events: Vec<GameEvent>,
    /// Ticks per frame (dev fast-forward).
    pub speed: u32,
    pub paused: bool,
    pub debug: bool,
    pub camera_x: f32,
    pub bag_bounce: f32,
    pub day_banner: Option<(u32, f32)>,
    /// Animator state for the monarch: current action and time in its clip.
    pub anim: (Action, f32),
    /// Seconds since start, for looping ambient animations (coin spin).
    pub t: f32,
    /// Unlocks and islands reached. orig: `UnlockManager` (`UnlockStorage`).
    pub progress: Progress,
    /// Seconds since the island was won (the boat sails off first).
    pub won_for: f32,
    /// Where saves go; `None` turns saving off.
    pub storage: Option<Box<dyn Storage>>,
    /// orig: `world.bin`'s fingerprint, kept in saves.
    pub world_id: u32,
    /// The run ended: what it unlocked. orig: `Game.EndGame()`.
    ended: Option<Unveiled>,
    /// The island map shown on the touch screen after a run.
    pub map: Option<MapChoice>,
    /// An extra line for the debug overlay (the backend's frame timings).
    pub note: String,
    /// The debug overlay's game-state lines (the 3DS shows only `note`:
    /// each letter costs a sprite).
    pub debug_lines: bool,
    /// Testing aid kept across runs: a full purse and an unhurt monarch
    /// (`GameState::god`).
    pub god: bool,
    /// A problem the player has to fix, shown on the touch screen (sound
    /// files missing or out of date).
    pub warning: String,
    /// Scroll of the water's displacement maps in texels (coarse, fine),
    /// and the camera they follow. orig: `Water._scrollSlow` / `_scrollFast`.
    pub water_scroll: (f32, f32),
    water_camera: f32,
    /// Sounds the game asked for since the backend's sound player last ran.
    pub cues: Vec<crate::sound::Cue>,
    pub music_cues: Vec<crate::sound::MusicCue>,
    /// The game slot saves go to (`save::game_slot`).
    pub slot: usize,
    /// The active slot must be loaded successfully before this run may write to it.
    active_slot_protected: bool,
    pub pending_new_slot: Option<usize>,
    /// What each slot holds, read when the pause menu opens.
    pub slots: [SlotState; save::SLOTS],
    /// The banner editor, open over the pause menu.
    pub editor: Option<crate::touch::Editor>,
    /// The corner notice for saving (see [`SaveNotice`]).
    pub save_notice: Option<SaveNotice>,
}

/// Seconds "Game saved" (or "Save failed") stays up.
pub const SAVE_NOTICE_TIME: f32 = 2.5;

/// Seconds the "Saving" spinner shows at the least (a quick write would
/// only flash it).
const SAVING_MIN_TIME: f32 = 0.6;

/// Saving, as shown in the top screen's corner. An autosave waits a frame
/// in `Saving`, so the spinner is on screen while the card is written (the
/// write holds up that frame), then shows its result for a while.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SaveNotice {
    /// Seconds shown, and whether the write worked once it has run.
    Saving(f32, Option<bool>),
    /// Seconds left.
    Saved(f32),
    Failed(f32),
}

/// A save slot's summary for the pause menu.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotInfo {
    pub island: String,
    pub day: u32,
    pub coins: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum SlotState {
    #[default]
    Empty,
    Saved(SlotInfo),
    Recovered(SlotInfo),
    Protected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedGameError {
    Read,
    Invalid(save::LoadError),
}

struct Recovered<T> {
    value: T,
    backup: bool,
    writable: bool,
}

fn read_record<T>(
    storage: &dyn Storage,
    slot: &str,
    decode: impl Fn(&[u8]) -> Result<T, save::LoadError>,
) -> Result<Option<Recovered<T>>, SavedGameError> {
    let primary = storage
        .read(slot)
        .map_err(|_| SavedGameError::Read)
        .and_then(|bytes| {
            bytes
                .as_deref()
                .map(&decode)
                .transpose()
                .map_err(SavedGameError::Invalid)
        });
    if let Ok(Some(value)) = primary {
        return Ok(Some(Recovered {
            value,
            backup: false,
            writable: true,
        }));
    }
    match storage
        .read_backup(slot)
        .map_err(|_| SavedGameError::Read)
        .and_then(|bytes| {
            bytes
                .as_deref()
                .map(decode)
                .transpose()
                .map_err(SavedGameError::Invalid)
        }) {
        Ok(Some(value)) => Ok(Some(Recovered {
            value,
            backup: true,
            writable: !matches!(primary, Err(SavedGameError::Read)),
        })),
        backup => match primary {
            Err(e) => Err(e),
            _ => backup.map(|_| None),
        },
    }
}

impl SaveNotice {
    fn result(ok: bool) -> SaveNotice {
        if ok {
            SaveNotice::Saved(SAVE_NOTICE_TIME)
        } else {
            SaveNotice::Failed(SAVE_NOTICE_TIME)
        }
    }
}

impl Session {
    pub fn new(world: WorldData, island: usize, seed: u32, progress: Progress) -> Self {
        let game = GameState::start(&world, island, seed, &progress, None);
        let camera_x = game.monarch.x;
        let mut s = Session {
            game,
            world,
            mapper: ControlMapper::default(),
            events: Vec::new(),
            speed: 1,
            paused: false,
            debug: false,
            camera_x,
            bag_bounce: 0.0,
            day_banner: None,
            anim: (Action::Stand, 0.0),
            t: 0.0,
            progress,
            won_for: 0.0,
            storage: None,
            water_scroll: (0.0, 0.0),
            cues: Vec::new(),
            music_cues: Vec::new(),
            slot: 0,
            active_slot_protected: false,
            pending_new_slot: None,
            slots: Default::default(),
            water_camera: camera_x,
            world_id: 0,
            ended: None,
            map: None,
            note: String::new(),
            debug_lines: true,
            warning: String::new(),
            god: false,
            editor: None,
            save_notice: None,
        };
        // orig: IntroSequence.RunSequence plays the intro track on the first island.
        if island == 0 {
            if let Some(t) = s.world.audio.sound("IntroSequence.introSoundTrack") {
                s.music_cues.push(crate::sound::MusicCue::Play(t));
            }
        }
        s
    }

    /// Picks up a saved run. orig: `Loader` → `PersistentManager.Load()`.
    pub fn resume(&mut self, game: GameState) {
        self.game = game;
        // The banner editor's pick holds for saved games too.
        if let Some((coat, look)) = self.progress.chosen {
            (self.game.coat, self.game.look) = (coat, look);
        }
        self.camera_x = self.game.monarch.x;
        self.water_camera = self.camera_x;
        self.music_cues.clear();
    }

    /// orig: `PersistentManager.Save("AUTO")`. Returns false if it failed.
    pub fn save(&mut self) -> bool {
        let written = self.write_save(self.slot);
        self.save_notice = written.map(SaveNotice::result);
        written.unwrap_or(true)
    }

    /// Writes the run: whether it worked, or `None` with nothing to save
    /// (saving off, or the run over).
    fn write_save(&mut self, slot: usize) -> Option<bool> {
        self.storage.as_ref()?;
        if self.game.game_over || self.game.won {
            return None;
        }
        if slot == self.slot && self.active_slot_protected {
            return Some(false);
        }
        let backup = match self.read_game(slot) {
            Ok(None) => false,
            Ok(Some(record)) if record.writable => record.backup,
            _ => {
                self.active_slot_protected |= slot == self.slot;
                return Some(false);
            }
        };
        let bytes = save::save_game(&self.game, self.world_id);
        let st = self.storage.as_mut()?;
        let name = save::game_slot(slot);
        let ok = if backup {
            st.write_preserving_backup(&name, &bytes)
        } else {
            st.write(&name, &bytes)
        };
        Some(ok && st.write(save::CURRENT_SLOT, &[slot as u8]))
    }

    /// Runs a save put off to this frame, and counts the notice down.
    fn update_save_notice(&mut self, dt: f32) {
        self.save_notice = match self.save_notice {
            Some(SaveNotice::Saving(t, None)) => self
                .write_save(self.slot)
                .map(|ok| SaveNotice::Saving(t + dt, Some(ok))),
            Some(SaveNotice::Saving(t, Some(ok))) if t + dt < SAVING_MIN_TIME => {
                Some(SaveNotice::Saving(t + dt, Some(ok)))
            }
            Some(SaveNotice::Saving(_, Some(ok))) => Some(SaveNotice::result(ok)),
            Some(SaveNotice::Saved(t)) if t > dt => Some(SaveNotice::Saved(t - dt)),
            Some(SaveNotice::Failed(t)) if t > dt => Some(SaveNotice::Failed(t - dt)),
            _ => None,
        };
    }

    /// The slot the last session used (stored with each save).
    pub fn last_slot(st: &dyn platform::Storage) -> usize {
        st.read(save::CURRENT_SLOT)
            .ok()
            .flatten()
            .and_then(|b| b.first().copied())
            .map_or(0, |b| (b as usize).min(save::SLOTS - 1))
    }

    /// Pauses or resumes; pausing reads the slots for the menu.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if !paused {
            self.pending_new_slot = None;
        }
        if paused {
            self.read_slots();
        } else {
            self.close_editor();
        }
    }

    /// Keep the active slot protected even if its file is repaired while the
    /// current session still holds a different run.
    pub fn protect_active_slot(&mut self) {
        self.active_slot_protected = true;
        self.set_paused(true);
    }

    /// Shows the coat of arms and monarch `look` on this island, and keeps
    /// them for runs to come.
    pub fn choose(&mut self, coat: game::kingdom::CoatOfArms, look: game::kingdom::Look) {
        (self.game.coat, self.game.look) = (coat, look);
        self.progress.chosen = Some((coat, look));
    }

    /// Closes the banner editor, keeping what was picked with the unlocks.
    pub fn close_editor(&mut self) {
        if self.editor.take().is_some() {
            self.write_progress();
        }
    }

    fn write_progress(&mut self) {
        if let Some(st) = &mut self.storage {
            let bytes = save::save_progress(&self.progress);
            match Self::read_progress(st.as_ref()) {
                Ok(None) => {
                    st.write(save::UNLOCKS_SLOT, &bytes);
                }
                Ok(Some(record)) if record.writable => {
                    if record.backup {
                        st.write_preserving_backup(save::UNLOCKS_SLOT, &bytes);
                    } else {
                        st.write(save::UNLOCKS_SLOT, &bytes);
                    }
                }
                _ => {}
            }
        }
    }

    fn read_progress(st: &dyn Storage) -> Result<Option<Recovered<Progress>>, SavedGameError> {
        read_record(st, save::UNLOCKS_SLOT, save::load_progress)
    }

    pub fn saved_progress(st: &dyn Storage) -> Progress {
        Self::read_progress(st)
            .ok()
            .flatten()
            .map(|record| record.value)
            .unwrap_or_default()
    }

    pub fn saved_game(
        st: &dyn Storage,
        slot: usize,
        world_id: u32,
        world: &WorldData,
    ) -> Result<Option<GameState>, SavedGameError> {
        read_record(st, &save::game_slot(slot), |bytes| {
            save::load_game(bytes, world_id, world)
        })
        .map(|record| record.map(|record| record.value))
    }

    fn read_game(&self, slot: usize) -> Result<Option<Recovered<GameState>>, SavedGameError> {
        self.storage.as_deref().map_or(Ok(None), |st| {
            read_record(st, &save::game_slot(slot), |bytes| {
                save::load_game(bytes, self.world_id, &self.world)
            })
        })
    }

    pub fn resume_saved_slot(&mut self) {
        match self.read_game(self.slot) {
            Ok(Some(record)) => {
                self.resume(record.value);
                if !record.writable {
                    self.protect_active_slot();
                }
            }
            Ok(None) => {}
            Err(_) => self.protect_active_slot(),
        }
    }

    fn read_slots(&mut self) {
        for i in 0..save::SLOTS {
            if i == self.slot && self.active_slot_protected {
                self.slots[i] = SlotState::Protected;
                continue;
            }
            self.slots[i] = match self.read_game(i) {
                Ok(Some(record)) if record.writable => {
                    let g = record.value;
                    let info = SlotInfo {
                        island: self
                            .world
                            .levels
                            .get(g.level_index)
                            .map_or(String::new(), |l| l.name.clone()),
                        day: g.days.day,
                        coins: g.wallet.coins(),
                    };
                    if record.backup {
                        SlotState::Recovered(info)
                    } else {
                        SlotState::Saved(info)
                    }
                }
                Ok(Some(_)) => SlotState::Protected,
                Ok(None) => SlotState::Empty,
                Err(_) => SlotState::Protected,
            };
            if i == self.slot && self.slots[i] == SlotState::Protected {
                self.active_slot_protected = true;
            }
        }
    }

    /// Saves the run into slot `i`, which autosaves use from now on.
    pub fn save_to(&mut self, i: usize) -> bool {
        let i = i.min(save::SLOTS - 1);
        let written = self.write_save(i);
        self.save_notice = written.map(SaveNotice::result);
        let ok = written.unwrap_or(true);
        if written == Some(true) || self.storage.is_none() {
            self.slot = i;
            self.active_slot_protected = false;
        }
        self.read_slots();
        ok
    }

    /// Loads slot `i`, or starts a new game in it when it's empty. Unpauses.
    pub fn load_from(&mut self, i: usize) {
        let i = i.min(save::SLOTS - 1);
        let Ok(saved) = self.read_game(i) else {
            if i == self.slot {
                self.protect_active_slot();
            } else {
                self.set_paused(true);
            }
            return;
        };
        self.slot = i;
        self.active_slot_protected = saved.as_ref().is_some_and(|record| !record.writable);
        self.pending_new_slot = None;
        self.map = None;
        self.ended = None;
        self.won_for = 0.0;
        self.day_banner = None;
        match saved {
            Some(record) => self.resume(record.value),
            None => {
                let seed = self.game.rng.next_u32();
                let g = GameState::start(&self.world, 0, seed, &self.progress, None);
                self.resume(g);
                if let Some(t) = self.world.audio.sound("IntroSequence.introSoundTrack") {
                    self.music_cues.push(crate::sound::MusicCue::Play(t));
                }
                self.save();
            }
        }
        if !self.active_slot_protected {
            if let Some(st) = &mut self.storage {
                st.write(save::CURRENT_SLOT, &[i as u8]);
            }
        }
        self.paused = false;
    }

    pub fn confirm_new_game(&mut self) {
        let Some(i) = self.pending_new_slot.take() else {
            return;
        };
        // Only replace a rejected file. A slot restored during confirmation
        // must be loaded through Retry instead.
        if self.read_game(i).is_ok() {
            self.read_slots();
            return;
        }
        let mut rng = self.game.rng.clone();
        let g = GameState::start(&self.world, 0, rng.next_u32(), &self.progress, None);
        let bytes = save::save_game(&g, self.world_id);
        let ok = self
            .storage
            .as_mut()
            .is_some_and(|st| st.write_preserving_backup(&save::game_slot(i), &bytes));
        self.save_notice = Some(SaveNotice::result(ok));
        if ok {
            self.load_from(i);
        } else {
            self.read_slots();
        }
    }

    /// orig: `Game.EndGame()`: unlocks are settled and stored; a lost run's
    /// save is deleted (`DeleteSavedGame`).
    fn end_run(&mut self) {
        if self.ended.is_some() {
            return;
        }
        let g = &self.game;
        let out = self.progress.finish(Ending {
            land: g.level_index,
            won: g.won,
            unlock: g.queued_unlock,
        });
        self.ended = Some(out);
        let (options, selected) = self.progress.choices(g.level_index, g.won, out.next);
        self.map = Some(MapChoice {
            options,
            selected,
            won: g.won,
            t: 0.0,
        });
        self.write_progress();
        if self.game.game_over
            && !self.active_slot_protected
            && matches!(
                self.read_game(self.slot),
                Ok(Some(Recovered {
                    backup: false,
                    writable: true,
                    ..
                }))
            )
        {
            if let Some(st) = &mut self.storage {
                st.delete(&save::game_slot(self.slot));
            }
        }
    }

    /// orig: `Game.EndGame()` → `UnlockManager.UnlockAndQueueNextLand()`, then
    /// the island picked on the map loads (sailed to after a win).
    pub fn next_island(&mut self) {
        self.end_run();
        self.ended = None;
        // orig: the map's action button: sail to (or start on) the island picked.
        let land = self
            .map
            .take()
            .map_or(self.game.level_index, |m| m.selected);
        let seed = self.game.rng.next_u32();
        let voyage = self
            .game
            .voyage
            .clone()
            .filter(|_| self.game.won)
            .map(|mut v| {
                v.land = land;
                v
            });
        let look = self.game.look;
        self.game = GameState::start(&self.world, land, seed, &self.progress, voyage.as_ref());
        // orig: NextLandData keeps the player model (and the coat, in the voyage).
        if voyage.is_some() {
            self.game.look = look;
        }
        self.camera_x = self.game.monarch.x;
        self.won_for = 0.0;
        // orig: UnlockManager.Restore → AutoSave.
        if voyage.is_some() {
            self.save();
        }
    }

    /// The map after a run is up (the boat gets a few seconds to sail first).
    pub fn map_shown(&self) -> bool {
        self.map.is_some() && (self.game.game_over || (self.game.won && self.won_for > 4.0))
    }

    /// One frame: `speed` ticks of the simulation with `raw` input.
    pub fn step(&mut self, raw: RawInput) {
        self.step_ticks(raw, self.speed);
    }

    /// One frame that advances the simulation `ticks` times (a backend
    /// running slower than 60 fps catches up this way).
    pub fn step_ticks(&mut self, raw: RawInput, ticks: u32) {
        self.update_save_notice(TICK * ticks.max(1) as f32);
        if self.paused {
            return;
        }
        let (mut saves, mut ended, mut tutorial) = (false, false, false);
        let mut raw = raw;
        self.game.god = self.god;
        for _ in 0..ticks {
            let controls = self.mapper.map(TICK, raw);
            raw.bag_tap = false;
            self.events.clear();
            self.game.tick(&self.world, &controls, &mut self.events);
            for e in &self.events {
                crate::sound::on_event(
                    &self.world,
                    &self.game,
                    e,
                    &mut self.cues,
                    &mut self.music_cues,
                );
                match *e {
                    GameEvent::Day(time::DayEvent::Started {
                        day,
                        show_counter: true,
                    }) => {
                        self.day_banner = Some((day, 3.0));
                    }
                    GameEvent::Coin(game::coins::CoinEvent::PickedUp(
                        game::coins::Who::Monarch,
                    )) => self.bag_bounce = 0.15,
                    GameEvent::AutoSave => saves = true,
                    GameEvent::TutorialDone => tutorial = true,
                    GameEvent::GameOver | GameEvent::Won => ended = true,
                    _ => {}
                }
            }
            let action = self.game.monarch.action();
            self.anim = if action == self.anim.0 {
                (action, self.anim.1 + TICK)
            } else {
                (action, 0.0)
            };
            self.t += TICK;
        }
        // orig: MainCamera follows the monarch with a lead; plain follow for now.
        // orig: MainCamera follows the teleporter exit while it flies.
        let target = match self.game.teleports.exit {
            Some(e) => e.x,
            None => self.game.monarch.x + self.game.monarch.velocity * 0.5,
        };
        self.camera_x +=
            (target - self.camera_x) * (1.0 - (-4.0 * TICK * ticks.max(1) as f32).exp());
        // orig: Water.Update: the maps drift and keep still against the ground
        // as the camera pans (a quarter of a map per unit, 4 maps per band).
        // No wind yet, so the fine map only follows the camera.
        let w = &self.world.water;
        let per_unit = 0.25 * w.size.0 as f32;
        let pan = (self.camera_x - self.water_camera) * per_unit;
        self.water_camera = self.camera_x;
        self.water_scroll.0 += TICK * ticks as f32 * 0.08 * w.size.0 as f32 + pan;
        self.water_scroll.1 += pan;
        if saves && self.storage.is_some() {
            self.save_notice = Some(SaveNotice::Saving(0.0, None));
        }
        // orig: TutorialManager.CompleteTutorial → PrefsStore TutorialCompleted.
        if tutorial && !self.progress.tutorial_done {
            self.progress.tutorial_done = true;
            self.write_progress();
        }
        if ended {
            self.end_run();
        }
        let dt = TICK * ticks as f32;
        if self.game.won {
            self.won_for += dt;
        }
        if let Some(m) = &mut self.map {
            m.t += dt;
        }
        self.bag_bounce = (self.bag_bounce - dt).max(0.0);
        if let Some((_, t)) = &mut self.day_banner {
            *t -= dt;
            if *t <= 0.0 {
                self.day_banner = None;
            }
        }
    }
}

#[cfg(test)]
mod save_protection_tests {
    use super::*;
    use game::data::{DayDef, DirectorDef, LevelConfigDef};
    use std::{
        cell::RefCell,
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
    };

    #[derive(Clone, Default)]
    struct Memory(Rc<RefCell<Files>>);

    #[derive(Clone, Debug, Default, PartialEq)]
    struct Files {
        bytes: BTreeMap<String, Vec<u8>>,
        backups: BTreeMap<String, Vec<u8>>,
        archived: Vec<Vec<u8>>,
        unreadable: BTreeSet<String>,
    }

    impl Storage for Memory {
        fn read(&self, slot: &str) -> Result<Option<Vec<u8>>, platform::ReadError> {
            let files = self.0.borrow();
            if files.unreadable.contains(slot) {
                Err(platform::ReadError)
            } else {
                Ok(files.bytes.get(slot).cloned())
            }
        }

        fn write(&mut self, slot: &str, data: &[u8]) -> bool {
            self.0.borrow_mut().bytes.insert(slot.into(), data.to_vec());
            true
        }

        fn read_backup(&self, slot: &str) -> Result<Option<Vec<u8>>, platform::ReadError> {
            Ok(self.0.borrow().backups.get(slot).cloned())
        }

        fn write_preserving_backup(&mut self, slot: &str, data: &[u8]) -> bool {
            let mut files = self.0.borrow_mut();
            if files.unreadable.contains(slot) {
                return false;
            }
            if let Some(old) = files.bytes.insert(slot.into(), data.to_vec()) {
                files.archived.push(old);
            }
            true
        }

        fn delete(&mut self, slot: &str) {
            self.0.borrow_mut().bytes.remove(slot);
        }
    }

    fn session(storage: &Memory) -> Session {
        let world = WorldData {
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
        };
        let mut s = Session::new(world, 0, 17, Progress::default());
        s.world_id = 42;
        s.storage = Some(Box::new(storage.clone()));
        s
    }

    #[test]
    fn failed_load_protects_files_from_save_autosave_and_game_over() {
        for (unreadable, wrong_world) in [(true, false), (false, true), (false, false)] {
            let mut memory = Memory::default();
            let mut s = session(&memory);
            let data = if wrong_world {
                save::save_game(&s.game, 7)
            } else {
                b"existing data".to_vec()
            };
            memory.write(save::GAME_SLOT, &data);
            memory.write(save::CURRENT_SLOT, &[0]);
            memory.write(save::UNLOCKS_SLOT, b"existing progress");
            if unreadable {
                memory
                    .0
                    .borrow_mut()
                    .unreadable
                    .insert(save::GAME_SLOT.into());
            }
            let before = memory.0.borrow().clone();
            s.game.wallet.set(27);
            let original_run = save::save_game(&s.game, s.world_id);

            assert!(Session::saved_game(&memory, 0, s.world_id, &s.world).is_err());
            s.load_from(0);
            assert!(s.paused);
            assert_eq!(s.slots[0], SlotState::Protected);
            assert_eq!(save::save_game(&s.game, s.world_id), original_run);
            assert!(!s.save());
            s.save_notice = Some(SaveNotice::Saving(0.0, None));
            s.step_ticks(RawInput::default(), 1);
            assert!(matches!(
                s.save_notice,
                Some(SaveNotice::Saving(_, Some(false)))
            ));
            s.game.game_over = true;
            s.end_run();
            assert_eq!(*memory.0.borrow(), before);
        }
    }

    #[test]
    fn rejecting_another_slot_preserves_current_run_and_empty_slots_still_work() {
        let mut memory = Memory::default();
        let mut s = session(&memory);
        s.game.wallet.set(27);
        assert!(s.save());
        memory.write(&save::game_slot(1), b"existing data");
        let before = memory.0.borrow().clone();
        s.load_from(1);
        assert!(!s.save_to(1));
        assert_eq!(s.slot, 0);
        assert_eq!(s.game.wallet.coins(), 27);
        assert_eq!(*memory.0.borrow(), before);

        s.load_from(2);
        assert!(!s.paused);
        assert_eq!(s.slot, 2);
        let saved = Session::saved_game(&memory, 2, s.world_id, &s.world)
            .unwrap()
            .unwrap();
        assert_eq!(saved.wallet.coins(), 5);
        assert_eq!(Session::last_slot(&memory), 2);
    }

    #[test]
    fn restored_active_file_requires_retry_before_save_or_delete() {
        let mut memory = Memory::default();
        let mut s = session(&memory);
        s.game.wallet.set(27);
        let restored = save::save_game(&s.game, s.world_id);
        s.game.wallet.set(5);
        memory.write(save::GAME_SLOT, b"existing data");
        s.load_from(0);
        memory.write(save::GAME_SLOT, &restored);

        assert!(!s.save());
        assert!(!s.save_to(0));
        assert_eq!(s.slots[0], SlotState::Protected);
        s.game.game_over = true;
        s.end_run();
        assert_eq!(memory.read(save::GAME_SLOT).unwrap(), Some(restored));

        s.load_from(0);
        assert!(!s.paused);
        assert!(!s.game.game_over);
        assert_eq!(s.game.wallet.coins(), 27);
        assert!(s.save());
    }

    #[test]
    fn recovery_preserves_a_validated_backup_and_archives_rejected_bytes() {
        let mut memory = Memory::default();
        let mut s = session(&memory);
        s.game.wallet.set(27);
        let backup = save::save_game(&s.game, s.world_id);
        memory
            .0
            .borrow_mut()
            .backups
            .insert(save::GAME_SLOT.into(), backup.clone());
        memory.write(save::GAME_SLOT, b"corrupt primary");
        s.game.wallet.set(5);
        s.resume_saved_slot();
        assert_eq!(s.game.wallet.coins(), 27);
        s.set_paused(true);
        assert!(matches!(s.slots[0], SlotState::Recovered(_)));

        s.game.game_over = true;
        s.end_run();
        assert_eq!(
            memory.read(save::GAME_SLOT).unwrap().unwrap(),
            b"corrupt primary"
        );
        assert_eq!(
            memory.read_backup(save::GAME_SLOT).unwrap(),
            Some(backup.clone())
        );
        s.load_from(0);
        assert!(s.save());
        assert_eq!(memory.read_backup(save::GAME_SLOT).unwrap(), Some(backup));
        assert_eq!(memory.0.borrow().archived, [b"corrupt primary".to_vec()]);
    }

    #[test]
    fn unreadable_primary_recovers_read_only_and_failed_replacement_preserves_run() {
        let mut memory = Memory::default();
        let mut s = session(&memory);
        s.game.wallet.set(27);
        let backup = save::save_game(&s.game, s.world_id);
        memory
            .0
            .borrow_mut()
            .backups
            .insert(save::GAME_SLOT.into(), backup.clone());
        memory.write(save::GAME_SLOT, b"unreadable");
        memory
            .0
            .borrow_mut()
            .unreadable
            .insert(save::GAME_SLOT.into());
        s.game.wallet.set(5);
        s.resume_saved_slot();
        assert_eq!(s.game.wallet.coins(), 27);
        assert!(!s.save());

        let other = save::game_slot(1);
        memory.write(&other, b"reject");
        memory.0.borrow_mut().unreadable.insert(other.clone());
        s.pending_new_slot = Some(1);
        s.confirm_new_game();
        assert_eq!(s.slot, 0);
        assert_eq!(s.game.wallet.coins(), 27);
        assert_eq!(memory.read_backup(save::GAME_SLOT).unwrap(), Some(backup));
        assert_eq!(memory.0.borrow().bytes[&other], b"reject");
    }

    #[test]
    fn replacement_requires_confirmation_and_keeps_the_rejected_file() {
        let mut memory = Memory::default();
        let mut s = session(&memory);
        memory.write(save::GAME_SLOT, b"reject");
        s.resume_saved_slot();
        s.game.wallet.set(27);
        let sheet = crate::sheet::Sheet::default();
        let _ = crate::touch::tap(&mut s, &sheet, (100.0, 160.0));
        assert_eq!(memory.read(save::GAME_SLOT).unwrap().unwrap(), b"reject");
        s.pending_new_slot = Some(0);
        let _ = crate::touch::tap(&mut s, &sheet, (80.0, 160.0));
        assert!(s.pending_new_slot.is_none());
        assert_eq!(memory.read(save::GAME_SLOT).unwrap().unwrap(), b"reject");
        s.pending_new_slot = Some(0);
        let _ = crate::touch::tap(&mut s, &sheet, (220.0, 160.0));
        assert_eq!(s.game.wallet.coins(), 5);
        assert_eq!(memory.0.borrow().archived, [b"reject".to_vec()]);
        assert!(!s.paused);
    }
}
