//! The whole simulation for one island, advanced one fixed tick at a time.

use alloc::vec::Vec;

use crate::build::{self, PayKind, Tech, Upgraded};
use crate::coins::{CoinEvent, Coins, Grabber, Wallet, Who};
use crate::controls::Controls;
use crate::data::WorldData;
use crate::director::{Director, DirectorOutput, Season, WaveKind};
use crate::farm::{FarmCtx, Farms};
use crate::grass::Meadows;
use crate::greed::{Greed, GreedCtx, GreedEvent, Loot, LootInfo, PortalInfo, Target, TargetInfo};
use crate::kingdom::{self, Borders, CoatOfArms};
use crate::level::{self, Level, Unlocks};
use crate::lighting::Blender;
use crate::monarch::{Ground, Monarch, MonarchEvent, SteedStats};
use crate::payable::{choose_target, Payable};
use crate::payment::{PayEvent, PayInput, PayMachine, PayableId};
use crate::rng::Rng;
use crate::time::{Clock, DayCounter, DayEvent};
use crate::units::{Ctx, FarmInfo, Population, SlotInfo, UnitEvent};
use crate::wildlife::{Species, WildCtx, WildEvent, Wildlife};

/// How often the bear swipes while galloping (its gallop clip's ability event).
const BEAR_SWIPE_INTERVAL: f32 = 0.5;

/// Fixed simulation step. Backends call [`GameState::tick`] at this rate.
pub const TICK: f32 = 1.0 / 60.0;
/// What god mode fills the purse to, once (spending doesn't take from it):
/// fewer than the bag holds before coins spill over the rim (~50).
pub const GOD_COINS: u32 = 20;

/// Things the backends react to with sound, UI or effects.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GameEvent {
    Day(DayEvent),
    Monarch(MonarchEvent),
    Coin(CoinEvent),
    Pay(PayEvent),
    /// orig: `World.GoToSeason()`.
    Season(Season),
    /// A wave is due today; the greed system schedules it.
    WaveDue(WaveKind),
    /// A build site was paid for and replaced by its next stage.
    Upgraded(Upgraded),
    /// A scaffold finished building (object id).
    Built(u32),
    Unit(UnitEvent),
    Greed(GreedEvent),
    /// orig: `Player.LoseCrown()`.
    CrownLost,
    CrownRecovered,
    /// The greed took the crown. orig: `Game.EndGame(Result.Lost)`.
    GameOver,
    /// Both cliff portals are down. orig: `Game.EndGame(Result.Won)`.
    Won,
    Wild(WildEvent),
    Harbor(crate::boat::HarborEvent),
    /// Time to save. orig: `PersistentManager.HandleOnBecameSafe()` →
    /// `AutoSave()` (2 s later, from the second day on).
    AutoSave,
    Bag(crate::bag::BagEvent),
    /// Queue this soundtrack. orig: `AudioManager.QueueTrack()`.
    Music(crate::data::SoundId),
    /// Two hired and a wall up: the ghost fades. orig: `TutorialManager.CompleteTutorial()`.
    TutorialDone,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct GameState {
    pub clock: Clock,
    pub days: DayCounter,
    pub monarch: Monarch,
    pub wallet: Wallet,
    pub coins: Coins,
    pub payables: Vec<Payable>,
    pub pay: PayMachine,
    pub rng: Rng,
    pub level_index: usize,
    pub level: Level,
    pub director: Director,
    pub blender: Blender,
    pub season: Season,
    pub tech: Tech,
    /// Merged forest x-ranges. orig: `World.forests`.
    pub forests: Vec<(f32, f32)>,
    pub population: Population,
    /// orig: `Kingdom.campfirePosition`.
    pub campfire: f32,
    pub borders: Borders,
    /// orig: `Kingdom.isSafe`: day and no enemies around.
    pub safe: bool,
    pub greed: Greed,
    /// orig: `Player.hasCrown`.
    pub has_crown: bool,
    pub game_over: bool,
    /// orig: `Game.Result.Won`.
    pub won: bool,
    pub farms: Farms,
    /// orig: `World._season` (0 summer, 1 autumn, 2 winter), easing toward
    /// `_seasonTarget`.
    pub season_blend: f32,
    season_target: f32,
    /// orig: `Kingdom.*StatueCharged`.
    pub farmer_statue: bool,
    pub archer_statue: bool,
    pub worker_statue: bool,
    pub knight_statue: bool,
    pub statues: crate::shrines::Statues,
    farmers_seen: usize,
    /// Forest decorations fading out after their forest shrank: (object id,
    /// seconds left). orig: `ForestItem.FadeAndDestroy()`.
    fading: Vec<(u32, f32)>,
    pub meadows: Meadows,
    pub wildlife: Wildlife,
    pub mounts: crate::mounts::Mounts,
    pub hermits: crate::hermits::Hermits,
    pub merchants: crate::merchant::Merchants,
    pub treasure: crate::treasure::Treasure,
    pub siege: crate::siege::Siege,
    pub teleports: crate::teleport::Teleports,
    pub dogs: crate::dog::Dogs,
    /// Workshops whose counters are already out.
    workshops: Vec<u32>,
    /// The hermit riding with the monarch (its object). orig: `Player.passenger`.
    pub passenger: Option<u32>,
    /// The steed the monarch rides (index into `WorldData::steeds`).
    pub steed: u16,
    /// orig: `Steed._abilityTimer`.
    steed_ability: f32,
    /// Coins the unicorn still has to drop: (left, seconds to the next).
    poop: (u8, f32),
    /// Seconds to the bear's next swipe while galloping.
    steed_attack: f32,
    /// orig: `Kingdom.coatOfArms`.
    pub coat: CoatOfArms,
    /// Guard slots that were free last tick.
    free_slots: Vec<(u32, u8)>,
    /// Ticks since the start.
    pub ticks: u32,
    /// Became safe this tick (`Kingdom.OnBecameSafe`), for the merchant.
    became_safe: bool,
    payables_dirty: bool,
    /// The wharf, the boat and the beach.
    pub harbor: crate::boat::Harbor,
    /// What the signpost's map could unlock here. orig: `Signpost._available`
    /// (`UnlockManager.GetUnlockableOnCurrentLand()`).
    unlockable: Vec<u8>,
    /// The unlock the map promised. orig: `UnlockManager._queuedUnlock`.
    pub queued_unlock: Option<u8>,
    /// orig: `UnlockManager.reign`.
    pub reign: u32,
    /// What sails on to the next island, once the island is won.
    /// orig: `UnlockManager.QueueLand()`.
    pub voyage: Option<crate::progress::Voyage>,
    /// The monarch's height off the ground (on deck while sailing).
    pub monarch_y: f32,
    /// Events from paying, handed out with the next tick's.
    queued: Vec<GameEvent>,
    /// Seconds until the autosave.
    autosave_in: Option<f32>,
    /// The coins piled in the purse. orig: `CoinBag`.
    pub bag: crate::bag::Bag,
    /// The bag's own dice, so its tumbling doesn't shift the island's.
    bag_rng: Rng,
    /// The ghost guide and the title. Last, so older saves load with it
    /// appended (`save::load_game`).
    pub tutorial: crate::tutorial::Tutorial,
    /// The banner's pattern and emblem, and king or queen. After the
    /// tutorial for the same reason.
    pub look: kingdom::Look,
    /// Testing aid: the purse stays topped up and nothing hurts the
    /// monarch. Not saved.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub god: bool,
}

impl GameState {
    /// Reject saved world indices before they can reach indexed access.
    pub(crate) fn ids_valid(&self, world: &WorldData) -> bool {
        self.level_index < world.levels.len()
            && (self.steed as usize) < world.steeds.len()
            && self
                .mounts
                .free
                .iter()
                .all(|f| (f.steed as usize) < world.steeds.len())
            && self
                .level
                .all()
                .all(|o| (o.proto as usize) < world.protos.len())
            && self.blender.presets_valid(world)
            && self.director.lights_valid(world)
    }
    /// Starts a fresh run on island `level_index` (0 = Land One).
    pub fn new(world: &WorldData, level_index: usize, seed: u32) -> Self {
        Self::start(
            world,
            level_index,
            seed,
            &crate::progress::Progress::everything(),
            None,
        )
    }

    /// A run on island `level_index` with `progress`'s unlocks, carrying on
    /// from `voyage` if the monarch sailed here.
    pub fn start(
        world: &WorldData,
        level_index: usize,
        seed: u32,
        progress: &crate::progress::Progress,
        voyage: Option<&crate::progress::Voyage>,
    ) -> Self {
        let mut g = Self::with_rng(world, level_index, Rng::new(seed), progress);
        g.level.split_scenery(world);
        // orig: Game.StartGame → TutorialManager.StartTutorial on a fresh start.
        let guide = level_index == 0 && voyage.is_none() && !progress.tutorial_done;
        g.tutorial = crate::tutorial::Tutorial::new(world, &mut g.level, guide);
        // Its own dice, so the island comes out the same either way.
        g.look = kingdom::Look::random(&mut Rng::new(seed ^ 0x6c00_c0a7));
        if let Some((coat, look)) = progress.chosen {
            (g.coat, g.look) = (coat, look);
        }
        if let Some(v) = voyage {
            g.restore(world, v);
        }
        g
    }

    /// orig: `UnlockManager.Restore()`: the steed, coins and coat of arms,
    /// and everyone off the boat, near the monarch.
    fn restore(&mut self, world: &WorldData, v: &crate::progress::Voyage) {
        use crate::data::Role;
        if let Some(def) = world.steeds.get(v.steed as usize) {
            self.steed = v.steed;
            self.monarch.steed = SteedStats::from_def(def);
            self.steed_ability = def.ability_cooldown;
        }
        self.wallet.set(v.coins);
        self.coat = v.coat;
        let x = self.monarch.x;
        for (role, n) in [(Role::Archer, v.archers), (Role::Worker, v.workers)] {
            for _ in 0..n {
                let dx = self.rng.range(-2.0, 2.0);
                self.population.spawn(role, x + dx, &mut self.rng);
            }
        }
        for _ in 0..v.knights {
            let dx = self.rng.range(-2.0, 2.0);
            self.population.spawn_knight(x + dx, &mut self.rng);
        }
        if let Some(h) = v.hermit.filter(|h| (*h as usize) < world.protos.len()) {
            let dx = self.rng.range(-2.0, 2.0);
            self.hermits
                .spawn(world, &mut self.level, h, x + dx, &mut self.rng);
        }
        if let Some(c) = v.dog {
            let dx = self.rng.range(-2.0, 2.0);
            self.dogs.adopt(x + dx, c);
        }
        self.reign = v.reign;
        self.payables_dirty = true;
    }

    fn with_rng(
        world: &WorldData,
        level_index: usize,
        rng: Rng,
        progress: &crate::progress::Progress,
    ) -> Self {
        let mut rng = rng;
        let coat = CoatOfArms::random(&mut rng);
        let unlocks = progress.block_unlocks(world, level_index);
        let unlocks = match &unlocks {
            Some((ids, force)) => Unlocks::Only { ids, force: *force },
            None => Unlocks::All,
        };
        let mut level = level::generate(world, level_index, &unlocks, &mut rng);
        // The monarch is a placeholder in the PlayerSpawn block; take its
        // position and drop the object (we draw the monarch ourselves).
        let spawn = level
            .objects
            .iter()
            .position(|o| world.protos[o.proto as usize].name == "Player");
        let start_x = spawn.map_or(0.0, |i| level.objects.remove(i).x);
        let clock = Clock::new();
        let mut wallet = Wallet::default();
        wallet.set(world.levels[level_index].starting_coins.max(0) as u32);
        let director = Director::new(world, level_index, &mut rng);
        let forests = build::settle_forests(world, &mut level);
        let meadows = Meadows::from_level(world, &mut level, &forests, 0.0, &mut rng);
        let mut mounts = crate::mounts::Mounts::default();
        mounts.spawn_initial(world, &mut level, &mut rng);
        let steed = world.default_steed;
        let steed_stats = world
            .steeds
            .get(steed as usize)
            .map_or(SteedStats::HORSE_REGULAR, SteedStats::from_def);
        let population = Population::from_level(world, &mut level, &mut rng);
        let campfire = kingdom::campfire_x(world, &level);
        let borders = kingdom::borders(world, &level, campfire);
        let days = DayCounter::new(&clock);
        let free_parts = world.levels[level_index].free_boat_parts;
        let harbor = crate::boat::Harbor::new(world, &mut level, free_parts, &mut rng);
        // orig: Signpost.Start: the map shows only if something is left to find here.
        let unlockable = progress.unlockable_on(world, level_index);
        for o in &mut level.objects {
            if let Some(p) = world.protos[o.proto as usize].signpost {
                if let Some(part) = o.parts.get_mut(p as usize) {
                    part.visible = !unlockable.is_empty();
                }
            }
        }
        Self {
            safe: days.is_daytime,
            days,
            clock,
            monarch: Monarch::new(start_x, steed_stats),
            wallet,
            coins: Coins::default(),
            payables: Vec::new(),
            pay: PayMachine::default(),
            rng,
            level_index,
            level,
            director,
            blender: Blender::new(world),
            season: Season::Summer,
            tech: Tech::default(),
            forests,
            population,
            campfire,
            borders,
            greed: Greed::default(),
            has_crown: true,
            game_over: false,
            won: false,
            farms: Farms::default(),
            season_blend: 0.0,
            season_target: 0.0,
            farmer_statue: false,
            archer_statue: false,
            worker_statue: false,
            knight_statue: false,
            statues: Default::default(),
            farmers_seen: 0,
            fading: Vec::new(),
            meadows,
            wildlife: Wildlife::default(),
            mounts,
            hermits: Default::default(),
            merchants: Default::default(),
            treasure: Default::default(),
            siege: Default::default(),
            teleports: Default::default(),
            dogs: Default::default(),
            workshops: Vec::new(),
            passenger: None,
            steed,
            steed_ability: world
                .steeds
                .get(steed as usize)
                .map_or(15.0, |s| s.ability_cooldown),
            poop: (0, 0.0),
            steed_attack: 0.0,
            coat,
            free_slots: Vec::new(),
            ticks: 0,
            became_safe: false,
            payables_dirty: true,
            harbor,
            unlockable,
            queued_unlock: None,
            reign: 0,
            voyage: None,
            monarch_y: 0.0,
            queued: Vec::new(),
            autosave_in: None,
            bag: Default::default(),
            bag_rng: Rng::new(0x5eed_ba95),
            tutorial: Default::default(),
            look: Default::default(),
            god: false,
        }
    }

    /// Recomputes the payable list after build sites or forests change.
    fn refresh_payables(&mut self, world: &WorldData) {
        self.borders = kingdom::borders(world, &self.level, self.campfire);
        self.move_border_banners(world);
        let pop = &self.population;
        let statues = &self.statues;
        let hermits = &self.hermits;
        let merchants = &self.merchants;
        let siege = &self.siege;
        let teleports = &self.teleports;
        let safe = self.safe;
        let mounts = &self.mounts;
        let steed = self.steed;
        let level = &self.level;
        let portals = self.portal_infos(world);
        let campfire = self.campfire;
        let harbor = &self.harbor;
        let signpost_open = !self.unlockable.is_empty() && self.queued_unlock.is_none();
        // orig: Knight.CanCharge: the boat on its side with room for a knight.
        let boat_side = harbor
            .boat_at(level)
            .filter(|_| {
                harbor.boat_mode(level) != Some(crate::boat::mode::SAILING)
                    && harbor.room(world, level, true)
            })
            .map(|b| b.1);
        self.payables =
            build::payables(
                world,
                &self.level,
                &self.forests,
                self.tech,
                |kind, o| match kind {
                    PayKind::Shop => world.protos[o.proto as usize]
                        .shop
                        .is_some_and(|s| pop.rack_count(o.id) < s.max_items),
                    PayKind::Shield => pop.shield_can_pay(o.id),
                    PayKind::Statue => statues.can_pay(world, level, o.id),
                    PayKind::Cabin => hermits.cabin_can_pay(o.id),
                    PayKind::Merchant => merchants.can_pay(o.id, safe),
                    PayKind::Workshop => siege.counter_can_pay(o.id),
                    PayKind::Teleporter => teleports.can_pay(o.id),
                    PayKind::Hermit => hermits.can_pay(o.id),
                    // orig: Steed.CanPay: standing still, and the ridden steed allows switching.
                    PayKind::Steed => {
                        mounts.can_pay(o.id)
                            && !world
                                .steeds
                                .get(steed as usize)
                                .is_some_and(|s| s.disable_switching)
                    }
                    // orig: PayableBorder.CanPay: a knight in range that can charge
                    // (a live portal on its side of the campfire).
                    PayKind::Border => {
                        let range = world.protos[o.proto as usize]
                            .border
                            .map_or(8.0, |b| b.knight_range);
                        let portal_on_side = portals
                            .iter()
                            .any(|p| p.alive && (p.x < campfire) == (o.x < campfire));
                        let side = if o.x < campfire { -1 } else { 1 };
                        (portal_on_side || boat_side == Some(side))
                            && !pop.knights_near(o.x, range).is_empty()
                    }
                    PayKind::Wharf => harbor.wharf_can_pay(),
                    PayKind::Beach => harbor.beach_can_pay(level),
                    PayKind::Signpost => signpost_open,
                    _ => true,
                },
            );
        self.payables_dirty = false;
    }

    /// Portals as units see them.
    fn portal_infos(&self, world: &WorldData) -> Vec<crate::units::PortalTarget> {
        self.portal_hit_infos(world)
            .into_iter()
            .filter_map(|p| {
                let o = &self.level.objects[self.level.index_of(p.object)?];
                let d = world.protos[o.proto as usize].portal?;
                Some(crate::units::PortalTarget {
                    id: p.object,
                    x: p.x,
                    x0: p.hit_box.0,
                    x1: p.hit_box.1,
                    cy: (p.hit_box.2 + p.hit_box.3) * 0.5,
                    hp: o.hp,
                    alive: o.hp > 0 && self.greed.portal_can_spawn(p.object),
                    by_arrows: d.by_arrows,
                    by_knights: d.by_knights,
                })
            })
            .collect()
    }

    /// orig: `Kingdom.intactWall`: the outermost wall on each side that's
    /// still above `minimumIntactHitPoints`.
    fn intact_walls(&self, world: &WorldData) -> [Option<f32>; 2] {
        let mut out = [None, None];
        for o in &self.level.objects {
            let Some(w) = world.protos[o.proto as usize].wall_def else {
                continue;
            };
            if build::under_construction(world, o)
                || (o.hp as f32) < w.hit_points as f32 * w.minimum_intact
            {
                continue;
            }
            let i = usize::from(o.x > self.campfire);
            let further = |cur: f32| if i == 0 { o.x < cur } else { o.x > cur };
            if out[i].is_none_or(further) {
                out[i] = Some(o.x);
            }
        }
        out
    }

    /// orig: `Kingdom.MoveBorderOnSide()`: once the castle stands and a
    /// border is past `minCampfireDistance`, a banner stands just outside it.
    fn move_border_banners(&mut self, world: &WorldData) {
        let Some(proto) = world.border else { return };
        let Some(def) = world.protos[proto as usize].border else {
            return;
        };
        let castle_built = self
            .level
            .objects
            .iter()
            .any(|o| world.protos[o.proto as usize].castle_level.is_some());
        for side in [-1i8, 1] {
            let border = self.borders.side(side);
            let far = (border - self.campfire) * side as f32 > def.min_campfire_distance;
            let existing = self
                .level
                .all()
                .position(|o| o.proto == proto && (o.x < self.campfire) == (side < 0));
            if castle_built && far {
                let x = border + side as f32 * def.distance_from_wall;
                match existing {
                    Some(i) => self.level.objects[i].x = x,
                    None => {
                        let p = &world.protos[proto as usize];
                        let z = p.fixed.and_then(|f| f.fix_z).unwrap_or(p.root_z);
                        let o = level::instantiate(
                            world,
                            proto,
                            crate::data::Layer::Game,
                            x,
                            0.0,
                            z,
                            side as f32,
                        );
                        self.level.add(o);
                    }
                }
            } else if let Some(i) = existing {
                self.level.objects.remove(i);
            }
        }
    }

    /// Rebuild the payable list on the next tick: for tests and scenarios
    /// that place objects with `level.add` after the island is generated.
    pub fn mark_payables_dirty(&mut self) {
        self.payables_dirty = true;
    }

    /// Debug: finish every scaffold now.
    pub fn complete_construction(&mut self, world: &WorldData) {
        let building: Vec<u32> = self
            .level
            .objects
            .iter()
            .filter(|o| build::under_construction(world, o))
            .map(|o| o.id)
            .collect();
        build::complete_all(world, &mut self.level);
        for id in building {
            self.started(world, id);
        }
        self.payables_dirty = true;
    }

    /// Dev: rebuilds the wreck into a wharf if needed, then fits every part,
    /// launches the boat and brings it to the beach, the monarch beside it.
    pub fn dev_boat_at_beach(&mut self, world: &WorldData) {
        if self.harbor.wharf.is_none() {
            let wreck = self.level.objects.iter().find_map(|o| {
                let next = world.protos[o.proto as usize].upgrade?.next?;
                world.protos[next as usize].wharf.is_some().then_some(o.id)
            });
            if let Some(u) = wreck.and_then(|id| build::upgrade(world, &mut self.level, id, None)) {
                self.started(world, u.new);
            }
        }
        self.harbor
            .dev_finish(world, &mut self.level, &mut self.rng);
        if let Some((x, dir)) = self.harbor.boat_at(&self.level) {
            self.monarch.x = x - dir as f32 * 4.0;
        }
        self.payables_dirty = true;
    }

    /// What's under the steed's front hooves. orig: `Player.Update()` checks the
    /// point half a unit ahead against the Forest and Grass layers.
    fn ground(&self) -> Ground {
        let front = self.monarch.x + 0.5 * self.monarch.facing as f32;
        if build::in_forest(&self.forests, front) {
            Ground::Forest
        } else if self.meadows.is_grass(front) {
            Ground::Grass
        } else {
            Ground::Normal
        }
    }

    /// orig: `World.winterness`.
    pub fn winterness(&self) -> f32 {
        (1.0 - (2.0 - self.season_blend).abs()).clamp(0.0, 1.0)
    }

    /// orig: `World.Update()`: the season eases over `seasonChangeDays`.
    fn update_season(&mut self, world: &WorldData, dt: f32) {
        let days = world.levels[self.level_index].season_change_days.max(0.01);
        let delta = 1.0 / (crate::time::SECONDS_PER_HOUR * 24.0 * days);
        if self.season_blend < self.season_target - delta {
            self.season_blend += delta * dt;
        } else if self.season_blend > self.season_target + delta {
            self.season_blend -= delta * dt;
        } else {
            self.season_blend = self.season_target;
        }
    }

    fn farm_infos(&self, world: &WorldData) -> Vec<FarmInfo> {
        self.farms
            .lands
            .iter()
            .filter_map(|l| {
                let o = &self.level.objects[self.level.index_of(l.object)?];
                let house = l.farmhouse.and_then(|h| {
                    let ho = &self.level.objects[self.level.index_of(h)?];
                    if build::under_construction(world, ho) {
                        return None;
                    }
                    Some((ho.x, world.protos[ho.proto as usize].farmhouse?.level))
                });
                Some(FarmInfo {
                    id: l.object,
                    x: o.x,
                    farmable: l.farmable,
                    house,
                })
            })
            .collect()
    }

    /// A building became active (built, or placed without construction):
    /// clear the grass under it. orig: `ClearSpaceOnStart.Start()`.
    fn started(&mut self, world: &WorldData, id: u32) {
        let Some(i) = self.level.index_of(id) else {
            return;
        };
        let p = &world.protos[self.level.objects[i].proto as usize];
        if build::under_construction(world, &self.level.objects[i]) {
            return;
        }
        if p.teleporter.is_some() {
            self.teleports.register(id);
        }
        if p.wharf.is_some() && self.harbor.wharf.is_none() {
            let free = world.levels[self.level_index].free_boat_parts;
            self.harbor
                .wharf_built(world, &mut self.level, id, free, &mut self.rng);
            self.payables_dirty = true;
            return;
        }
        let o = &self.level.objects[i];
        // orig: MasonShrine.Start → Kingdom.stoneBuildingUnlocked.
        if p.mason_shrine {
            self.tech.stone = true;
            self.payables_dirty = true;
        }
        // orig: TowerKnight.Start: a shield shop at its foot.
        if p.tower_knight {
            if let Some(shop) = world.shield_shop {
                let (layer, x, y, z) = (o.layer, o.x, o.y, o.z - 0.001);
                let s = level::instantiate(world, shop, layer, x, y, z, 1.0);
                self.level.add(s);
                self.payables_dirty = true;
            }
            return;
        }
        if !p.clear_grass {
            return;
        }
        if let Some(g) = p.gaps {
            let (a, b) = g.span(o.x, o.flip);
            self.meadows.clear(a, b, &mut self.level);
        }
    }

    /// Guard slots of built towers. orig: `GuardSlot` (inactive while building).
    fn slot_infos(&self, world: &WorldData) -> Vec<SlotInfo> {
        let mut out = Vec::new();
        for o in &self.level.objects {
            let p = &world.protos[o.proto as usize];
            if p.guard_slots.is_empty() || build::under_construction(world, o) {
                continue;
            }
            let dir = if o.flip { -1.0 } else { 1.0 };
            for (i, &(sx, sy)) in p.guard_slots.iter().enumerate() {
                out.push(SlotInfo {
                    id: (o.id, i as u8),
                    x: o.x + sx * dir,
                    y: o.y + sy - build::WALK_Y,
                });
            }
        }
        out
    }

    /// Newly free slots fetch an archer; at dusk every free slot does.
    /// orig: `GuardSlot.Start()` / `ExitArcher()` → `Kingdom.AddGuardSlot()`,
    /// `Kingdom.BecomeUnsafe()`.
    fn staff_towers(&mut self, slots: &[SlotInfo], dusk: bool) {
        let taken: Vec<(u32, u8)> = self.population.taken_slots().collect();
        let free: Vec<&SlotInfo> = slots.iter().filter(|s| !taken.contains(&s.id)).collect();
        for s in &free {
            if dusk || !self.free_slots.contains(&s.id) {
                self.population.fetch_archer_for_slot(s.id, s.x);
            }
        }
        if dusk {
            self.population.distribute_free_archers();
        }
        let taken: Vec<(u32, u8)> = self.population.taken_slots().collect();
        self.free_slots = slots
            .iter()
            .map(|s| s.id)
            .filter(|id| !taken.contains(id))
            .collect();
    }

    /// Steed abilities triggered by what it does. orig: `Steed.EatingDone()`
    /// (the unicorn drops coins), `Steed.OnStartGallop()` (the warhorse
    /// inspires citizens).
    fn steed_ability_event(&mut self, world: &WorldData, e: MonarchEvent) {
        let Some(def) = world.steeds.get(self.steed as usize) else {
            return;
        };
        match e {
            MonarchEvent::AteDone if def.poop_coins > 0 && self.steed_ability <= 0.0 => {
                self.poop = (def.poop_coins, 0.0);
            }
            MonarchEvent::StartedGallop if def.inspire_citizens && self.steed_ability <= 0.0 => {
                self.steed_ability = def.ability_cooldown;
                let (x, r, d) = (self.monarch.x, def.inspired_range, def.inspired_duration);
                for u in &mut self.population.units {
                    if (u.x - x).abs() <= r {
                        u.inspired = d;
                    }
                }
            }
            _ => {}
        }
    }

    /// Free steeds, the unicorn's coins and the bear's swipes.
    fn update_steeds(&mut self, world: &WorldData, dt: f32, events: &mut Vec<GameEvent>) {
        self.steed_ability -= dt;
        self.mounts
            .update(dt, world, &mut self.level, &mut self.rng);
        // orig: Steed.PoopCoins: one every 0.1 s from behind the steed, for the monarch.
        if self.poop.0 > 0 {
            self.poop.1 -= dt;
            if self.poop.1 <= 0.0 {
                self.poop = (self.poop.0 - 1, 0.1);
                let f = self.monarch.facing as f32;
                self.coins.drop(
                    Who::Thing,
                    (self.monarch.x - 0.5 * f, 0.4),
                    (self.rng.range(-1.5, -3.0) * f, 0.0),
                    crate::coins::Policy::Nobody,
                    Some(Who::Monarch),
                    None,
                );
            }
        }
        // orig: Steed.OnAnimTriggerAbility (bear): swipes while galloping.
        let Some(def) = world.steeds.get(self.steed as usize) else {
            return;
        };
        if !def.attacks_while_running || !self.monarch.is_running() {
            self.steed_attack = 0.0;
            return;
        }
        self.steed_attack -= dt;
        if self.steed_attack > 0.0 {
            return;
        }
        self.steed_attack = BEAR_SWIPE_INTERVAL;
        let f = self.monarch.facing as f32;
        let (hx, _, hw, _) = def.hit_box;
        let (a, b) = (self.monarch.x + hx * f, self.monarch.x + (hx + hw) * f);
        let (lo, hi) = (a.min(b), a.max(b));
        let mut hit = false;
        let trolls: Vec<usize> = (0..self.greed.trolls.len())
            .filter(|&i| {
                let t = &self.greed.trolls[i];
                !t.is_dead() && t.x > lo && t.x < hi
            })
            .collect();
        for i in trolls {
            hit = true;
            let mut ge = Vec::new();
            self.greed.hurt(i, def.attack_damage, &mut |e| ge.push(e));
            for effect in ge {
                self.apply_greed_effect(effect, events);
            }
        }
        let animals: Vec<u32> = self
            .wildlife
            .animals
            .iter()
            .filter(|w| !w.is_dead() && w.x > lo && w.x < hi)
            .map(|w| w.id)
            .collect();
        for id in animals {
            hit = true;
            let mut we = Vec::new();
            self.wildlife.hurt(
                id,
                def.attack_damage,
                self.monarch.x,
                &mut self.coins,
                &mut self.rng,
                &mut |e| we.push(e),
            );
            events.extend(we.into_iter().map(GameEvent::Wild));
        }
        if hit {
            self.monarch.stamina -= def.attack_stamina_cost;
        }
    }

    /// Loaded ballistas aim at the toughest greed in front and fire.
    /// orig: `Ballista.Update()` / `Fire()`.
    fn update_ballistas(&mut self, world: &WorldData, dt: f32) {
        const SPEED: f32 = 20.0;
        const PITCH_SPEED: f32 = 40.0;
        for oi in 0..self.level.objects.len() {
            let o = &self.level.objects[oi];
            let Some(b) = world.protos[o.proto as usize].ballista.as_ref() else {
                continue;
            };
            if build::under_construction(world, o) {
                continue;
            }
            // Show the winding. orig: bodySprite = reloadFrames[work × (n−1) / reloadWork].
            let n = b.frames.len();
            if let (Some(part), true) = (b.body_part, n > 0) {
                let f =
                    (o.reload.max(0) as usize * (n - 1) / b.reload_work.max(1) as usize).min(n - 1);
                let s = b.frames[f];
                if let Some(p) = self.level.objects[oi].parts.get_mut(part as usize) {
                    p.sprite = Some(s);
                }
            }
            let o = &self.level.objects[oi];
            if o.reload < b.reload_work {
                continue;
            }
            let dir = if o.flip { -1.0 } else { 1.0 };
            let (ox, oy) = (o.x + b.bolt_pos.0 * dir, o.y + b.bolt_pos.1 - build::WALK_Y);
            // orig: the scanner sees `shootRange` ahead, nothing nearer than `minRange`.
            let target = self
                .greed
                .trolls
                .iter()
                .filter(|t| {
                    let d = (t.x - o.x) * dir;
                    !t.is_dead() && d >= b.min_range && d <= b.range
                })
                .max_by_key(|t| t.hp)
                .map(|t| (t.x, t.y + 0.35));
            let Some((tx, ty)) = target else { continue };
            let g = crate::coins::GRAVITY * 0.7;
            let (dx, dy) = ((tx - ox) * dir, ty - oy);
            // orig: Util.ComputeTrajectoryAngle (low shot), clamped to angleRange.
            let v2 = SPEED * SPEED;
            let disc = v2 * v2 - g * (g * dx * dx + 2.0 * dy * v2);
            let want = if disc >= 0.0 {
                libm::atan2f(v2 - libm::sqrtf(disc), g * dx).to_degrees()
            } else {
                45.0
            }
            .clamp(b.angle.0, b.angle.1);
            let step = PITCH_SPEED * dt;
            let o = &mut self.level.objects[oi];
            o.aim = want.clamp(o.aim - step, o.aim + step);
            if (o.aim - want).abs() < 2.0 {
                let speed = self.rng.range(18.0, 20.0);
                let a = o.aim.to_radians();
                let (vx, vy) = (libm::cosf(a) * speed * dir, libm::sinf(a) * speed);
                o.reload = 0;
                self.greed.fire_bolt(ox, oy, vx, vy, b.damage, b.max_hits);
            }
        }
    }

    fn update_siege(&mut self, world: &WorldData, dt: f32, events: &mut Vec<GameEvent>) {
        // orig: the Workshop's counters exist as soon as it does.
        let new: Vec<u32> = self
            .level
            .objects
            .iter()
            .filter(|o| !world.protos[o.proto as usize].workshop.is_empty())
            .filter(|o| !build::under_construction(world, o) && !self.workshops.contains(&o.id))
            .map(|o| o.id)
            .collect();
        for id in new {
            self.workshops.push(id);
            crate::siege::Siege::place_counters(world, &mut self.level, id);
            self.payables_dirty = true;
        }
        let pushers: Vec<(u32, usize)> = self
            .siege
            .catapults
            .iter()
            .map(|c| {
                let n = self
                    .population
                    .units
                    .iter()
                    .filter(|u| matches!(u.ai, crate::units::Ai::Work { target, working: true, .. } if target == c.object))
                    .count();
                (c.object, n)
            })
            .collect();
        let enemies: Vec<(u32, f32)> = self
            .greed
            .trolls
            .iter()
            .filter(|t| !t.is_dead())
            .map(|t| (t.id, t.x))
            .collect();
        let hits = self.siege.update(
            dt,
            world,
            &mut self.level,
            self.borders,
            &pushers,
            &enemies,
            &mut self.rng,
        );
        for crate::siege::BoulderHit::Troll(id, damage) in hits {
            if let Some(i) = self.greed.trolls.iter().position(|t| t.id == id) {
                let mut ge = Vec::new();
                self.greed.hurt(i, damage, &mut |e| ge.push(e));
                for effect in ge {
                    self.apply_greed_effect(effect, events);
                }
            }
        }
    }

    fn update_teleports(&mut self, world: &WorldData, dt: f32, flying: bool, dir: i8, go: bool) {
        let mut actions = Vec::new();
        let (dir, go) = if flying { (dir, go) } else { (0, false) };
        self.teleports.update(dt, dir, go, |a| actions.push(a));
        for a in actions {
            match a {
                crate::teleport::TeleportAction::MoveMonarch(x) => self.monarch.x = x,
                // orig: TeleporterExit.DoTeleport: a rift unless price tags are within 1.
                crate::teleport::TeleportAction::LeaveRift { x, entrance } => {
                    let clear = !self.payables.iter().any(|p| (p.x - x).abs() < 1.0);
                    if let (true, Some(rift)) = (clear, world.rift_proto) {
                        let p = &world.protos[rift as usize];
                        let o = level::instantiate(
                            world,
                            rift,
                            crate::data::Layer::Game,
                            x,
                            0.0,
                            p.root_z,
                            1.0,
                        );
                        let id = self.level.add(o);
                        self.teleports.rift_left(id, entrance);
                    }
                }
                crate::teleport::TeleportAction::RemoveRift(id) => {
                    if let Some(i) = self.level.index_of(id) {
                        if world.protos[self.level.objects[i].proto as usize].rift {
                            self.level.objects.remove(i);
                        }
                    }
                }
            }
            self.payables_dirty = true;
        }
    }

    fn update_dog(&mut self, world: &WorldData, dt: f32) {
        self.dogs
            .check_rescue(world, &mut self.level, &self.coins, &mut self.rng);
        if self.became_safe {
            self.dogs.became_safe();
        }
        let enemies: Vec<f32> = self
            .greed
            .trolls
            .iter()
            .filter(|t| !t.is_dead())
            .map(|t| t.x)
            .collect();
        let ctx = crate::dog::DogCtx {
            monarch_x: self.monarch.x,
            monarch_running: self.monarch.is_running(),
            enemies: &enemies,
        };
        self.dogs.update(dt, &ctx, &mut self.rng);
    }

    fn update_treasure(&mut self, world: &WorldData, dt: f32) {
        let x = self.treasure.banker.as_ref().map_or(self.campfire, |b| b.x);
        let ctx = crate::treasure::BankCtx {
            campfire: self.campfire,
            daytime: self.days.is_daytime,
            safe: self.safe,
            enemies_near: self
                .greed
                .trolls
                .iter()
                .any(|t| !t.is_dead() && (t.x - x).abs() <= 5.0),
            monarch_x: self.monarch.x,
            monarch_running: self.monarch.is_running(),
            monarch_coins: self.wallet.coins(),
        };
        let income = world.levels[self.level_index].income_multiplier;
        self.treasure.update(
            dt,
            world,
            &mut self.level,
            &ctx,
            income,
            &mut self.coins,
            &mut self.rng,
        );
    }

    fn update_merchants(&mut self, world: &WorldData, dt: f32) {
        let payables: Vec<(u32, f32)> = self.payables.iter().map(|p| (p.id.0, p.x)).collect();
        let castle_built = self
            .level
            .objects
            .iter()
            .any(|o| world.protos[o.proto as usize].castle_level.is_some());
        let ctx = crate::merchant::MerchantCtx {
            world,
            castle_built,
            campfire: self.campfire,
            borders: self.borders,
            safe: self.safe,
            became_safe: self.became_safe,
            monarch_x: self.monarch.x,
            monarch_running: self.monarch.is_running(),
            payables: &payables,
            income: world.levels[self.level_index].income_multiplier,
        };
        let before = self
            .merchants
            .list
            .iter()
            .map(|m| m.state)
            .collect::<Vec<_>>();
        self.merchants
            .update(dt, &ctx, &mut self.level, &mut self.coins, &mut self.rng);
        // A merchant halting or leaving changes what's payable.
        if self.merchants.list.iter().map(|m| m.state).ne(before) {
            self.payables_dirty = true;
        }
    }

    /// orig: `Player.DumpPassenger()`.
    fn dump_passenger(&mut self) {
        if let Some(h) = self.passenger.take() {
            self.hermits
                .dismount(&mut self.level, h, self.monarch.x, &mut self.rng);
            self.tech.passenger = None;
            self.payables_dirty = true;
        }
    }

    fn update_hermits(&mut self, dt: f32) {
        let payables: Vec<(u32, f32)> = self.payables.iter().map(|p| (p.id.0, p.x)).collect();
        let ctx = crate::hermits::HermitCtx {
            borders: self.borders,
            campfire: self.campfire,
            daytime: self.days.is_daytime,
            monarch_x: self.monarch.x,
            selected: self.pay.selected().map(|p| p.0),
            payables: &payables,
        };
        if self
            .hermits
            .update(dt, &ctx, &mut self.level, &mut self.rng)
        {
            self.dump_passenger();
        }
    }

    /// orig: `Steed.Pay()` → `Player.Ride()`: the old steed is left free here.
    fn ride(&mut self, world: &WorldData, object: u32) {
        self.dump_passenger();
        let Some(new) = self.mounts.take(&mut self.level, object) else {
            return;
        };
        let old = self.steed;
        self.mounts.place(
            world,
            &mut self.level,
            old,
            self.monarch.x,
            self.monarch.facing,
        );
        self.steed = new;
        let def = &world.steeds[new as usize];
        self.monarch.steed = SteedStats::from_def(def);
        self.monarch.stamina = 1.0;
        self.monarch.tired_timer = 0.0;
        self.steed_ability = 0.0;
        self.payables_dirty = true;
    }

    /// orig: `Statue.Update()` → `Kingdom.SetCharged()`.
    fn update_statues(&mut self, world: &WorldData, dt: f32) {
        use crate::data::Deity;
        let mut changes = Vec::new();
        self.statues
            .update(dt, world, &mut self.level, |b| changes.push(b));
        for b in changes {
            match b.deity {
                Deity::Archer => self.archer_statue = b.on,
                Deity::Worker => self.worker_statue = b.on,
                Deity::Knight => self.knight_statue = b.on,
                Deity::Farmer => self.farmer_statue = b.on,
                // orig: the time statue pushes back a day limit; islands here have none.
                Deity::Time => {}
            }
            // orig: Wall.HandleOnStatueCharged → AddWork if it can take more.
            if b.on {
                self.list_wall_repairs(world);
            }
            self.payables_dirty = true;
        }
    }

    /// Damaged walls become builders' jobs. orig: `Wall.HandleOnBecameSafe()`
    /// and friends → `Kingdom.AddWork()`.
    fn list_wall_repairs(&mut self, world: &WorldData) {
        let statue = self.worker_statue;
        for o in &mut self.level.objects {
            if crate::units::wall_needs_repair(world, o, statue) {
                o.repair = true;
            }
        }
    }

    /// orig: `Kingdom.CalculateForestEdges()`: the forest edges nearest the campfire.
    fn closest_forest(&self) -> (f32, f32) {
        let (mut l, mut r) = (self.level.left, self.level.left + self.level.width);
        for &(a, b) in &self.forests {
            if b < self.campfire && b > l {
                l = b;
            }
            if a > self.campfire && a < r {
                r = a;
            }
        }
        (l, r)
    }

    /// A forest lost an item: recompute, and fade decorations left at the edge.
    /// orig: `Forest.RemoveAndUpdate()` → `UpdateRegion()`.
    fn forest_changed(&mut self, world: &WorldData) {
        self.forests = build::forests(world, &self.level);
        self.payables_dirty = true;
        for id in build::forest_edge_items(world, &self.level, &self.forests) {
            if !self.fading.iter().any(|f| f.0 == id) {
                // orig: removeDelay (10) × Random.Range(0.5, 1.5), then a 1 s fade.
                let t = 10.0 * self.rng.range(0.5, 1.5) + 1.0;
                self.fading.push((id, t));
            }
        }
    }

    fn update_fading(&mut self, world: &WorldData, dt: f32) {
        let mut removed = false;
        self.fading.retain_mut(|f| {
            f.1 -= dt;
            if f.1 > 0.0 {
                return true;
            }
            removed |= self.level.remove_any(f.0);
            false
        });
        if removed {
            self.forest_changed(world);
        }
    }

    fn payable_index(&self, id: PayableId) -> Option<usize> {
        self.payables.iter().position(|p| p.id == id)
    }

    pub fn tick(&mut self, world: &WorldData, controls: &Controls, events: &mut Vec<GameEvent>) {
        crate::prof::start();
        let dt = TICK;
        // orig: Game.EndGame → SendInputTo(null).
        let idle = Controls::default();
        // orig: Game.SendInputTo(TeleporterExit) while the exit is flying.
        let flying = self.teleports.exit.is_some();
        let (exit_dir, exit_go) = (controls.direction, controls.pay_down);
        let controls = if self.game_over || self.won || flying {
            &idle
        } else {
            controls
        };
        self.wallet.bottomless = self.god;
        // Never more: a fuller bag spills, and a spilled coin wouldn't leave.
        if self.god && self.wallet.coins() != GOD_COINS {
            self.wallet.set(GOD_COINS);
        }
        // The border banner's price depends on where knights are.
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks.is_multiple_of(30) && self.population.count(crate::data::Role::Knight) > 0 {
            self.payables_dirty = true;
        }
        if self.payables_dirty {
            self.refresh_payables(world);
        }
        crate::prof::mark("payables");
        let ground = self.ground();

        let mut steed_events = Vec::new();
        self.monarch
            .update(dt, controls, ground, &mut self.rng, |e| {
                steed_events.push(e)
            });
        for e in steed_events {
            self.steed_ability_event(world, e);
            events.push(GameEvent::Monarch(e));
        }
        crate::prof::mark("monarch");
        self.update_steeds(world, dt, events);
        self.update_hermits(dt);
        self.update_merchants(world, dt);
        self.update_treasure(world, dt);
        self.update_siege(world, dt, events);
        self.update_harbor(world, dt, events);
        self.update_teleports(world, dt, flying, exit_dir, exit_go);
        self.update_dog(world, dt);
        // Consumed by the merchant and the dog.
        self.became_safe = false;
        self.update_ballistas(world, dt);

        crate::prof::mark("systems");
        let target = choose_target(
            &self.payables,
            self.pay.selected(),
            self.monarch.x,
            self.monarch.is_running(),
        )
        .map(|i| self.payables[i].target());
        let input = PayInput {
            dt,
            key: controls.pay,
            key_down: controls.pay_down,
            target,
            coins: self.wallet.coins(),
        };
        let mut pay_events: [Option<PayEvent>; 6] = [None; 6];
        let mut n = 0;
        self.pay.update(&input, |e| {
            assert!(n < pay_events.len(), "pay machine emitted too many events");
            pay_events[n] = Some(e);
            n += 1;
        });
        for e in pay_events.into_iter().flatten() {
            events.push(GameEvent::Pay(e));
            if let Some(u) = self.apply_pay_event(world, e) {
                self.farms.remap_house(u.old, u.new);

                // orig: Castle.Start (Castle4): a banker steps out.
                let banker = self
                    .level
                    .index_of(u.new)
                    .and_then(|i| world.protos[self.level.objects[i].proto as usize].banker)
                    .and_then(|b| world.protos[b as usize].banker_def);
                if let Some(def) = banker {
                    let x = self
                        .level
                        .index_of(u.new)
                        .map_or(self.campfire, |i| self.level.objects[i].x);
                    self.treasure.spawn_banker(def, x, &mut self.rng);
                }
                // orig: Castle6's shield shops wait for its keep.
                if u.castle_level.is_some() {
                    let keep = self.level.index_of(u.new).map_or(0.0, |i| {
                        world.protos[self.level.objects[i].proto as usize].keep_time
                    });
                    self.population.register_shield_shops(
                        world,
                        &self.level,
                        self.campfire,
                        self.coat,
                        keep,
                        &mut self.rng,
                    );
                }
                self.started(world, u.new);
                events.push(GameEvent::Upgraded(u));
            }
        }

        crate::prof::mark("pay");
        // Sites finished this tick. Two builders can land the last stroke on
        // the same site in one tick; it's built (and `started`) once.
        let mut built_now: Vec<u32> = Vec::new();
        for id in build::step(world, &mut self.level, dt) {
            self.payables_dirty = true;
            self.started(world, id);
            built_now.push(id);
            events.push(GameEvent::Built(id));
        }
        if self.level.objects.iter().any(|o| o.cooldown > 0.0) {
            self.payables_dirty = true;
        }

        if controls.drop_coin {
            self.coins
                .drop_from(&self.monarch, &mut self.wallet, &mut self.rng);
            events.push(GameEvent::Pay(PayEvent::DropCoin));
        }

        let enemies: Vec<(u32, f32, f32, i32)> = self
            .greed
            .trolls
            .iter()
            .filter(|t| !t.is_dead())
            .map(|t| (t.id, t.x, t.mover.vx, t.hp))
            .collect();
        let walls: Vec<f32> = self
            .level
            .objects
            .iter()
            .filter(|o| world.protos[o.proto as usize].wall)
            .map(|o| o.x)
            .collect();
        crate::prof::mark("build");
        self.update_season(world, dt);
        self.update_fading(world, dt);
        self.update_statues(world, dt);
        let tutorial_ctx = crate::tutorial::TutorialCtx {
            world,
            monarch_x: self.monarch.x,
            campfire: self.campfire,
            population: &self.population,
        };
        if self.tutorial.update(dt, &tutorial_ctx, &mut self.level) {
            events.push(GameEvent::TutorialDone);
        }
        let farm_ctx = FarmCtx {
            world,
            winterness: self.winterness(),
            statue: self.farmer_statue,
            forests: &self.forests,
        };
        self.farms.update(
            dt,
            &farm_ctx,
            &mut self.level,
            &mut self.coins,
            &mut self.rng,
        );
        // orig: Kingdom.ReassignFarms on farmer/farmland changes and on becoming safe.
        let farmers = self.population.count(crate::data::Role::Farmer);
        if self.farms.changed || farmers != self.farmers_seen {
            self.farms.changed = false;
            self.farmers_seen = farmers;
            let lands = self.farms.farmable(&self.level);
            self.population.assign_farms(&lands);
        }
        crate::prof::mark("season_farms");
        let winterness = self.winterness();
        self.meadows.update(
            dt,
            world,
            &mut self.level,
            &self.forests,
            self.borders,
            winterness,
            &mut self.rng,
        );
        crate::prof::mark("meadows");
        let thickets: Vec<(u32, f32)> = self.meadows.thickets().collect();
        let citizens: Vec<(f32, bool)> = self
            .population
            .units
            .iter()
            .filter(|u| u.role != crate::data::Role::Beggar)
            .map(|u| (u.x, u.role == crate::data::Role::Archer))
            .collect();
        let wild_ctx = WildCtx {
            forests: &self.forests,
            thickets: &thickets,
            winterness: self.winterness(),
            monarch_x: self.monarch.x,
            monarch_moving: self.monarch.velocity.abs() > 0.01,
            monarch_running: self.monarch.is_running(),
            attracts_deer: world
                .steeds
                .get(self.steed as usize)
                .is_some_and(|s| s.attracts_deer),
            citizens: &citizens,
        };
        let bounds = (self.level.left, self.level.left + self.level.width);
        let mut wild_events: Vec<WildEvent> = Vec::new();
        self.wildlife
            .update(dt, &wild_ctx, bounds, &mut self.rng, |e| {
                wild_events.push(e)
            });
        events.extend(wild_events.into_iter().map(GameEvent::Wild));
        let wildlife: Vec<(u32, f32, f32, bool, f32)> = self
            .wildlife
            .animals
            .iter()
            .filter(|a| !a.is_dead())
            .map(|a| {
                let (_, y0, _, y1) = a.hit_box();
                (
                    a.id,
                    a.x,
                    a.mover.vx,
                    a.species == Species::Deer,
                    (y0 + y1) * 0.5,
                )
            })
            .collect();
        crate::prof::mark("wildlife");
        let closest_forest = self.closest_forest();
        let slots = self.slot_infos(world);
        self.staff_towers(&slots, false);
        let farm_infos = self.farm_infos(world);
        self.population.register_shield_shops(
            world,
            &self.level,
            self.campfire,
            self.coat,
            0.0,
            &mut self.rng,
        );
        let portal_infos = self.portal_infos(world);
        let intact = self.intact_walls(world);
        let knights = crate::units::KnightCtx {
            intact_wall: intact,
            banner: [-1i8, 1].map(|side| {
                world.border.and_then(|b| {
                    self.level
                        .objects
                        .iter()
                        .find(|o| o.proto == b && (o.x < self.campfire) == (side < 0))
                        .map(|o| o.x)
                })
            }),
        };
        // Night shelter: inside the outermost built, intact walls.
        // With no such wall on a side (only scaffolding, or nothing), the
        // camp's minimum extent.
        let near = kingdom::MIN_KINGDOM_EXTENTS;
        let shelter = kingdom::Borders {
            left: intact[0]
                .unwrap_or(self.campfire - near)
                .max(self.borders.left),
            right: intact[1]
                .unwrap_or(self.campfire + near)
                .min(self.borders.right),
        };
        let ctx = Ctx {
            world,
            farmlands: &farm_infos,
            wildlife: &wildlife,
            closest_forest,
            slots: &slots,
            archer_statue: self.archer_statue,
            knight_statue: self.knight_statue,
            worker_statue: self.worker_statue,
            coat: self.coat,
            portals: &portal_infos,
            knights,
            borders: self.borders,
            shelter,
            campfire: self.campfire,
            daytime: self.days.is_daytime,
            safe: self.safe,
            monarch_x: self.monarch.x,
            monarch_running: self.monarch.is_running(),
            forests: &self.forests,
            enemies: &enemies,
            walls: &walls,
            boat: self
                .harbor
                .boat_at(&self.level)
                .map(|(x, dir)| crate::units::BoatTarget {
                    x,
                    dir,
                    mode: self.harbor.boat_mode(&self.level).unwrap_or(0),
                    knight_room: self.harbor.room(world, &self.level, true),
                }),
        };
        crate::prof::mark("ctx");
        let mut unit_events: Vec<UnitEvent> = Vec::new();
        self.population.update(
            dt,
            &ctx,
            &mut self.level,
            &mut self.coins,
            &mut self.rng,
            |e| unit_events.push(e),
        );

        crate::prof::mark("population");
        let mut grabbers = Vec::with_capacity(1 + self.population.units.len());
        grabbers.push(Grabber::monarch(&self.monarch, &self.wallet));
        grabbers.extend(self.population.grabbers());
        // orig: the banker's Wallet grabs only the monarch's coins.
        if let Some((x, room)) = self.treasure.banker_room() {
            grabbers.push(Grabber {
                who: Who::Banker,
                x,
                y: 0.0,
                flip: false,
                rect: crate::coins::Rect::centered(0.32, 0.33, 0.64),
                purse: (x, 0.2),
                only_monarch_coins: true,
                room,
            });
        }
        let mut picked: Vec<Who> = Vec::new();
        self.coins.update(dt, &grabbers, |e| {
            if let CoinEvent::PickedUp(who) = e {
                picked.push(who);
            }
            events.push(GameEvent::Coin(e))
        });
        for who in picked {
            match who {
                // God mode's purse stays put, short of spilling.
                Who::Monarch if self.god && self.wallet.coins() >= GOD_COINS => {}
                Who::Monarch => self.wallet.set(self.wallet.coins() + 1),
                Who::Unit(id) => self
                    .population
                    .credit(id, &mut self.rng, &mut |e| unit_events.push(e)),
                Who::Banker => self.treasure.credit_banker(),
                Who::Thing | Who::Wildlife => {}
            }
        }
        crate::prof::mark("coins");
        self.update_bag(world, dt, events);
        for e in unit_events {
            match e {
                UnitEvent::TreeChopped(_) => self.forest_changed(world),
                UnitEvent::BuildPoint(id) => {
                    // A stroke changes nothing a payable shows (price, can-pay);
                    // refreshing on every stroke rebuilt the whole list at up to
                    // 60 Hz while builders hammer. The refresh those need happens
                    // when the site finishes or is paid for.
                    let done = self
                        .level
                        .index_of(id)
                        .is_some_and(|i| !build::under_construction(world, &self.level.objects[i]));
                    if done && !built_now.contains(&id) {
                        built_now.push(id);
                        self.payables_dirty = true;
                        self.started(world, id);
                        events.push(GameEvent::Built(id));
                    }
                }
                UnitEvent::Promoted { .. } | UnitEvent::KnightLost { .. } => {
                    self.payables_dirty = true
                }
                UnitEvent::Wound(id) => {
                    let wharf = self.harbor.wharf.as_ref().is_some_and(|w| w.object == id);
                    if wharf {
                        if let Some(e) = self.harbor.work(world, &mut self.level) {
                            self.payables_dirty = true;
                            events.push(GameEvent::Harbor(e));
                        }
                    } else {
                        crate::siege::Siege::crank(world, &mut self.level, id)
                    }
                }
                UnitEvent::Embark(id) => self.embark(world, id),
                UnitEvent::StrikePortal { portal, damage } => {
                    self.hurt_portal(world, portal, damage, events)
                }
                UnitEvent::Strike { troll, damage, .. } => {
                    if let Some(i) = self.greed.trolls.iter().position(|t| t.id == troll) {
                        let mut ge = Vec::new();
                        self.greed.hurt(i, damage, &mut |e| ge.push(e));
                        for effect in ge {
                            self.apply_greed_effect(effect, events);
                        }
                    }
                }
                UnitEvent::Farmed(id) => {
                    let farm_ctx = FarmCtx {
                        world,
                        winterness: self.winterness(),
                        statue: self.farmer_statue,
                        forests: &self.forests,
                    };
                    self.farms
                        .receive_farming(&farm_ctx, &mut self.level, id, &mut self.rng);
                    // orig: Farmland.ReceiveFarming → ClearSpaceOnStart.ClearNow (grass).
                    if let Some(i) = self.level.index_of(id) {
                        let o = &self.level.objects[i];
                        if let Some(g) = world.protos[o.proto as usize].gaps {
                            let (a, b) = g.span(o.x, o.flip);
                            self.meadows.clear(a, b, &mut self.level);
                        }
                    }
                }
                UnitEvent::Arrow {
                    x,
                    y,
                    vx,
                    vy,
                    missed,
                    perfect,
                } => self.greed.shoot(x, y, vx, vy, missed, perfect),
                _ => {}
            }
            events.push(GameEvent::Unit(e));
        }

        crate::prof::mark("bag_events");
        self.clock.advance(dt);
        let mut outputs: Vec<DirectorOutput> = Vec::new();
        self.director
            .update(world, &self.clock, &mut self.blender, &mut self.rng, |o| {
                outputs.push(o)
            });
        for o in outputs {
            match o {
                DirectorOutput::Season(s) => {
                    self.season = s;
                    // orig: World.GoToSeason.
                    self.season_target = match s {
                        Season::Summer => 0.0,
                        Season::Autumn => 1.0,
                        Season::Winter => 2.0,
                    };
                    events.push(GameEvent::Season(s));
                }
                DirectorOutput::Wave(k) => {
                    self.greed.schedule(
                        world,
                        &self.level,
                        self.level_index,
                        k,
                        self.season,
                        self.clock.day,
                        self.campfire,
                        self.borders,
                        &mut self.rng,
                    );
                    events.push(GameEvent::WaveDue(k));
                }
                DirectorOutput::Track(t) => events.push(GameEvent::Music(t)),
            }
        }
        crate::prof::mark("director");
        self.update_greed(world, dt, events);
        self.arrows_hit_wildlife(events);
        crate::prof::mark("greed");
        self.blender.update(world, dt);
        if let Some(e) = self.days.update(&self.clock) {
            match e {
                // orig: EnemyManager.HandleOnDayStart (greed retreat), Kingdom.CheckBecomeSafe.
                DayEvent::Started { .. } => {
                    self.greed.dawn();
                    self.treasure.dawn();
                    self.safe = !self.greed.is_dangerous(self.borders);
                    self.became_safe = self.safe;
                    self.farms.changed |= self.safe;
                    if self.safe {
                        self.list_wall_repairs(world);
                    }
                }
                // orig: Kingdom.BecomeUnsafe at dusk.
                DayEvent::Ended => {
                    self.safe = false;
                    let slots = self.slot_infos(world);
                    self.staff_towers(&slots, true);
                }
            }
            events.push(GameEvent::Day(e));
        }
        // orig: EnemyManager.CheckDanger → Kingdom.CheckBecomeSafe once the attack is over.
        if !self.safe && self.days.is_daytime && !self.greed.is_dangerous(self.borders) {
            self.safe = true;
            self.became_safe = true;
            self.farms.changed = true;
            self.list_wall_repairs(world);
        }
        if self.became_safe && self.clock.day > 0 {
            self.autosave_in = Some(2.0);
        }
        if let Some(t) = &mut self.autosave_in {
            *t -= dt;
            if *t <= 0.0 {
                self.autosave_in = None;
                if !self.game_over && !self.won {
                    events.push(GameEvent::AutoSave);
                }
            }
        }
        crate::prof::mark("days");
        events.append(&mut self.queued);
    }

    /// orig: `Wallet.ShowCoinBag()` → `CoinBag`; a coin over the rim is
    /// `Player.CoinFellFromBag()`: dropped, half the time into the water.
    fn update_bag(&mut self, world: &WorldData, dt: f32, events: &mut Vec<GameEvent>) {
        let mut out = Vec::new();
        self.bag.update(
            dt,
            &world.bag,
            self.wallet.coins(),
            &mut self.bag_rng,
            |e| out.push(e),
        );
        for e in out {
            if e == crate::bag::BagEvent::Spilled {
                self.coins
                    .drop_from(&self.monarch, &mut self.wallet, &mut self.bag_rng);
                // orig: overflowCoinsWaterProbability 0.5 → Droppable.DropInWater.
                if self.bag_rng.value() < 0.5 {
                    if let Some(c) = self.coins.list.last_mut() {
                        c.motion = crate::coins::Motion::Fading {
                            t: 0.0,
                            duration: 1.0,
                        };
                    }
                }
            }
            events.push(GameEvent::Bag(e));
        }
    }

    /// The wharf's loose parts, the boat, and the monarch aboard once it sails.
    fn update_harbor(&mut self, world: &WorldData, dt: f32, events: &mut Vec<GameEvent>) {
        let boat = self.harbor.boat.as_ref().map(|b| b.object);
        let pushers = boat.map_or(0, |b| {
            self.population
                .units
                .iter()
                .filter(|u| {
                    u.pushing
                        && matches!(u.ai, crate::units::Ai::Work { target, working: true, .. } if target == b)
                })
                .count()
        });
        // orig: Boat.Update: a forest collider at (x, y + 3) slows the pushing.
        let in_forest = self
            .harbor
            .boat_at(&self.level)
            .is_some_and(|(x, _)| self.forests.iter().any(|&(a, b)| a <= x && x <= b));
        let mut out = Vec::new();
        self.harbor
            .update(dt, world, &mut self.level, pushers, in_forest, |e| {
                out.push(e)
            });
        if !out.is_empty() {
            self.payables_dirty = true;
        }
        events.extend(out.into_iter().map(GameEvent::Harbor));
        // orig: Boat.SailAway: the monarch rides on deck.
        if self.harbor.boat_mode(&self.level) == Some(crate::boat::mode::SAILING) {
            if let Some((x, y)) = self.harbor.deck_pos(&self.level, crate::boat::MONARCH_DECK) {
                self.monarch.x = x;
                self.monarch.velocity = 0.0;
                self.monarch_y = y;
            }
            if let Some((_, dir)) = self.harbor.boat_at(&self.level) {
                self.monarch.facing = dir;
            }
        }
    }

    /// A worker or knight (with its archers) steps aboard. orig: `Embark()`,
    /// `Knight.EmbarkArchers()`.
    fn embark(&mut self, world: &WorldData, id: u32) {
        let Some(u) = self.population.units.iter().find(|u| u.id == id) else {
            return;
        };
        let knight = u.role == crate::data::Role::Knight;
        if self.harbor.boat_mode(&self.level) != Some(crate::boat::mode::WAITING)
            || !self.harbor.room(world, &self.level, knight)
        {
            return;
        }
        let mut ids = alloc::vec![id];
        if knight {
            ids.extend(self.population.followers_of(id));
        }
        for id in ids {
            if let Some(u) = self.population.take(id) {
                self.harbor.embark(&mut self.level, u, &mut self.rng);
            }
        }
        self.payables_dirty = true;
    }

    /// orig: `Game.EndGame(Result.Won)` → `UnlockManager.QueueLand()`: what
    /// sails on is taken now.
    fn win(&mut self, world: &WorldData) {
        self.won = true;
        let boat = self.harbor.boat.as_ref();
        self.voyage = Some(crate::progress::Voyage {
            land: if self.level_index < crate::progress::LAND_FIVE {
                self.level_index + 1
            } else {
                0
            },
            reign: self.reign + 1,
            coat: self.coat,
            steed: self.steed,
            coins: self.wallet.coins(),
            hermit: self
                .passenger
                .and_then(|h| self.level.index_of(h))
                .map(|i| self.level.objects[i].proto),
            dog: self
                .dogs
                .dog
                .as_ref()
                .map(|d| d.color)
                .or(boat.and_then(|b| b.dog)),
            archers: boat.map_or(0, |b| b.archers),
            workers: boat.map_or(0, |b| b.workers),
            knights: boat.map_or(0, |b| b.knights),
        });
        let _ = world;
    }

    /// orig: `Beach.Pay()` → `Boat.SailAway()`: archers on the boat's guard
    /// slots go along, the dog jumps aboard, and the island is won.
    fn set_sail(&mut self, world: &WorldData) {
        let Some(boat) = self.harbor.boat.as_ref().map(|b| b.object) else {
            return;
        };
        if let Some(e) = self.harbor.sail_away(&mut self.level) {
            self.queued.push(GameEvent::Harbor(e));
        }
        let aboard: Vec<u32> = self
            .population
            .units
            .iter()
            .filter(|u| u.in_slot && u.slot.is_some_and(|s| s.0 == boat))
            .map(|u| u.id)
            .collect();
        for id in aboard {
            if let Some(mut u) = self.population.take(id) {
                u.slot = None;
                u.in_slot = false;
                self.harbor.embark(&mut self.level, u, &mut self.rng);
            }
        }
        if let (Some(d), Some(b)) = (self.dogs.dog.take(), self.harbor.boat.as_mut()) {
            b.dog = Some(d.color);
        }
        self.win(world);
        self.queued.push(GameEvent::Won);
    }

    /// orig: Squid.HandleOnDeath → Character.Release.
    fn apply_greed_effect(&mut self, effect: GreedEvent, events: &mut Vec<GameEvent>) {
        if let GreedEvent::Released { x, count } = effect {
            for _ in 0..count {
                self.population
                    .spawn(crate::data::Role::Peasant, x, &mut self.rng);
            }
            self.payables_dirty = true;
        }
        events.push(GameEvent::Greed(effect));
    }

    /// Troll targets, loot and the results of their attacks.
    fn update_greed(&mut self, world: &WorldData, dt: f32, events: &mut Vec<GameEvent>) {
        use crate::coins::{Rect, MONARCH_BOX};
        let mut targets: Vec<TargetInfo> = Vec::new();
        if self.has_crown {
            targets.push(TargetInfo {
                target: Target::Monarch,
                x: self.monarch.x,
                rect: MONARCH_BOX.at(self.monarch.x, 0.0, self.monarch.facing < 0),
                priority: false,
                wall: false,
            });
        }
        for u in &self.population.units {
            // orig: in a guard slot, archers can't be hurt by greed.
            if u.role == crate::data::Role::Beggar || u.stunned > 0.0 || u.in_slot || u.y > 0.5 {
                continue;
            }
            targets.push(TargetInfo {
                target: Target::Unit(u.id),
                x: u.x,
                rect: Rect::centered(0.32, 0.33, 0.64).at(u.x, 0.0, false),
                // orig: archers (and knights) register as priority targets.
                priority: matches!(
                    u.role,
                    crate::data::Role::Archer | crate::data::Role::Knight
                ),
                wall: false,
            });
        }
        for o in &self.level.objects {
            if world.protos[o.proto as usize].wall_def.is_some()
                && !build::under_construction(world, o)
            {
                targets.push(TargetInfo {
                    target: Target::Object(o.id),
                    x: o.x,
                    rect: Rect::centered(1.0, 1.0, 2.0).at(o.x, 0.0, false),
                    priority: false,
                    wall: true,
                });
            }
        }
        // orig: catapults register as priority targets.
        for c in &self.siege.catapults {
            if let Some(o) = self
                .level
                .index_of(c.object)
                .map(|i| &self.level.objects[i])
            {
                if !build::under_construction(world, o) {
                    targets.push(TargetInfo {
                        target: Target::Object(o.id),
                        x: o.x,
                        rect: Rect::centered(0.5, 1.2, 1.0).at(o.x, 0.0, false),
                        priority: true,
                        wall: false,
                    });
                }
            }
        }
        // orig: the mule's Damageable takes troll hits while it carries a load.
        for m in &self.merchants.list {
            if m.mule_packed() {
                targets.push(TargetInfo {
                    target: Target::Object(m.object),
                    x: m.mule_x,
                    rect: Rect::centered(0.4, 0.8, 0.8).at(m.mule_x, 0.0, false),
                    priority: false,
                    wall: false,
                });
            }
        }
        let mut loot: Vec<LootInfo> = Vec::new();
        for (i, c) in self.coins.list.iter().enumerate() {
            if c.is_free() && c.y < 0.05 {
                loot.push(LootInfo {
                    loot: Loot::Coin,
                    x: c.x,
                    y: c.y,
                    index: i,
                });
            }
        }
        let n_coins = self.coins.list.len();
        for (j, t) in self.population.tools.iter().enumerate() {
            loot.push(LootInfo {
                loot: Loot::Tool(t.kind),
                x: t.x,
                y: t.y,
                index: n_coins + j,
            });
        }
        let crown_index = n_coins + self.population.tools.len();
        if let Some(c) = &self.greed.crown {
            if c.t > 1.0 {
                loot.push(LootInfo {
                    loot: Loot::Crown,
                    x: c.x,
                    y: c.y,
                    index: crown_index,
                });
            }
        }
        let parts_index = crown_index + 1;
        if let Some(w) = &self.harbor.wharf {
            for (j, p) in w.stacked.iter().enumerate() {
                if p.y <= 0.0 {
                    loot.push(LootInfo {
                        loot: Loot::BoatPart(p.index),
                        x: p.x,
                        y: 0.0,
                        index: parts_index + j,
                    });
                }
            }
        }
        let portals = Greed::portals_in(world, &self.level);
        let portal_infos = self.portal_hit_infos(world);
        let mut threats = alloc::vec![self.monarch.x];
        threats.extend(
            self.population
                .units
                .iter()
                .filter(|u| {
                    matches!(
                        u.role,
                        crate::data::Role::Archer | crate::data::Role::Knight
                    )
                })
                .map(|u| u.x),
        );
        let ctx = GreedCtx {
            portal_infos: &portal_infos,
            threats: &threats,
            season: self.season,
            day: self.clock.day,
            boat: self.harbor.boat.is_some(),
            world,
            level_index: self.level_index,
            campfire: self.campfire,
            borders: self.borders,
            monarch_x: self.monarch.x,
            monarch_has_crown: self.has_crown,
            targets: &targets,
            loot: &loot,
            portals: &portals,
        };
        let mut out: Vec<GreedEvent> = Vec::new();
        let hours = self.clock.date();
        self.greed
            .update(dt, hours, &ctx, &self.level, &mut self.rng, |e| out.push(e));

        let mut coin_removals: Vec<usize> = Vec::new();
        let mut part_removals: Vec<usize> = Vec::new();
        let mut tool_removals: Vec<usize> = Vec::new();
        for e in out {
            match e {
                GreedEvent::Hit { by, target, damage } => {
                    let dir = self
                        .greed
                        .trolls
                        .iter()
                        .find(|t| t.id == by)
                        .map_or(1.0, |t| t.mover.facing as f32);
                    match target {
                        Target::Monarch => self.hurt_monarch(damage, dir, events),
                        Target::Unit(id) => {
                            let mut ue = Vec::new();
                            self.population.hurt(
                                id,
                                damage,
                                dir,
                                &mut self.coins,
                                &mut self.rng,
                                &mut |e| ue.push(e),
                            );
                            self.payables_dirty = true;
                            events.extend(ue.into_iter().map(GameEvent::Unit));
                        }
                        Target::Object(id)
                            if self.merchants.list.iter().any(|m| m.object == id) =>
                        {
                            self.merchants.mule_hit(id, &mut self.coins, &mut self.rng)
                        }
                        Target::Object(id)
                            if self.siege.catapults.iter().any(|c| c.object == id) =>
                        {
                            // orig: Catapult Damageable → HandleOnDeath → wreck.
                            if let Some(i) = self.level.index_of(id) {
                                self.level.objects[i].hp -= damage;
                                if self.level.objects[i].hp <= 0 {
                                    self.level.objects.remove(i);
                                    self.payables_dirty = true;
                                }
                            }
                        }
                        Target::Object(id) => self.hurt_wall(world, id, damage),
                    }
                }
                GreedEvent::TookLoot { index, .. } => {
                    if index < n_coins {
                        coin_removals.push(index);
                    } else if index < crown_index {
                        tool_removals.push(index - n_coins);
                    } else if index == crown_index {
                        self.greed.crown = None;
                    } else {
                        part_removals.push(index - parts_index);
                    }
                }
                GreedEvent::CrownTaken => {
                    self.game_over = true;
                    self.greed.retreat_all();
                    events.push(GameEvent::GameOver);
                }
                GreedEvent::Grabbed { by, unit } => {
                    if !self.population.snatch(unit, &mut self.coins, &mut self.rng) {
                        // Knights can't be carried off.
                        if let Some(t) = self.greed.trolls.iter_mut().find(|t| t.id == by) {
                            t.victims = t.victims.saturating_sub(1);
                        }
                    }
                    self.payables_dirty = true;
                }
                // orig: EnemyManager.OnEnemiesSpawned → the dog barks that way.
                GreedEvent::PortalOpened(object) => {
                    if let Some(i) = self.level.index_of(object) {
                        let x = self.level.objects[i].x;
                        self.dogs.enemies_spawned(x);
                    }
                }
                GreedEvent::PortalHit { object, damage } => {
                    self.hurt_portal(world, object, damage, events)
                }
                GreedEvent::PortalRemoved(object) => {
                    if let Some(i) = self.level.index_of(object) {
                        self.level.objects.remove(i);
                    }
                    self.payables_dirty = true;
                }
                GreedEvent::PortalRebuilt(object) => {
                    if let Some(i) = self.level.index_of(object) {
                        let o = &mut self.level.objects[i];
                        o.hp = world.protos[o.proto as usize]
                            .portal
                            .map_or(1, |p| p.hit_points);
                    }
                    self.payables_dirty = true;
                }
                GreedEvent::CliffsDestroyed if !self.game_over => {
                    self.win(world);
                    events.push(GameEvent::Won);
                }
                _ => {}
            }
            self.apply_greed_effect(e, events);
        }
        coin_removals.sort_unstable();
        for i in coin_removals.into_iter().rev() {
            self.coins.list.remove(i);
        }
        // orig: Wharf.HandleOnPartPickedUp: the part goes back to be paid for again.
        part_removals.sort_unstable();
        for i in part_removals.into_iter().rev() {
            self.harbor.part_taken(&mut self.level, i);
            self.payables_dirty = true;
        }
        tool_removals.sort_unstable();
        for i in tool_removals.into_iter().rev() {
            let t = self.population.tools.remove(i);
            self.population.tool_lost(&t);
            self.payables_dirty = true;
        }

        // orig: the monarch picks the crown back up by touching it.
        if let Some(c) = self.greed.crown {
            let box_ = crate::coins::MONARCH_BOX.at(self.monarch.x, 0.0, false);
            let crown = Rect::centered(0.0, 0.25, 0.16).at(c.x, c.y, false);
            if !self.has_crown && c.t > 1.0 && box_.overlaps(&crown) {
                self.greed.crown = None;
                self.has_crown = true;
                events.push(GameEvent::CrownRecovered);
            }
        }
    }

    /// Arrows in flight hitting animals. orig: `Arrow.HitObject()` on wildlife.
    fn arrows_hit_wildlife(&mut self, events: &mut Vec<GameEvent>) {
        let mut hits: Vec<(u32, f32)> = Vec::new();
        for a in &mut self.greed.arrows {
            if a.stuck > 0.0 || a.missed {
                continue;
            }
            let hit = self.wildlife.animals.iter().find(|w| {
                let (x0, y0, x1, y1) = w.hit_box();
                !w.is_dead() && a.x >= x0 && a.x <= x1 && a.y >= y0 && a.y <= y1
            });
            if let Some(w) = hit {
                hits.push((w.id, a.from_x));
                a.stuck = 10.0;
            }
        }
        for (id, from) in hits {
            let mut out = Vec::new();
            self.wildlife
                .hurt(id, 1, from, &mut self.coins, &mut self.rng, &mut |e| {
                    out.push(e)
                });
            events.extend(out.into_iter().map(GameEvent::Wild));
        }
    }

    /// Portal triggers in ground-relative world coordinates.
    fn portal_hit_infos(&self, world: &WorldData) -> Vec<PortalInfo> {
        self.level
            .objects
            .iter()
            .filter_map(|o| {
                let d = world.protos[o.proto as usize].portal?;
                let (x0, x1, y0, y1) = d.hit_box;
                let (a, b) = if o.flip {
                    (o.x - x1, o.x - x0)
                } else {
                    (o.x + x0, o.x + x1)
                };
                let y = o.y - build::WALK_Y;
                Some(PortalInfo {
                    object: o.id,
                    x: o.x,
                    hit_box: (a, b, y + y0, y + y1),
                    by_arrows: d.by_arrows,
                    defense_range: d.defense_range,
                    defense_delay: d.defense_delay,
                    defense_cooldown: d.defense_cooldown,
                    cliff: d.cliff,
                })
            })
            .collect()
    }

    /// Arrows and knights wear a portal down. orig: `Portal` `Damageable`
    /// → `HandleOnDeath()` → `Crumble()` (a regular portal leaves a rift).
    fn hurt_portal(
        &mut self,
        world: &WorldData,
        object: u32,
        damage: i32,
        events: &mut Vec<GameEvent>,
    ) {
        let Some(i) = self.level.index_of(object) else {
            return;
        };
        let Some(def) = world.protos[self.level.objects[i].proto as usize].portal else {
            return;
        };
        if !self.greed.portal_can_spawn(object) || self.level.objects[i].hp <= 0 {
            return;
        }
        self.level.objects[i].hp -= damage;
        if self.level.objects[i].hp > 0 {
            return;
        }
        let mut out = Vec::new();
        self.greed
            .portal_destroyed(object, def.cliff, def.rebuild_days, &mut |e| out.push(e));
        events.extend(out.into_iter().map(GameEvent::Greed));
        if let (false, Some(rift)) = (def.cliff, def.rift) {
            let o = &self.level.objects[i];
            let (layer, x, y, flip) = (o.layer, o.x, o.y, o.flip);
            let r = level::instantiate(
                world,
                rift,
                layer,
                x,
                y,
                world.protos[rift as usize].root_z,
                if flip { -1.0 } else { 1.0 },
            );
            self.level.add(r);
        }
        // orig: EnemyManager.PortalDestroyed → Kingdom.BecomeUnsafe.
        self.safe = false;
        let slots = self.slot_infos(world);
        self.staff_towers(&slots, true);
        self.payables_dirty = true;
    }

    /// orig: `Player.HandleOnReceiveDamage()`: coins fly out (half into the
    /// water); with none left, the crown falls off.
    fn hurt_monarch(&mut self, damage: i32, dir: f32, events: &mut Vec<GameEvent>) {
        if self.god {
            return;
        }
        // orig: Player.HandleOnReceiveDamage drops the passenger first.
        self.dump_passenger();
        if self.wallet.coins() > 0 {
            for _ in 0..damage {
                if self.wallet.coins() == 0 {
                    break;
                }
                self.wallet.set(self.wallet.coins() - 1);
                if self.rng.value() >= 0.5 {
                    let v = (dir * self.rng.range(0.5, 4.0), self.rng.range(3.0, 5.0));
                    self.coins.drop(
                        Who::Monarch,
                        (self.monarch.x, 0.79),
                        v,
                        crate::coins::Policy::Nobody,
                        None,
                        None,
                    );
                }
            }
        } else if self.has_crown {
            self.has_crown = false;
            let r = self.rng.range(3.0, 6.0);
            self.greed.drop_crown(self.monarch.x, dir * r, r);
            events.push(GameEvent::CrownLost);
        }
    }

    /// orig: `Wall.HandleOnReceiveDamage()` / `HandleOnDeath()` → wreck.
    fn hurt_wall(&mut self, world: &WorldData, id: u32, damage: i32) {
        let Some(i) = self.level.index_of(id) else {
            return;
        };
        self.level.objects[i].hp -= damage;
        // orig: Wall.HandleOnReceiveDamage: AddWork when the kingdom is safe.
        if self.safe {
            self.level.objects[i].repair = true;
        }
        if self.level.objects[i].hp <= 0 {
            let old = self.level.objects.remove(i);
            if let Some(wreck) = world.protos[old.proto as usize]
                .wall_def
                .and_then(|w| w.wreck)
            {
                let p = &world.protos[wreck as usize];
                let o = level::instantiate(
                    world,
                    wreck,
                    old.layer,
                    old.x,
                    old.y,
                    p.root_z,
                    if old.flip { -1.0 } else { 1.0 },
                );
                self.level.add(o);
            }
        }
        self.payables_dirty = true;
    }

    fn apply_pay_event(&mut self, world: &WorldData, e: PayEvent) -> Option<Upgraded> {
        match e {
            PayEvent::DropCoin => {
                self.coins
                    .drop_from(&self.monarch, &mut self.wallet, &mut self.rng)
            }
            PayEvent::CoinToPayable { id, slot } => {
                if let Some(i) = self.payable_index(id) {
                    let pos = self.payables[i].coin_slot(slot);
                    self.coins
                        .send_to_payable(&self.monarch, &mut self.wallet, id, pos);
                }
            }
            PayEvent::Complete { id, .. } => {
                self.coins.spend_floating();
                // orig: Payable.TransactionComplete → Pay(), then priceIncrease.
                let kind = self
                    .level
                    .index_of(id.0)
                    .and_then(|i| build::pay_kind(world, &self.level.objects[i]));
                let upgraded = match kind {
                    Some(PayKind::Upgrade) => {
                        let up = build::upgrade(world, &mut self.level, id.0, self.tech.passenger);
                        // orig: TeleporterRift.HandleOnUpgraded → link to its entrance.
                        if let Some(u) = up {
                            let cd = self.level.index_of(u.new).and_then(|i| {
                                world.protos[self.level.objects[i].proto as usize].teleporter
                            });
                            if let Some(cd) = cd {
                                self.teleports.rift_upgraded(u.old, u.new, cd);
                            }
                        }
                        // orig: Player.OnPassengerUsed: the hermit builds, then hops off.
                        if up.is_some_and(|u| u.used_passenger) {
                            if let Some(h) = self.passenger.take() {
                                self.hermits.dismount(
                                    &mut self.level,
                                    h,
                                    self.monarch.x,
                                    &mut self.rng,
                                );
                                self.hermits.build(h);
                                self.tech.passenger = None;
                            }
                        }
                        up
                    }
                    Some(PayKind::Shop) => {
                        self.population.stock_shop(world, &self.level, id.0);
                        None
                    }
                    Some(PayKind::MarkTree) => {
                        build::mark_tree(world, &mut self.level, id.0);
                        None
                    }
                    Some(PayKind::Shield) => {
                        self.population.stock_shield(world, &self.level, id.0);
                        None
                    }
                    Some(PayKind::Statue) => {
                        self.statues.pay(world, &self.level, id.0);
                        None
                    }
                    Some(PayKind::Steed) => {
                        self.ride(world, id.0);
                        None
                    }
                    Some(PayKind::Teleporter) => {
                        let cd = self
                            .level
                            .index_of(id.0)
                            .and_then(|i| {
                                world.protos[self.level.objects[i].proto as usize].teleporter
                            })
                            .unwrap_or(70.0);
                        let x = self
                            .level
                            .index_of(id.0)
                            .map_or(self.monarch.x, |i| self.level.objects[i].x);
                        // orig: PayableTeleporter.TeleportPlayer at the linked end.
                        if let Some(dest) = self.teleports.pay(id.0, x, cd) {
                            if let Some(i) = self.level.index_of(dest) {
                                self.monarch.x = self.level.objects[i].x;
                            }
                        }
                        None
                    }
                    Some(PayKind::Workshop) => {
                        self.siege.order(world, &mut self.level, id.0);
                        None
                    }
                    Some(PayKind::Merchant) => {
                        self.merchants.pay(id.0);
                        None
                    }
                    Some(PayKind::Cabin) => {
                        self.hermits
                            .open_cabin(world, &mut self.level, id.0, &mut self.rng);
                        None
                    }
                    // orig: Hermit.Pay → Player.PickUpPassenger (dropping any other).
                    Some(PayKind::Hermit) => {
                        self.dump_passenger();
                        self.hermits.mount(id.0);
                        self.passenger = Some(id.0);
                        self.tech.passenger = self.hermits.kind_of(id.0);
                        None
                    }
                    Some(PayKind::Wharf) => {
                        if let Some(e) =
                            self.harbor.pay_wharf(world, &mut self.level, &mut self.rng)
                        {
                            self.queued.push(GameEvent::Harbor(e));
                        }
                        None
                    }
                    Some(PayKind::Beach) => {
                        self.set_sail(world);
                        None
                    }
                    // orig: Signpost.Pay → QueueRandomUnlock; the map is taken.
                    Some(PayKind::Signpost) => {
                        let n = self.unlockable.len() as i32;
                        if n > 0 {
                            let pick = self.rng.range_i32(0, n) as usize;
                            self.queued_unlock = Some(self.unlockable[pick]);
                        }
                        if let Some(i) = self.level.index_of(id.0) {
                            let o = &mut self.level.objects[i];
                            if let Some(p) = world.protos[o.proto as usize].signpost {
                                if let Some(part) = o.parts.get_mut(p as usize) {
                                    part.visible = false;
                                }
                            }
                        }
                        None
                    }
                    // orig: PayableBorder.Pay: the knight in range with the most archers charges.
                    Some(PayKind::Border) => {
                        if let Some(i) = self.level.index_of(id.0) {
                            let o = &self.level.objects[i];
                            let range = world.protos[o.proto as usize]
                                .border
                                .map_or(8.0, |b| b.knight_range);
                            let best = self
                                .population
                                .knights_near(o.x, range)
                                .into_iter()
                                .max_by_key(|k| k.1);
                            if let Some((k, _)) = best {
                                self.population.order_charge(k);
                            }
                        }
                        None
                    }
                    None => None,
                };
                if let Some(i) = self.level.index_of(id.0) {
                    if let Some(def) = world.protos[self.level.objects[i].proto as usize].payable {
                        let o = &mut self.level.objects[i];
                        o.price = o.price.saturating_add(def.price_increase).clamp(1, 16);
                    }
                }
                self.payables_dirty = true;
                return upgraded;
            }
            PayEvent::ReleaseCoins { .. } => self.coins.release_floating(),
            PayEvent::Focus(_)
            | PayEvent::Unfocus(_)
            | PayEvent::ShowPrice(_)
            | PayEvent::Highlight(_) => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::data::{
        BuildingDef, DayDef, DirectorDef, Layer, LevelConfigDef, PayableDef, Proto, Role, ShopDef,
        ToolKind, UpgradeDef,
    };
    use alloc::string::ToString;
    use alloc::vec;

    /// No blocks, one day kind, no lighting: enough to run the clock.
    fn empty_world(coins: i32) -> WorldData {
        WorldData {
            days: vec![DayDef {
                name: "d".to_string(),
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
                name: "t".to_string(),
                autumn_start: (40, 40),
                winter_start: (60, 60),
                days_before_game_over: -1,
                starting_coins: coins,
                min_level_width: 0,
                group_counts: vec![],
                waves: Default::default(),
                wave_timed_multiplier: 1.0,
                wave_multiplier_timeframe: (0, -1),
                season_change_days: 3.0,
                income_multiplier: 1.0,
                free_boat_parts: 0,
                wave_boat_multiplier: 1.0,
                force_all_unlocks: false,
                defence_waves: [None; 3],
                retaliation_waves: [None; 3],
            }],
            ..Default::default()
        }
    }

    fn wall_proto(name: &str, price: u8, next: Option<u16>, build_points: i32) -> Proto {
        Proto {
            name: name.to_string(),
            tag: "Untagged".to_string(),
            root_z: 0.1,
            payable: Some(PayableDef {
                price,
                price_increase: 0,
                coin_offset: (0.0, 2.5),
                coin_spacing: 0.4,
                pay_point_offset: 0.0,
                pay_distance: 0.75,
            }),
            upgrade: Some(UpgradeDef {
                next,
                only_in_buildable_region: true,
                requires_stone: false,
                cooldown: 0.0,
            }),
            building: (build_points > 0).then(|| BuildingDef {
                build_points,
                auto_build_rate: 0.0,
                scaffold: vec![],
            }),
            ..Default::default()
        }
    }

    fn run(g: &mut GameState, w: &WorldData, seconds: f32, c: Controls) {
        let mut ev = Vec::new();
        for i in 0..(seconds * 60.0) as u32 {
            let mut c = c;
            c.drop_coin = c.drop_coin && i == 0;
            g.tick(w, &c, &mut ev);
        }
    }

    #[test]
    fn beggar_takes_a_coin_peasant_takes_a_hammer_worker_builds() {
        let mut w = empty_world(5);
        let mut shop = wall_proto("ShopHammer", 3, None, 0);
        shop.upgrade = None;
        shop.shop = Some(ShopDef {
            tool: ToolKind::Hammer,
            item_placement: (-1.05, 0.25 + build::WALK_Y),
            item_spacing: 0.2,
            max_items: 4,
        });
        w.protos = vec![
            wall_proto("Wall0", 1, Some(1), 0),
            wall_proto("Wall1", 3, None, 3),
            shop,
        ];
        w.layers = vec![crate::data::LayerDef { ratio: 1.0, z: 0.0 }; 7];
        let mut g = GameState::new(&w, 0, 4);
        g.monarch.x = 0.0;
        let wall = g
            .level
            .add(level::instantiate(&w, 1, Layer::Game, -3.0, 0.0, 0.1, 1.0));
        g.level.objects.last_mut().unwrap().build = Some(0.0);
        let shop = g
            .level
            .add(level::instantiate(&w, 2, Layer::Game, 3.0, 0.0, 0.1, 1.0));
        g.payables_dirty = true;
        let beggar = g.population.spawn(Role::Beggar, 1.5, &mut g.rng);

        // The monarch drops a coin; the beggar (2 units away) runs for it.
        run(
            &mut g,
            &w,
            6.0,
            Controls {
                drop_coin: true,
                ..Default::default()
            },
        );
        let u = g.population.units.iter().find(|u| u.id == beggar).unwrap();
        assert_eq!(u.role, Role::Peasant);

        // Buy a hammer; the peasant walks to the rack and becomes a worker.
        g.population.stock_shop(&w, &g.level, shop);
        run(&mut g, &w, 15.0, Controls::default());
        let u = g.population.units.iter().find(|u| u.id == beggar).unwrap();
        assert_eq!(u.role, Role::Worker);

        // The worker finds the scaffold and builds it (3 points × 0.7 s, plus walking).
        run(&mut g, &w, 20.0, Controls::default());
        let o = &g.level.objects[g.level.index_of(wall).unwrap()];
        let u = g.population.units.iter().find(|u| u.id == beggar).unwrap();
        assert!(
            !build::under_construction(&w, o),
            "build points {:?}, worker at {} ai {:?}, wall at {}",
            o.build,
            u.x,
            u.ai,
            o.x
        );
    }

    /// At night a worker doesn't take a job outside the walls: sent out and
    /// called back every tick, they'd shuffle on the spot.
    #[test]
    fn workers_leave_jobs_outside_the_walls_until_morning() {
        let mut w = empty_world(0);
        w.protos = vec![wall_proto("Wall1", 3, None, 3)];
        w.layers = vec![crate::data::LayerDef { ratio: 1.0, z: 0.0 }; 7];
        let mut g = GameState::new(&w, 0, 5);
        g.clock.time = 23.0;
        g.days = crate::time::DayCounter::new(&g.clock);
        g.monarch.x = 0.0;
        let x = g.campfire + crate::kingdom::MIN_KINGDOM_EXTENTS + 4.0;
        let wall = g
            .level
            .add(level::instantiate(&w, 0, Layer::Game, x, 0.0, 0.1, 1.0));
        g.level.objects.last_mut().unwrap().build = Some(0.0);
        g.payables_dirty = true;
        let id = g.population.spawn(Role::Worker, g.campfire, &mut g.rng);
        let mut turns = 0;
        let mut facing = 0;
        for _ in 0..(10.0 * 60.0) as u32 {
            g.tick(&w, &Controls::default(), &mut Vec::new());
            let u = g.population.units.iter().find(|u| u.id == id).unwrap();
            if u.mover.facing != facing {
                turns += 1;
                facing = u.mover.facing;
            }
        }
        assert!(!g.days.is_daytime);
        assert!(turns < 6, "the worker turned {turns} times in 10 s");
        let o = &g.level.objects[g.level.index_of(wall).unwrap()];
        assert_eq!(o.build, Some(0.0));
    }

    fn fight_world() -> WorldData {
        let mut w = empty_world(0);
        let mut wall = wall_proto("Wall1", 3, None, 0);
        wall.upgrade = None;
        wall.payable = None;
        wall.wall = true;
        wall.wall_def = Some(crate::data::WallDef {
            hit_points: 3,
            repair_rate: 1,
            repair_rate_above_max: 1,
            bonus_hit_points_multiplier: 1.8,
            minimum_intact: 0.7,
            wreck: None,
        });
        w.protos = vec![wall];
        w.layers = vec![crate::data::LayerDef { ratio: 1.0, z: 0.0 }; 7];
        w
    }

    fn run_events(g: &mut GameState, w: &WorldData, seconds: f32) -> Vec<GameEvent> {
        let mut ev = Vec::new();
        for _ in 0..(seconds * 60.0) as u32 {
            g.tick(w, &Controls::default(), &mut ev);
        }
        ev
    }

    #[test]
    fn trolls_break_down_a_wall() {
        let w = fight_world();
        let mut g = GameState::new(&w, 0, 2);
        g.monarch.x = 0.0;
        let wall = g
            .level
            .add(level::instantiate(&w, 0, Layer::Game, 5.0, 0.0, 0.1, 1.0));
        g.payables_dirty = true;
        for _ in 0..3 {
            g.greed
                .spawn_troll(crate::greed::EnemyKind::Troll, 12.0, 1, false, &mut g.rng);
        }
        run_events(&mut g, &w, 20.0);
        assert!(g.level.index_of(wall).is_none(), "wall still standing");
    }

    #[test]
    fn greed_cannot_walk_through_a_wall() {
        let mut w = fight_world();
        if let Some(d) = &mut w.protos[0].wall_def {
            d.hit_points = 10_000;
        }
        let mut g = GameState::new(&w, 0, 3);
        g.monarch.x = 0.0;
        g.level
            .add(level::instantiate(&w, 0, Layer::Game, 5.0, 0.0, 0.1, 1.0));
        g.payables_dirty = true;
        for _ in 0..3 {
            g.greed
                .spawn_troll(crate::greed::EnemyKind::Troll, 12.0, 1, false, &mut g.rng);
        }
        for _ in 0..(20.0 * 60.0) as u32 {
            g.tick(&w, &Controls::default(), &mut Vec::new());
            for t in &g.greed.trolls {
                assert!(t.x > 5.0, "a troll got through to x {}", t.x);
            }
        }
    }

    /// Archers defend from behind a wall; in the open a charging troll
    /// disarms them. Far from the campfire, the archer guards the right wall.
    /// Arrows scatter (`maxError`, water), so judge over many seeds.
    #[test]
    fn archers_shoot_trolls() {
        let w = fight_world();
        let mut kills = 0;
        for seed in 0..40 {
            let mut g = GameState::new(&w, 0, seed);
            g.monarch.x = -20.0;
            g.level
                .add(level::instantiate(&w, 0, Layer::Game, 14.0, 0.0, 0.1, 1.0));
            g.payables_dirty = true;
            g.population.spawn(Role::Archer, 12.0, &mut g.rng);
            g.greed
                .spawn_troll(crate::greed::EnemyKind::Troll, 22.0, 1, false, &mut g.rng);
            let ev = run_events(&mut g, &w, 15.0);
            if ev
                .iter()
                .any(|e| matches!(e, GameEvent::Greed(GreedEvent::Killed(_))))
            {
                kills += 1;
            }
        }
        assert!(kills >= 24, "only {kills}/40 trolls shot");
    }

    #[test]
    fn a_peasant_takes_a_shield_and_the_knight_cuts_down_greed() {
        let mut w = fight_world();
        let mut shop = wall_proto("ShopShield", 6, None, 0);
        shop.upgrade = None;
        shop.shield_shop = Some(crate::data::ShieldShopDef {
            shield_pos: (0.0, 1.72),
            banner_part: None,
            banner: None,
            torn: None,
        });
        w.protos.push(shop);
        let mut g = GameState::new(&w, 0, 4);
        g.monarch.x = -20.0;
        let shop = g
            .level
            .add(level::instantiate(&w, 1, Layer::Game, 2.0, 0.0, 0.1, 1.0));
        g.payables_dirty = true;
        run_events(&mut g, &w, 0.1);
        assert!(g.population.shield_can_pay(shop));
        g.population.stock_shield(&w, &g.level, shop);
        assert!(!g.population.shield_can_pay(shop));
        let p = g.population.spawn(Role::Peasant, 0.0, &mut g.rng);
        g.population
            .units
            .iter_mut()
            .find(|u| u.id == p)
            .unwrap()
            .coins = 5;
        run_events(&mut g, &w, 20.0);
        let k = g.population.units.iter().find(|u| u.id == p).unwrap();
        assert_eq!(k.role, Role::Knight);
        assert_eq!(
            g.population.shields[0].status,
            crate::units::ShieldStatus::Taken
        );
        // A tough troll walks into the knight's sword: 5 hit points, 6 damage.
        let kx = k.x;
        g.greed.spawn_troll(
            crate::greed::EnemyKind::ToughTroll,
            kx + 6.0,
            1,
            false,
            &mut g.rng,
        );
        let ev = run_events(&mut g, &w, 10.0);
        assert!(
            ev.iter()
                .any(|e| matches!(e, GameEvent::Greed(GreedEvent::Killed(_)))),
            "{:?}",
            g.population.units[0]
        );
    }

    #[test]
    fn archers_bring_down_a_portal_and_the_greed_retaliates() {
        let mut w = fight_world();
        let rift = w.protos.len() as u16 + 1;
        w.protos.push(Proto {
            name: "Portal".to_string(),
            portal: Some(crate::data::PortalDef {
                cliff: false,
                hit_points: 4,
                by_arrows: true,
                by_knights: true,
                hit_box: (-1.0, 1.0, 0.0, 2.0),
                defense_range: 0.0,
                defense_delay: (2.0, 8.0),
                defense_cooldown: 8.0,
                rebuild_days: 0.0,
                rift: Some(rift),
            }),
            ..Default::default()
        });
        w.protos.push(Proto {
            name: "TeleporterRift".to_string(),
            ..Default::default()
        });
        w.waves.push(crate::data::WaveDef {
            name: "Retaliation".to_string(),
            trolls: crate::data::Curve {
                keys: vec![[0.0, 2.0, 0.0, 0.0]],
            },
            ..Default::default()
        });
        w.levels[0].retaliation_waves = [Some(0); 3];
        let mut g = GameState::new(&w, 0, 7);
        g.monarch.x = 0.0;
        let portal = g
            .level
            .add(level::instantiate(&w, 1, Layer::Game, 20.0, 0.0, 0.1, 1.0));
        // Another portal on the left for the retaliation to come from.
        g.level
            .add(level::instantiate(&w, 1, Layer::Game, -40.0, 0.0, 0.1, 1.0));
        g.payables_dirty = true;
        g.population.spawn(Role::Archer, 16.0, &mut g.rng);
        let ev = run_events(&mut g, &w, 40.0);
        assert!(
            ev.iter().any(|e| matches!(
                e,
                GameEvent::Greed(GreedEvent::PortalDestroyed { object, .. }) if *object == portal
            )),
            "portal hp {:?}",
            g.level.index_of(portal).map(|i| g.level.objects[i].hp)
        );
        assert!(g
            .level
            .objects
            .iter()
            .any(|o| w.protos[o.proto as usize].name == "TeleporterRift"));
        assert!(
            ev.iter()
                .any(|e| matches!(e, GameEvent::Greed(GreedEvent::Retaliation))),
            "no retaliation"
        );
        // The rubble goes after 10 s.
        assert!(g.level.index_of(portal).is_none());
    }

    #[test]
    fn builders_repair_damaged_walls_by_day() {
        let w = fight_world();
        let mut g = GameState::new(&w, 0, 9);
        g.monarch.x = -20.0;
        let wall = g
            .level
            .add(level::instantiate(&w, 0, Layer::Game, 5.0, 0.0, 0.1, 1.0));
        g.payables_dirty = true;
        run_events(&mut g, &w, 0.1);
        g.hurt_wall(&w, wall, 2);
        assert_eq!(g.level.objects[g.level.index_of(wall).unwrap()].hp, 1);
        g.population.spawn(Role::Worker, 0.0, &mut g.rng);
        let ev = run_events(&mut g, &w, 30.0);
        assert!(ev
            .iter()
            .any(|e| matches!(e, GameEvent::Unit(UnitEvent::Repaired(_)))));
        assert!(ev.iter().any(|e| matches!(
            e,
            GameEvent::Unit(UnitEvent::WorkSound { x, tree: false }) if (*x - 5.0).abs() < 2.0
        )));
        let o = &g.level.objects[g.level.index_of(wall).unwrap()];
        assert_eq!(o.hp, 3);
        assert!(!o.repair);
    }

    #[test]
    fn a_statue_charges_and_blesses_then_wears_off() {
        let mut w = fight_world();
        let mut statue = wall_proto("Statue Farmer", 4, None, 0);
        statue.upgrade = None;
        statue.statue = Some(crate::data::StatueDef {
            deity: crate::data::Deity::Farmer,
            charge_days: 1.0,
            charge_multiplier: 1.0,
            max_charge: 3.0,
            overgrown: None,
            clean: None,
            charge_parts: vec![],
        });
        w.protos.push(statue);
        let mut g = GameState::new(&w, 0, 1);
        let id = g
            .level
            .add(level::instantiate(&w, 1, Layer::Game, 3.0, 0.0, 0.1, 1.0));
        run_events(&mut g, &w, 0.1);
        assert!(g.statues.can_pay(&w, &g.level, id));
        g.statues.pay(&w, &g.level, id);
        g.statues.pay(&w, &g.level, id);
        g.statues.pay(&w, &g.level, id);
        // Full at 3 charges.
        assert!(!g.statues.can_pay(&w, &g.level, id));
        run_events(&mut g, &w, 1.0);
        assert!(g.farmer_statue);
        // One charge lasts a day (240 s).
        run_events(&mut g, &w, 3.0 * 240.0);
        assert!(!g.farmer_statue);
    }

    #[test]
    fn a_hermit_turns_a_tower_into_a_ballista_that_shoots_greed() {
        use crate::data::{BallistaDef, HermitKind};
        let mut w = fight_world();
        // 1 Ballista tower, 2 Hermit, 3 Cabin, 4 Tower4.
        let mut tower = wall_proto("Tower Ballista", 1, None, 0);
        tower.payable = None;
        tower.upgrade = None;
        tower.ballista = Some(BallistaDef {
            bolt_pos: (0.4, 2.7),
            range: 10.0,
            min_range: 1.2,
            angle: (-30.0, 30.0),
            reload_work: 7,
            damage: 12,
            max_hits: 8,
            body_part: None,
            frames: vec![],
            work_pos: (-0.5, 2.0625),
        });
        w.protos.push(tower);
        let mut hermit = wall_proto("Hermit Ballista", 1, None, 0);
        hermit.upgrade = None;
        hermit.hermit = Some(HermitKind::Ballista);
        w.protos.push(hermit);
        let mut cabin = wall_proto("Hermit House Ballista", 4, None, 0);
        cabin.upgrade = None;
        cabin.cabin = Some(2);
        w.protos.push(cabin);
        let mut t4 = wall_proto("Tower4", 9, None, 0);
        t4.passenger_upgrades = vec![(HermitKind::Ballista, 1)];
        w.protos.push(t4);
        let mut g = GameState::new(&w, 0, 3);
        g.monarch.x = -2.0;
        let cabin = g
            .level
            .add(level::instantiate(&w, 3, Layer::Game, -2.0, 0.0, 0.1, 1.0));
        let t4 = g
            .level
            .add(level::instantiate(&w, 4, Layer::Game, 3.0, 0.0, 0.1, 1.0));
        g.payables_dirty = true;
        run_events(&mut g, &w, 0.1);
        // Tower4 has no next stage on its own.
        assert!(!g.payables.iter().any(|p| p.id.0 == t4 && p.can_pay));
        g.apply_pay_event(
            &w,
            PayEvent::Complete {
                id: PayableId(cabin),
                coins: 4,
            },
        );
        let hermit = g.hermits.list[0].object;
        run_events(&mut g, &w, 1.0);
        g.apply_pay_event(
            &w,
            PayEvent::Complete {
                id: PayableId(hermit),
                coins: 1,
            },
        );
        assert_eq!(g.passenger, Some(hermit));
        run_events(&mut g, &w, 0.1);
        // With the hermit aboard, Tower4 can become the ballista tower.
        assert!(g.payables.iter().any(|p| p.id.0 == t4 && p.can_pay));
        g.apply_pay_event(
            &w,
            PayEvent::Complete {
                id: PayableId(t4),
                coins: 9,
            },
        );
        assert_eq!(g.passenger, None);
        let tower = g
            .level
            .objects
            .iter()
            .find(|o| w.protos[o.proto as usize].ballista.is_some())
            .map(|o| o.id)
            .expect("ballista tower");
        // A builder winds it; then it shoots a troll walking in from the right.
        g.population.spawn(Role::Worker, 0.0, &mut g.rng);
        run_events(&mut g, &w, 12.0);
        let o = &g.level.objects[g.level.index_of(tower).unwrap()];
        assert!(o.reload >= 7, "reload {}", o.reload);
        g.greed.spawn_troll(
            crate::greed::EnemyKind::ToughTroll,
            12.0,
            1,
            false,
            &mut g.rng,
        );
        let ev = run_events(&mut g, &w, 6.0);
        assert!(
            ev.iter()
                .any(|e| matches!(e, GameEvent::Greed(GreedEvent::Killed(_)))),
            "bolts {:?}",
            g.greed.bolts
        );
    }

    #[test]
    fn a_chest_spills_its_coins_for_the_monarch() {
        let mut w = fight_world();
        w.protos.push(Proto {
            name: "Chest".to_string(),
            chest: Some((12, (-3.0, 3.0))),
            ..Default::default()
        });
        let mut g = GameState::new(&w, 0, 2);
        g.wallet.set(0);
        g.monarch.x = 5.0;
        let chest = g
            .level
            .add(level::instantiate(&w, 1, Layer::Game, 5.0, 0.0, 0.1, 1.0));
        run_events(&mut g, &w, 15.0);
        // Twelve spill around it, for the monarch only.
        let on_ground = g
            .coins
            .list
            .iter()
            .filter(|c| c.claimer == Some(Who::Monarch))
            .count() as u32;
        assert_eq!(g.wallet.coins() + on_ground, 12);
        assert!(g.level.index_of(chest).is_none(), "fades 10 s after");
    }

    #[test]
    fn builders_push_and_crank_a_catapult_that_crushes_greed() {
        let mut w = fight_world();
        let mut cat = wall_proto("Catapult", 1, None, 0);
        cat.payable = None;
        cat.upgrade = None;
        cat.catapult = Some(crate::data::CatapultDef {
            crank_rate: 0.1,
            force: (7.0, 9.0),
            scan: (2.0, 7.5),
            hit_points: 4,
            boulder_damage: 15,
            stuck_probability: 0.5,
            spoon: (0.19, 1.03),
        });
        w.protos.push(cat);
        let mut counter = wall_proto("Workshop Right", 6, None, 0);
        counter.upgrade = None;
        counter.workshop_side = Some((1, 1));
        w.protos.push(counter);
        let mut g = GameState::new(&w, 0, 5);
        g.monarch.x = -10.0;
        let c = g
            .level
            .add(level::instantiate(&w, 2, Layer::Game, 0.0, 0.0, 0.1, 1.0));
        run_events(&mut g, &w, 0.1);
        assert!(g.siege.counter_can_pay(c));
        g.apply_pay_event(
            &w,
            PayEvent::Complete {
                id: PayableId(c),
                coins: 6,
            },
        );
        assert!(!g.siege.counter_can_pay(c));
        g.population.spawn(Role::Worker, 0.0, &mut g.rng);
        g.population.spawn(Role::Worker, 0.5, &mut g.rng);
        run_events(&mut g, &w, 25.0);
        let cat = g.siege.catapults[0].object;
        let o = &g.level.objects[g.level.index_of(cat).unwrap()];
        assert_eq!(o.mode, crate::siege::mode::READY, "aim {}", o.aim);
        let mut recovery = g.clone();
        let target_x = o.x + 5.0;
        g.greed.spawn_troll(
            crate::greed::EnemyKind::ToughTroll,
            9.0,
            1,
            false,
            &mut g.rng,
        );
        let ev = run_events(&mut g, &w, 6.0);
        assert!(ev
            .iter()
            .any(|e| matches!(e, GameEvent::Greed(GreedEvent::Killed(_)))));

        // Use the projectile fired by the siege system. Stage its position to
        // isolate collision and release from the port's current aiming model.
        recovery.greed.spawn_troll(
            crate::greed::EnemyKind::Squid,
            target_x,
            1,
            false,
            &mut recovery.rng,
        );
        recovery.greed.trolls[0].hp = 1;
        recovery.greed.trolls[0].victims = 2;
        let mut events = Vec::new();
        // Let the real projectile's ghost period finish before the impact.
        for _ in 0..42 {
            recovery.update_siege(&w, TICK, &mut events);
        }
        let boulder = recovery.siege.boulders.first_mut().expect("catapult fired");
        boulder.x = target_x;
        boulder.y = 0.3;
        boulder.vx = 0.0;
        boulder.vy = 0.0;
        recovery.update_siege(&w, TICK, &mut events);
        assert_captives_released(&recovery, &events, 2);
        recovery.update_siege(&w, TICK, &mut events);
        assert_captives_released(&recovery, &events, 2);
    }

    #[test]
    fn workers_build_and_push_the_boat_and_the_monarch_sails_on_with_them() {
        let mut w = fight_world();
        let base = w.protos.len() as u16;
        let mut boat = wall_proto("Boat", 1, None, 0);
        boat.payable = None;
        boat.upgrade = None;
        boat.boat = Some(crate::data::BoatDef {
            water_height: -0.7,
            push_speed: 0.5,
            forest_speed_multiplier: 0.5,
            sail_speed: 0.7,
            max_workers: 3,
            max_knights: 3,
            push_positions: vec![-0.3, -2.4],
            sail: None,
        });
        w.protos.push(boat);
        let mut wharf = wall_proto("Wharf", 2, None, 0);
        wharf.upgrade = None;
        wharf.wharf = Some(crate::data::WharfDef {
            work_per_part: 2,
            parts: vec![(0, 0.0, 1.0), (1, 0.5, 1.0)],
            skeleton: None,
            skeleton_at: (0.1, 0.2),
            boat: Some(base),
        });
        w.protos.push(wharf);
        let mut beach = wall_proto("Beach", 3, None, 0);
        beach.upgrade = None;
        beach.beach = true;
        w.protos.push(beach);
        let mut g = GameState::new(&w, 0, 5);
        g.monarch.x = 2.0;
        g.wallet.set(7);
        let wharf = g.level.add(level::instantiate(
            &w,
            base + 1,
            Layer::Game,
            0.0,
            build::WALK_Y,
            0.1,
            1.0,
        ));
        g.level.add(level::instantiate(
            &w,
            base + 2,
            Layer::Game,
            -8.0,
            0.0,
            0.1,
            1.0,
        ));
        g.harbor = crate::boat::Harbor::new(&w, &mut g.level, 0, &mut g.rng);
        let pay = |g: &mut GameState, id: u32| {
            g.apply_pay_event(
                &w,
                PayEvent::Complete {
                    id: PayableId(id),
                    coins: 2,
                },
            )
        };
        // Two parts paid for; the workers fit them (two strokes each).
        pay(&mut g, wharf);
        pay(&mut g, wharf);
        assert_eq!(g.harbor.wharf.as_ref().unwrap().stacked.len(), 2);
        assert!(!g.harbor.wharf_can_pay());
        g.population.spawn(Role::Worker, 1.0, &mut g.rng);
        g.population.spawn(Role::Worker, -1.0, &mut g.rng);
        run_events(&mut g, &w, 12.0);
        assert!(g.harbor.wharf.as_ref().unwrap().stacked.is_empty());
        // Launched, pushed to the beach, boarded.
        assert!(g.harbor.wharf_can_pay());
        pay(&mut g, wharf);
        assert_eq!(
            g.harbor.boat_mode(&g.level),
            Some(crate::boat::mode::PUSHING)
        );
        let ev = run_events(&mut g, &w, 40.0);
        assert!(ev
            .iter()
            .any(|e| matches!(e, GameEvent::Harbor(crate::boat::HarborEvent::Arrived))));
        assert_eq!(
            g.harbor.boat_mode(&g.level),
            Some(crate::boat::mode::WAITING)
        );
        assert_eq!(g.harbor.boat.as_ref().unwrap().workers, 2);
        assert_eq!(g.population.count(Role::Worker), 0);
        assert!(g.harbor.beach_can_pay(&g.level));
        // A knight sent from the border runs for the boat and boards.
        let k = g.population.spawn_knight(-2.0, &mut g.rng);
        if let Some(u) = g.population.units.iter_mut().find(|u| u.id == k) {
            u.knight.as_mut().unwrap().side = -1;
        }
        g.population.order_charge(k);
        run_events(&mut g, &w, 8.0);
        assert_eq!(g.harbor.boat.as_ref().unwrap().knights, 1);
        // The monarch pays the beach: the island is won and the boat sails.
        let beach = g.harbor.beach.unwrap().0;
        pay(&mut g, beach);
        let ev = run_events(&mut g, &w, 5.0);
        assert!(ev.iter().any(|e| matches!(e, GameEvent::Won)));
        let (bx, _) = g.harbor.boat_at(&g.level).unwrap();
        assert!(bx < -8.5, "boat at {bx}");
        assert!((g.monarch.x - (bx - crate::boat::MONARCH_DECK)).abs() < 0.01);
        let v = g.voyage.clone().unwrap();
        assert_eq!((v.workers, v.archers, v.knights), (2, 0, 1));
        assert_eq!(v.coins, 7);
        // The next island starts with them.
        let next = GameState::start(&w, 0, 9, &crate::progress::Progress::everything(), Some(&v));
        assert_eq!(next.population.count(Role::Worker), 2);
        assert_eq!(next.population.count(Role::Knight), 1);
        assert_eq!(next.wallet.coins(), 7);
        assert_eq!(next.coat, v.coat);
        assert!(next
            .population
            .units
            .iter()
            .all(|u| (u.x - next.monarch.x).abs() <= 2.0));
    }

    #[test]
    fn the_signpost_map_unlocks_something_from_this_island() {
        use crate::progress::{Ending, Progress};
        let mut w = empty_world(0);
        w.unlockable_on_land = vec![0, 1, 0];
        let mut sign = wall_proto("Signpost", 2, None, 0);
        sign.upgrade = None;
        sign.signpost = Some(0);
        w.protos = vec![sign];
        w.layers = vec![crate::data::LayerDef { ratio: 1.0, z: 0.0 }; 7];
        let mut progress = Progress::default();
        let mut g = GameState::start(&w, 0, 3, &progress, None);
        let s = g
            .level
            .add(level::instantiate(&w, 0, Layer::Game, 1.0, 0.0, 0.1, 1.0));
        g.refresh_payables(&w);
        assert!(g.payables.iter().any(|p| p.id.0 == s && p.can_pay));
        g.apply_pay_event(
            &w,
            PayEvent::Complete {
                id: PayableId(s),
                coins: 2,
            },
        );
        let u = g.queued_unlock.unwrap();
        assert!(u == 0 || u == 2);
        g.refresh_payables(&w);
        assert!(g.payables.iter().any(|p| p.id.0 == s && !p.can_pay));
        // Lost or won, the unlock is kept, and forced into the next islands.
        let out = progress.finish(Ending {
            land: 0,
            won: true,
            unlock: g.queued_unlock,
        });
        assert_eq!(out.unlock, Some(u));
        assert_eq!(out.next, Some(1));
        assert_eq!(progress.land, 1);
        assert_eq!(progress.force_recent, 3);
        assert_eq!(progress.unlockable_on(&w, 0).len(), 1);
        let (ids, force) = progress.block_unlocks(&w, 1).unwrap();
        assert_eq!((ids, force), (vec![u as i32], Some(u as i32)));
    }

    /// A loaded save carries on exactly as the original would have.
    #[cfg(feature = "save")]
    #[test]
    fn a_saved_run_resumes_exactly() {
        let mut w = fight_world();
        w.steeds.push(Default::default());
        let mut shop = wall_proto("ShopHammer", 3, None, 0);
        shop.upgrade = None;
        shop.shop = Some(ShopDef {
            tool: ToolKind::Hammer,
            max_items: 2,
            item_placement: (-1.05, 0.25 + build::WALK_Y),
            item_spacing: 0.3,
        });
        w.protos.push(shop);
        let mut g = GameState::new(&w, 0, 7);
        g.level
            .add(level::instantiate(&w, 1, Layer::Game, 3.0, 0.0, 0.1, 1.0));
        for x in [-2.0, 1.0, 4.0] {
            g.population.spawn(Role::Peasant, x, &mut g.rng);
        }
        g.greed
            .spawn_troll(crate::greed::EnemyKind::Troll, 12.0, 1, false, &mut g.rng);
        g.wallet.set(9);
        let walk = Controls {
            direction: 1,
            ..Default::default()
        };
        run(&mut g, &w, 7.0, walk);
        let bytes = crate::save::save_game(&g, 42);
        assert_eq!(
            crate::save::load_game(&bytes, 41, &w).err(),
            Some(crate::save::LoadError::World)
        );
        let mut h = crate::save::load_game(&bytes, 42, &w).unwrap();
        for _ in 0..3 {
            run(&mut g, &w, 5.0, walk);
            run(&mut h, &w, 5.0, walk);
            assert_eq!(g.monarch.x, h.monarch.x);
            assert_eq!(g.wallet.coins(), h.wallet.coins());
            assert_eq!(g.rng.next_u32(), h.rng.next_u32());
            let xs = |s: &GameState| s.population.units.iter().map(|u| u.x).collect::<Vec<_>>();
            assert_eq!(xs(&g), xs(&h));
            let ts = |s: &GameState| s.greed.trolls.iter().map(|t| t.x).collect::<Vec<_>>();
            assert_eq!(ts(&g), ts(&h));
        }
        let p = crate::progress::Progress::everything();
        let pb = crate::save::save_progress(&p);
        assert_eq!(crate::save::load_progress(&pb).unwrap(), p);
        assert_eq!(
            crate::save::load_progress(&pb[..pb.len() - 3]).err(),
            Some(crate::save::LoadError::Corrupt)
        );
    }

    #[test]
    fn a_teleporter_flies_an_exit_and_links_the_rift_built_there() {
        let mut w = fight_world();
        let mut tele = wall_proto("Teleporter", 1, None, 0);
        tele.upgrade = None;
        tele.teleporter = Some(70.0);
        w.protos.push(tele); // 1
        let mut rift = wall_proto("TeleporterRift", 5, Some(1), 0);
        rift.rift = true;
        w.protos.push(rift); // 2
        w.rift_proto = Some(2);
        let mut g = GameState::new(&w, 0, 1);
        g.monarch.x = 0.0;
        let a = g
            .level
            .add(level::instantiate(&w, 1, Layer::Game, 0.0, 0.0, 0.1, 1.0));
        g.started(&w, a);
        g.apply_pay_event(
            &w,
            PayEvent::Complete {
                id: PayableId(a),
                coins: 1,
            },
        );
        assert!(g.teleports.exit.is_some());
        let right = Controls {
            direction: 1,
            ..Default::default()
        };
        let mut ev = Vec::new();
        for _ in 0..60 {
            g.tick(&w, &right, &mut ev);
        }
        let go = Controls {
            pay_down: true,
            ..Default::default()
        };
        g.tick(&w, &go, &mut ev);
        assert!(g.monarch.x > 5.0, "monarch at {}", g.monarch.x);
        let r = g
            .level
            .objects
            .iter()
            .find(|o| o.proto == 2)
            .map(|o| o.id)
            .expect("rift");
        // Build it up: the new teleporter is linked back to the first.
        g.apply_pay_event(
            &w,
            PayEvent::Complete {
                id: PayableId(r),
                coins: 5,
            },
        );
        let b = g
            .level
            .objects
            .iter()
            .find(|o| o.proto == 1 && o.id != a)
            .unwrap()
            .id;
        assert_eq!(
            g.teleports
                .list
                .iter()
                .find(|t| t.object == a)
                .unwrap()
                .linked,
            Some(b)
        );
    }

    /// Hit a carrier through the production projectile collision and dispatch.
    fn shoot_carrier(g: &mut GameState, w: &WorldData, i: usize, bolt: bool) -> Vec<GameEvent> {
        let b = g.greed.trolls[i].box_at();
        let (x, y) = (b.x + b.w * 0.5, b.y + b.h * 0.5);
        if bolt {
            g.greed.fire_bolt(x, y, 0.0, 0.0, 30, 1);
        } else {
            g.greed.shoot(x, y, 0.0, 0.0, false, false);
        }
        let mut events = Vec::new();
        g.update_greed(w, TICK, &mut events);
        events
    }

    fn assert_captives_released(game: &GameState, events: &[GameEvent], count: u8) {
        assert_eq!(game.population.count(Role::Peasant), count as usize);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, GameEvent::Greed(
                GreedEvent::Released { count: n, .. }
            ) if *n == count))
                .count(),
            1
        );
    }

    #[test]
    fn a_squid_carries_off_a_peasant_and_releases_it_after_an_arrow_or_bolt() {
        for bolt in [false, true] {
            let w = fight_world();
            let mut g = GameState::new(&w, 0, 4);
            g.monarch.x = -20.0;
            let p = g.population.spawn(Role::Peasant, 4.0, &mut g.rng);
            g.greed
                .spawn_troll(crate::greed::EnemyKind::Squid, 6.0, 1, false, &mut g.rng);
            let ev = run_events(&mut g, &w, 10.0);
            assert!(ev.iter().any(
                |e| matches!(e, GameEvent::Greed(GreedEvent::Grabbed { unit, .. }) if *unit == p)
            ));
            assert_eq!(g.population.count(Role::Peasant), 0);
            g.greed.trolls[0].hp = 1;
            let events = shoot_carrier(&mut g, &w, 0, bolt);
            assert_captives_released(&g, &events, 1);
            assert!(shoot_carrier(&mut g, &w, 0, bolt)
                .iter()
                .all(|e| !matches!(e, GameEvent::Greed(GreedEvent::Released { .. }))));
            assert_eq!(g.population.count(Role::Peasant), 1);
        }
    }

    #[test]
    fn bear_swipe_releases_captives() {
        let mut w = fight_world();
        w.steeds.push(crate::data::SteedDef {
            attacks_while_running: true,
            hit_box: (0.0, 0.0, 2.0, 2.0),
            attack_damage: 30,
            run_speed: 2.0,
            forest_speed_multiplier: 1.0,
            ..Default::default()
        });
        let mut g = GameState::new(&w, 0, 44);
        g.monarch.x = 0.0;
        g.greed
            .spawn_troll(crate::greed::EnemyKind::Squid, 0.75, 1, false, &mut g.rng);
        g.greed.trolls[0].victims = 2;
        let mut events = Vec::new();
        g.tick(
            &w,
            &Controls {
                direction: 1,
                start_sprint: true,
                ..Default::default()
            },
            &mut events,
        );
        assert!(g.monarch.is_running());
        assert_captives_released(&g, &events, 2);
    }

    #[test]
    fn knight_slash_releases_captives() {
        let w = fight_world();
        let mut g = GameState::new(&w, 0, 45);
        g.monarch.x = -20.0;
        g.population.spawn_knight(8.0, &mut g.rng);
        g.greed
            .spawn_troll(crate::greed::EnemyKind::Squid, 8.25, 1, false, &mut g.rng);
        g.greed.trolls[0].hp = 1;
        g.greed.trolls[0].victims = 2;
        let events = run_events(&mut g, &w, 0.5);
        assert!(events
            .iter()
            .any(|e| matches!(e, GameEvent::Unit(UnitEvent::Strike { .. }))));
        assert_captives_released(&g, &events, 2);
    }

    #[test]
    fn a_boss_smashes_a_wall() {
        let w = fight_world();
        let mut g = GameState::new(&w, 0, 4);
        g.monarch.x = -20.0;
        let wall = g
            .level
            .add(level::instantiate(&w, 0, Layer::Game, 5.0, 0.0, 0.1, 1.0));
        g.payables_dirty = true;
        g.greed
            .spawn_troll(crate::greed::EnemyKind::Boss, 12.0, 1, true, &mut g.rng);
        run_events(&mut g, &w, 15.0);
        assert!(g.level.index_of(wall).is_none(), "the wall stands");
    }

    #[test]
    fn god_mode_keeps_the_purse_and_the_crown() {
        let w = fight_world();
        let mut g = GameState::new(&w, 0, 5);
        g.god = true;
        g.monarch.x = 0.0;
        g.wallet.set(0);
        for _ in 0..2 {
            g.greed
                .spawn_troll(crate::greed::EnemyKind::Troll, 6.0, 1, false, &mut g.rng);
        }
        let ev = run_events(&mut g, &w, 20.0);
        assert!(!ev.contains(&GameEvent::CrownLost));
        assert!(!g.game_over);
        assert_eq!(g.wallet.coins(), GOD_COINS);
    }

    #[test]
    fn broke_monarch_loses_the_crown_and_the_game() {
        let w = fight_world();
        let mut g = GameState::new(&w, 0, 5);
        g.monarch.x = 0.0;
        g.wallet.set(0);
        for _ in 0..2 {
            g.greed
                .spawn_troll(crate::greed::EnemyKind::Troll, 6.0, 1, false, &mut g.rng);
        }
        let ev = run_events(&mut g, &w, 20.0);
        assert!(ev.contains(&GameEvent::CrownLost));
        assert!(ev.contains(&GameEvent::GameOver));
        assert!(g.game_over);
    }

    #[test]
    fn walk_to_a_wall_site_and_pay_for_it() {
        let mut w = empty_world(5);
        w.protos = vec![
            wall_proto("Wall0", 3, Some(1), 0),
            wall_proto("Wall1", 3, None, 10),
        ];
        w.layers = vec![crate::data::LayerDef { ratio: 1.0, z: 0.0 }; 7];
        let mut g = GameState::new(&w, 0, 1);
        let site = g
            .level
            .add(level::instantiate(&w, 0, Layer::Game, 3.0, 0.0, 0.1, 1.0));
        g.payables_dirty = true;
        let mut ev = Vec::new();

        let walk = Controls {
            direction: 1,
            ..Default::default()
        };
        while g.monarch.x < 2.9 {
            g.tick(&w, &walk, &mut ev);
        }
        for i in 0..90 {
            let c = Controls {
                pay: true,
                pay_down: i == 0,
                ..Default::default()
            };
            g.tick(&w, &c, &mut ev);
        }
        assert!(ev.contains(&GameEvent::Pay(PayEvent::Complete {
            id: PayableId(site),
            coins: 3
        })));
        assert_eq!(g.wallet.coins(), 2);
        let up = ev
            .iter()
            .find_map(|e| match e {
                GameEvent::Upgraded(u) => Some(*u),
                _ => None,
            })
            .expect("upgraded");
        let wall = &g.level.objects[g.level.index_of(up.new).unwrap()];
        assert_eq!(wall.proto, 1);
        assert!(build::under_construction(&w, wall));
    }

    #[test]
    fn ghost_leads_to_the_campfire_and_fades_after_two_hires_and_a_wall() {
        let mut w = empty_world(5);
        let mut wall = wall_proto("Wall1", 3, None, 0);
        wall.wall_level = Some(1);
        let ghost = Proto {
            name: "Ghost".to_string(),
            tag: "Untagged".to_string(),
            ..Default::default()
        };
        w.protos = vec![wall, ghost];
        w.layers = vec![crate::data::LayerDef { ratio: 1.0, z: 0.0 }; 7];
        let mut g = GameState::new(&w, 0, 4);
        let id = g
            .level
            .add(level::instantiate(&w, 1, Layer::Game, 10.0, 0.0, 0.1, 1.0));
        g.level.split_scenery(&w);
        g.tutorial = crate::tutorial::Tutorial::new(&w, &mut g.level, true);
        g.campfire = 30.0;
        g.monarch.x = 8.5;
        run(&mut g, &w, 6.0, Controls::default());
        // It came to the monarch and leads off toward the campfire, staying close.
        let x = g.tutorial.ghost.as_ref().unwrap().x;
        assert!(x > 10.0 && x <= 8.5 + 2.6, "ghost at {x}");

        for _ in 0..2 {
            g.population.spawn(Role::Peasant, 8.0, &mut g.rng);
        }
        g.level
            .add(level::instantiate(&w, 0, Layer::Game, -3.0, 0.0, 0.1, 1.0));
        let mut ev = Vec::new();
        let mut done = 0;
        for _ in 0..60 * 6 {
            ev.clear();
            g.tick(&w, &Controls::default(), &mut ev);
            done += ev
                .iter()
                .filter(|e| matches!(e, GameEvent::TutorialDone))
                .count();
        }
        assert_eq!(done, 1);
        assert!(g.tutorial.ghost.is_none());
        assert!(!g.level.all().any(|o| o.id == id));
    }

    #[cfg(feature = "save")]
    #[test]
    fn saves_from_before_the_tutorial_and_the_look_still_load() {
        let mut w = empty_world(5);
        w.steeds.push(Default::default());
        let mut g = GameState::new(&w, 0, 4);
        g.look.queen = true;
        let bytes = crate::save::save_game(&g, 1);
        assert!(crate::save::load_game(&bytes, 1, &w).unwrap().look.queen);
        let look = postcard::to_allocvec(&g.look).unwrap().len();
        let tutorial = postcard::to_allocvec(&g.tutorial).unwrap().len();
        for cut in [look, look + tutorial] {
            let mut old = bytes[..bytes.len() - cut].to_vec();
            old[4..6].copy_from_slice(&4u16.to_le_bytes());
            let loaded = crate::save::load_game(&old, 1, &w).unwrap();
            assert_eq!(loaded.look, Default::default());
        }
    }

    #[test]
    fn a_chosen_look_is_kept_for_new_runs() {
        let w = empty_world(5);
        let look = kingdom::Look {
            queen: true,
            pattern: 3,
            emblem: 7,
        };
        let coat = CoatOfArms {
            primary: [1.0, 0.0, 0.0],
            secondary: [0.0; 3],
            emblem: [1.0; 3],
        };
        let progress = crate::progress::Progress {
            chosen: Some((coat, look)),
            ..Default::default()
        };
        let g = GameState::start(&w, 0, 9, &progress, None);
        assert_eq!((g.coat, g.look), (coat, look));
        let random = GameState::start(&w, 0, 9, &Default::default(), None);
        assert!(random.look.pattern < kingdom::PATTERNS && random.look.emblem < kingdom::EMBLEMS);
    }
    #[cfg(feature = "save")]
    #[test]
    fn damaged_saves_are_rejected_without_panicking() {
        let mut w = fight_world();
        w.steeds.push(Default::default());
        let mut g = GameState::new(&w, 0, 7);
        let bytes = crate::save::save_game(&g, 42);
        assert!(crate::save::load_game(&bytes, 42, &w).is_ok());
        for i in 10..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[i] = damaged[i].wrapping_add(1);
            let _ = crate::save::load_game(&damaged, 42, &w);
        }
        for cut in 1..=3 {
            assert_eq!(
                crate::save::load_game(&bytes[..bytes.len() - cut], 42, &w).err(),
                Some(crate::save::LoadError::Corrupt)
            );
        }
        g.level_index = w.levels.len();
        assert_eq!(
            crate::save::load_game(&crate::save::save_game(&g, 42), 42, &w).err(),
            Some(crate::save::LoadError::Corrupt)
        );
        let p = crate::save::save_progress(&Default::default());
        assert_eq!(
            crate::save::load_progress(&p[..p.len() - 1]).err(),
            Some(crate::save::LoadError::Corrupt)
        );
    }
}
