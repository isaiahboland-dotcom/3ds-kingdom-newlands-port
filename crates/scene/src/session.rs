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

/// PC target routing: TeleporterExit and Boat disable lead; dropped crowns
/// are tracked until pickup/despawn. Target changes preserve the old screen
/// position and seek, except startup/resume which explicitly snaps.
fn camera_target(game: &GameState) -> crate::camera::Target {
    use crate::camera::{Subject, Target};
    let (subject, x, lead) = if let Some(exit) = game.teleports.exit {
        (Subject::Teleporter, exit.x, false)
    } else if let Some(crown) = &game.greed.crown {
        (Subject::Crown, crown.x, false)
    } else if let Some(o) = game
        .harbor
        .boat
        .as_ref()
        .and_then(|b| game.level.index_of(b.object))
        .map(|i| &game.level.objects[i])
        .filter(|o| o.mode == game::boat::mode::SAILING)
    {
        (Subject::Boat(o.id), o.x, false)
    } else if game.game_over {
        (Subject::Inactive, game.monarch.x, false)
    } else {
        (Subject::Monarch, game.monarch.x, true)
    };
    Target {
        subject,
        x,
        direction: game.monarch.facing as f32,
        lead,
    }
}

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
    camera: Option<(crate::camera::CameraDef, crate::camera::Camera)>,
    camera_end_time: f32,
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
    /// What each slot holds, read when the pause menu opens.
    pub slots: [Option<SlotInfo>; save::SLOTS],
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
            camera: None,
            camera_end_time: 0.0,
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
            slots: [None, None, None],
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

    pub fn install_camera(&mut self, definition: crate::camera::CameraDef) {
        let camera = crate::camera::Camera::new(camera_target(&self.game));
        self.camera_x = camera.position();
        self.water_camera = self.camera_x;
        self.camera = Some((definition, camera));
    }

    fn reset_camera(&mut self) {
        self.camera_end_time = 0.0;
        self.camera_x = if let Some((_, camera)) = &mut self.camera {
            *camera = crate::camera::Camera::new(camera_target(&self.game));
            camera.position()
        } else {
            self.game.monarch.x
        };
    }

    /// Picks up a saved run. orig: `Loader` → `PersistentManager.Load()`.
    pub fn resume(&mut self, game: GameState) {
        self.game = game;
        // The banner editor's pick holds for saved games too.
        if let Some((coat, look)) = self.progress.chosen {
            (self.game.coat, self.game.look) = (coat, look);
        }
        self.reset_camera();
        self.water_camera = self.camera_x;
        self.music_cues.clear();
    }

    /// orig: `PersistentManager.Save("AUTO")`. Returns false if it failed.
    pub fn save(&mut self) -> bool {
        let written = self.write_save();
        self.save_notice = written.map(SaveNotice::result);
        written.unwrap_or(true)
    }

    /// Writes the run: whether it worked, or `None` with nothing to save
    /// (saving off, or the run over).
    fn write_save(&mut self) -> Option<bool> {
        let st = self.storage.as_mut()?;
        if self.game.game_over || self.game.won {
            return None;
        }
        let bytes = save::save_game(&self.game, self.world_id);
        Some(
            st.write(&save::game_slot(self.slot), &bytes)
                && st.write(save::CURRENT_SLOT, &[self.slot as u8]),
        )
    }

    /// Runs a save put off to this frame, and counts the notice down.
    fn update_save_notice(&mut self, dt: f32) {
        self.save_notice = match self.save_notice {
            Some(SaveNotice::Saving(t, None)) => self
                .write_save()
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
            .and_then(|b| b.first().copied())
            .map_or(0, |b| (b as usize).min(save::SLOTS - 1))
    }

    /// Pauses or resumes; pausing reads the slots for the menu.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if paused {
            self.read_slots();
        } else {
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

    fn write_progress(&mut self) {
        if let Some(st) = &mut self.storage {
            st.write(save::UNLOCKS_SLOT, &save::save_progress(&self.progress));
        }
    }

    fn read_slots(&mut self) {
        for i in 0..save::SLOTS {
            self.slots[i] = self.storage.as_ref().and_then(|st| {
                let g = save::load_game(&st.read(&save::game_slot(i))?, self.world_id, &self.world)
                    .ok()?;
                Some(SlotInfo {
                    island: self
                        .world
                        .levels
                        .get(g.level_index)
                        .map_or(String::new(), |l| l.name.clone()),
                    day: g.days.day,
                    coins: g.wallet.coins(),
                })
            });
        }
    }

    /// Saves the run into slot `i`, which autosaves use from now on.
    pub fn save_to(&mut self, i: usize) -> bool {
        self.slot = i.min(save::SLOTS - 1);
        let ok = self.save();
        self.read_slots();
        ok
    }

    /// Loads slot `i`, or starts a new game in it when it's empty. Unpauses.
    pub fn load_from(&mut self, i: usize) {
        let i = i.min(save::SLOTS - 1);
        let saved = self
            .storage
            .as_ref()
            .and_then(|st| st.read(&save::game_slot(i)))
            .and_then(|b| save::load_game(&b, self.world_id, &self.world).ok());
        self.slot = i;
        self.map = None;
        self.ended = None;
        self.won_for = 0.0;
        self.day_banner = None;
        match saved {
            Some(g) => self.resume(g),
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
        if let Some(st) = &mut self.storage {
            st.write(save::CURRENT_SLOT, &[i as u8]);
        }
        self.paused = false;
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
        if let Some(st) = &mut self.storage {
            st.write(save::UNLOCKS_SLOT, &save::save_progress(&self.progress));
            if self.game.game_over {
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
        self.reset_camera();
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
            if let Some((definition, camera)) = &mut self.camera {
                let mut target = camera_target(&self.game);
                if self.game.game_over && !self.game.won {
                    self.camera_end_time += TICK;
                    if self.camera_end_time >= 4.0 {
                        target.subject = crate::camera::Subject::Monarch;
                        target.x = self.game.monarch.x;
                        target.lead = false;
                    }
                }
                self.camera_x = camera.step(definition, target, TICK);
            }
        }
        // orig: MainCamera follows the monarch with a lead; plain follow for now.
        // orig: MainCamera follows the teleporter exit while it flies.
        if self.camera.is_none() {
            let target = match self.game.teleports.exit {
                Some(e) => e.x,
                None => self.game.monarch.x + self.game.monarch.velocity * 0.5,
            };
            self.camera_x +=
                (target - self.camera_x) * (1.0 - (-4.0 * TICK * ticks.max(1) as f32).exp());
        }
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
