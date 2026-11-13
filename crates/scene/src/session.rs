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
    /// The bytes this run last loaded from or wrote to `slot`: the run
    /// writes over the slot unasked only while it holds these (or nothing).
    pub(crate) slot_bytes: Option<Vec<u8>>,
    /// What each slot holds, read when the pause menu opens.
    pub slots: [SlotState; save::SLOTS],
    /// The unlocks file as last read or written, which writes may replace.
    pub(crate) unlocks_bytes: Option<Vec<u8>>,
    /// The unlocks file can't be used: unlocks aren't being saved.
    pub unlocks: Option<UnlocksProblem>,
    /// A question the pause menu is asking before anything is lost.
    pub prompt: Option<Prompt>,
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
    /// Seconds shown, and how the write went once it has run.
    Saving(f32, Option<Outcome>),
    /// Seconds left.
    Saved(f32),
    Failed(f32),
    /// Not saved: the slot holds something the run mustn't replace unasked.
    Blocked(f32),
}

/// A save slot's summary for the pause menu.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotInfo {
    pub island: String,
    pub day: u32,
    pub coins: u32,
}

pub use crate::saves::{
    Choice, Incompatible, Outcome, Problem, Prompt, SavedGameError, SavedUnlocks, SlotState,
    UnlocksProblem,
};

impl SaveNotice {
    pub(crate) fn result(out: Outcome) -> SaveNotice {
        match out {
            Outcome::Saved => SaveNotice::Saved(SAVE_NOTICE_TIME),
            Outcome::Failed => SaveNotice::Failed(SAVE_NOTICE_TIME),
            Outcome::Blocked => SaveNotice::Blocked(SAVE_NOTICE_TIME),
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
            slot_bytes: None,
            slots: Default::default(),
            unlocks_bytes: None,
            unlocks: None,
            prompt: None,
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

    /// Runs a save put off to this frame, and counts the notice down.
    fn update_save_notice(&mut self, dt: f32) {
        self.save_notice = match self.save_notice {
            Some(SaveNotice::Saving(t, None)) => self
                .write_save()
                .map(|out| SaveNotice::Saving(t + dt, Some(out))),
            Some(SaveNotice::Saving(t, Some(ok))) if t + dt < SAVING_MIN_TIME => {
                Some(SaveNotice::Saving(t + dt, Some(ok)))
            }
            Some(SaveNotice::Saving(_, Some(ok))) => Some(SaveNotice::result(ok)),
            Some(SaveNotice::Saved(t)) if t > dt => Some(SaveNotice::Saved(t - dt)),
            Some(SaveNotice::Failed(t)) if t > dt => Some(SaveNotice::Failed(t - dt)),
            Some(SaveNotice::Blocked(t)) if t > dt => Some(SaveNotice::Blocked(t - dt)),
            _ => None,
        };
    }

    /// Pauses or resumes; pausing reads the slots for the menu.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if paused {
            self.read_slots();
        } else {
            self.prompt = None;
            self.close_editor();
        }
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

    /// Loads slot `i`, or starts a new game in it when it's empty. Unpauses.
    pub fn load_from(&mut self, i: usize) {
        let i = i.min(save::SLOTS - 1);
        let Ok(saved) = self.take_slot(i) else {
            return;
        };
        self.slot = i;
        self.prompt = None;
        self.map = None;
        self.ended = None;
        self.won_for = 0.0;
        self.day_banner = None;
        match saved {
            Some((bytes, g)) => {
                self.slot_bytes = Some(bytes);
                self.resume(g);
            }
            None => {
                self.slot_bytes = None;
                let seed = self.game.rng.next_u32();
                let g = GameState::start(&self.world, 0, seed, &self.progress, None);
                self.resume(g);
                if let Some(t) = self.world.audio.sound("IntroSequence.introSoundTrack") {
                    self.music_cues.push(crate::sound::MusicCue::Play(t));
                }
                self.save();
            }
        }
        if let Some(st) = &mut self.storage {
            st.write(save::CURRENT_SLOT, &[i as u8]);
        }
        self.paused = false;
    }

    /// orig: `Game.EndGame()`: unlocks are settled and stored; a lost run's
    /// save is deleted (`DeleteSavedGame`).
    pub(crate) fn end_run(&mut self) {
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
        if self.game.game_over {
            self.delete_lost_run();
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
