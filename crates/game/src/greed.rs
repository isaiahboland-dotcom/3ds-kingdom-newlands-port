//! The greed: wave scheduling, portals spawning trolls, troll behaviour,
//! arrows, and the monarch's dropped crown.
//!
//! orig: `Director.ScheduleWaveToday()`, `EnemyManager` (`SpawnWave`,
//! `GetEnemies`, `CheckDanger`, wave cohesion), `Portal` (spawn queue),
//! `Troll`, `Arrow`, `Crown`, portal defence / destruction / rebuilding
//! and retaliation waves. Squids and bosses aren't ported yet.

use alloc::vec::Vec;

use crate::coins::Rect;
use crate::data::{ToolKind, WorldData};
use crate::director::{Season, WaveKind};
use crate::kingdom::Borders;
use crate::level::Level;
use crate::rng::Rng;
use crate::time::{SECONDS_PER_HOUR, WAVE_AT_CASTLE};
use crate::units::Mover;

/// orig: Troll prefab.
mod troll {
    pub const WALK: f32 = 2.0;
    pub const RUN: f32 = 2.8;
    pub const ANGRY_COOLDOWN: f32 = 12.0;
    pub const CHARGE_RANGE: f32 = 2.0;
    pub const CHARGE_SPEED: f32 = 3.0;
    pub const CHARGE_JUMP: (f32, f32) = (8.0, 2.3);
    pub const WALL_BOUNCE: (f32, f32) = (-3.0, 1.0);
    pub const ATTACK_COOLDOWN: f32 = 1.0;
    pub const HIT_DAMAGE: i32 = 1;
    pub const LOOT_RANGE: f32 = 3.0;
    pub const TOUGH_HEALTH: i32 = 5;
}
/// orig: Troll prefab z.
pub const TROLL_Z: f32 = 0.6;
/// Kingdom stays "in danger" while attackers are within this of the borders.
/// orig: `EnemyManager.safeBorderDistance`.
const SAFE_BORDER_DISTANCE: f32 = 20.0;
/// orig: `EnemyManager.retaliationDelayHours` (scene).
const RETALIATION_DELAY_HOURS: f32 = 2.0;
/// orig: `Director.ScheduleWaveToday`: the portal opens this many hours before the spawn.
const PORTAL_OPEN_LEAD: f32 = 6.0;
/// orig: Physics2DSettings gravity.
const GRAVITY: f32 = 15.0;
/// orig: Troll prefab trigger: offset (0, 0.35), size (0.3, 0.6).
const TROLL_BOX: Rect = Rect::centered(0.35, 0.3, 0.6);
/// orig: `Archer.maxForce` (prefab) on a 1 kg arrow.
pub const ARROW_SPEED: f32 = 8.5;
/// orig: Crown prefab gravity scale.
const CROWN_GRAVITY: f32 = 0.7 * GRAVITY;

/// orig: `EnemyType` (the ones ported).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnemyKind {
    Troll,
    ToughTroll,
    /// Flies over the walls and carries citizens off. orig: `Squid`.
    Squid,
    /// Smashes walls, spits out trolls. orig: `Boss`.
    Boss,
    /// A boss with 5000 hit points. orig: `EnemyType.KillerBoss`.
    KillerBoss,
}

impl EnemyKind {
    pub fn is_boss(self) -> bool {
        matches!(self, EnemyKind::Boss | EnemyKind::KillerBoss)
    }
}

/// orig: Squid prefab.
mod squid {
    pub const SPEED: f32 = 1.3;
    pub const ALTITUDE: f32 = 2.2;
    pub const SCAN: f32 = 5.0;
    pub const HP: i32 = 22;
    pub const CARRY: u8 = 2;
    /// orig: `hitBox` (x −0.2, y 0.3, 1 × 0.6) in front.
    pub const GRAB: (f32, f32, f32, f32) = (-0.2, 0.3, 1.0, 0.6);
}

/// orig: Boss prefab.
mod boss {
    pub const WALK: f32 = 1.2;
    pub const HP: i32 = 90;
    pub const KILLER_HP: i32 = 5000;
    pub const HIT_DAMAGE: i32 = 22;
    /// orig: `hitBox` (x 0, y 0.4, 1.6 × 0.7) in front.
    pub const REACH: f32 = 1.6;
    pub const BASH_TIME: f32 = 1.2;
    /// The punch lands partway into the bash clip.
    pub const BASH_IMPACT: f32 = 0.5;
    pub const PUKE_RANGE: f32 = 8.0;
    pub const PUKE_INTERVAL: f32 = 10.0;
    pub const PUKE_AMOUNT: usize = 4;
    pub const PUKE_OFFSET: (f32, f32) = (0.8, 1.2);
}

/// What a troll carries off. orig: `Troll._loot`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loot {
    Coin,
    Tool(ToolKind),
    Crown,
    /// A part stacked at the wharf (index into `WharfDef::parts`).
    /// orig: the `BoatPart` droppable.
    BoatPart(u16),
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrollState {
    /// orig: `Troll.Chase()`: walk at the monarch (or crown) for 3–5 s at a time.
    Chase { t: f32 },
    /// orig: `Troll.GrabLoot()`.
    GrabLoot { x: f32 },
    /// orig: `Troll.Charge()`: leap at a target; `hit` once something was struck.
    Charge {
        t: f32,
        landed: bool,
        hit: bool,
        recover: f32,
    },
    /// orig: `Troll.Retreat()`.
    Retreat { t: f32 },
    /// Knocked out, fading. orig: Damageable death + `troll_die`.
    Dead { t: f32 },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct Troll {
    pub id: u32,
    pub kind: EnemyKind,
    pub x: f32,
    pub y: f32,
    pub vy: f32,
    pub mover: Mover,
    pub hp: i32,
    pub state: TrollState,
    pub loot: Option<Loot>,
    pub wave: u32,
    pub boss_wave: bool,
    should_retreat: bool,
    /// Still part of an attack. orig: `Enemy.isAttacking`.
    attacking: bool,
    angry: f32,
    born: f32,
    wave_target_offset: f32,
    distance_from_wave_center: f32,
    speed_mult: f32,
    speed_mult_t: f32,
    target: Option<Target>,
    pub anim_t: f32,
    /// Squid: citizens it carries. orig: `_victims`.
    pub victims: u8,
    /// Boss: punching (time into it), puke timers. orig: `Boss` state.
    pub bash: Option<(f32, bool)>,
    pub puke: Option<f32>,
    since_puke: f32,
    puke_now: bool,
    reached_center: bool,
}

impl Troll {
    pub fn box_at(&self) -> Rect {
        match self.kind {
            EnemyKind::Boss | EnemyKind::KillerBoss => {
                Rect::centered(0.8, 1.0, 1.6).at(self.x, self.y, false)
            }
            EnemyKind::Squid => Rect::centered(0.3, 0.6, 0.6).at(self.x, self.y, false),
            _ => TROLL_BOX.at(self.x, self.y, false),
        }
    }

    pub fn is_dead(&self) -> bool {
        matches!(self.state, TrollState::Dead { .. })
    }

    fn airborne(&self) -> bool {
        self.y > 0.0 || self.vy > 0.0
    }
}

/// Something a troll can hit.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Monarch,
    Unit(u32),
    /// A wall object.
    Object(u32),
}

/// A potential troll target this tick, provided by the game state.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct TargetInfo {
    pub target: Target,
    pub x: f32,
    pub rect: Rect,
    /// orig: `TargetCacher` priority list (archers, knights).
    pub priority: bool,
    pub wall: bool,
}

/// Loot lying around for trolls to take.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct LootInfo {
    pub loot: Loot,
    pub x: f32,
    pub y: f32,
    /// Index in its owner's list, so the caller can remove it.
    pub index: usize,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arrow {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    /// Falls into the water and can't hit. orig: `Arrow.DropInWater()`.
    pub missed: bool,
    pub stuck: f32,
    /// Where it was loosed from (animals run from there).
    pub from_x: f32,
    /// orig: `hitDamage` (1), ×`perfectDamageMultiplier` (2) for a perfect shot.
    pub damage: i32,
}

/// A ballista bolt: flies flatter than an arrow and goes through up to
/// `maxHits` greed. orig: `Bolt`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Bolt {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub damage: i32,
    max_hits: u8,
    hit: Vec<u32>,
    pub stuck: f32,
}

/// orig: Bolt prefab gravity scale 0.7.
const BOLT_GRAVITY: f32 = 0.7 * GRAVITY;

/// The crown when it's off the monarch's head. orig: `Crown`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crown {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub t: f32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
struct ScheduledWave {
    /// Hours since day 0 when the portal opens / the wave spawns.
    open_at: f32,
    spawn_at: f32,
    portal: u32,
    wave: u16,
    day: u32,
    opened: bool,
}

/// orig: `Portal.State`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PortalPhase {
    Closed,
    Open,
    /// A regular portal fell; it lingers `t` more seconds as rubble.
    Crumbled {
        t: f32,
    },
    /// A cliff portal grows back over `left` seconds.
    Rebuilding {
        left: f32,
    },
}

/// orig: `Portal.SpawnCheck()` coroutine.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Defense {
    /// Looking for intruders once a second.
    Watch { t: f32 },
    /// Provoked; the first defenders come after `defenseSpawnDelay`.
    Provoked { t: f32 },
    /// Spawning a defence wave every `defenseSpawnCooldown` while intruders stay.
    Spawning { t: f32 },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
struct PortalState {
    object: u32,
    queue: Vec<(EnemyKind, u32, bool)>,
    next_in: f32,
    interval: f32,
    phase: PortalPhase,
    /// orig: `_closeAfterSpawn`.
    close_after: bool,
    defense: Defense,
}

impl PortalState {
    /// orig: `Portal.canSpawnWave`.
    fn can_spawn(&self) -> bool {
        matches!(self.phase, PortalPhase::Open | PortalPhase::Closed)
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GreedEvent {
    PortalOpened(u32),
    Spawned(u32),
    Charge(u32),
    /// A troll struck something.
    Hit {
        by: u32,
        target: Target,
        damage: i32,
    },
    /// A troll grabbed loot (`index` into the list it was given).
    TookLoot {
        by: u32,
        loot: Loot,
        index: usize,
    },
    Killed(u32),
    /// A troll got away with loot.
    Escaped {
        by: u32,
        loot: Option<Loot>,
    },
    /// The greed took the crown. orig: `Troll.PickupCrown()` → `Game.EndGame(Lost)`.
    CrownTaken,
    /// An arrow struck a portal. orig: `Arrow.HitObject()` on the portal's Damageable.
    PortalHit {
        object: u32,
        damage: i32,
    },
    /// A portal fell. orig: `Portal.Crumble()`, `EnemyManager.PortalDestroyed()`.
    PortalDestroyed {
        object: u32,
        cliff: bool,
    },
    /// The rubble of a regular portal is gone. orig: `Destroy(gameObject, 10)`.
    PortalRemoved(u32),
    /// A cliff portal stands again. orig: `Portal.Update()` (Rebuilding done).
    PortalRebuilt(u32),
    /// A squid snatched a citizen. orig: `Squid.GrabCitizen()` → `Character.Grab()`.
    Grabbed {
        by: u32,
        unit: u32,
    },
    /// A squid died and dropped its catch. orig: `Squid.HandleOnDeath()` → `Release()`.
    Released {
        x: f32,
        count: u8,
    },
    /// Two cliff portals are down at once. orig: `Director.OnCliffPortalDestroyed()` → `EndGame(Won)`.
    CliffsDestroyed,
    /// A retaliation wave came out. orig: `EnemyManager.SpawnRetaliation()`.
    Retaliation,
    /// EnemyManager admitted a wave with the authored boss flag.
    BossWaveSpawned {
        portal: u32,
    },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct Greed {
    pub trolls: Vec<Troll>,
    pub arrows: Vec<Arrow>,
    pub bolts: Vec<Bolt>,
    pub crown: Option<Crown>,
    scheduled: Vec<ScheduledWave>,
    portals: Vec<PortalState>,
    next_id: u32,
    next_wave: u32,
    cohesion_t: f32,
    /// orig: `EnemyManager._retaliationsQueued` / `_retaliationCountdown` /
    /// `_numRetaliationWavesSpawned`.
    retaliations_queued: u32,
    retaliation_t: f32,
    retaliations_spawned: u32,
    /// orig: `Director._cliffsCurrentlyDestroyed`.
    cliffs_down: u32,
}

/// A portal as the greed module sees it this tick.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct PortalInfo {
    pub object: u32,
    pub x: f32,
    /// Its trigger in ground-relative world coordinates: (x0, x1, y0, y1).
    pub hit_box: (f32, f32, f32, f32),
    pub by_arrows: bool,
    pub defense_range: f32,
    pub defense_delay: (f32, f32),
    pub defense_cooldown: f32,
    pub cliff: bool,
}

/// Inputs from the rest of the game.
pub struct GreedCtx<'a> {
    pub world: &'a WorldData,
    pub level_index: usize,
    pub campfire: f32,
    pub borders: Borders,
    pub monarch_x: f32,
    pub monarch_has_crown: bool,
    pub targets: &'a [TargetInfo],
    pub loot: &'a [LootInfo],
    /// (object id, x) of every portal.
    pub portals: &'a [(u32, f32)],
    pub portal_infos: &'a [PortalInfo],
    /// What provokes portals: the monarch, knights and archers.
    /// orig: `Portal._threatScanner` (tags Player, Knight, Archer).
    pub threats: &'a [f32],
    pub season: Season,
    pub day: u32,
    /// orig: `Kingdom.boat != null`.
    pub boat: bool,
}

impl Greed {
    pub fn portals_in(world: &WorldData, level: &Level) -> Vec<(u32, f32)> {
        level
            .objects
            .iter()
            .filter(|o| world.protos[o.proto as usize].portal.is_some())
            .map(|o| (o.id, o.x))
            .collect()
    }

    pub fn portal_open(&self, object: u32) -> bool {
        self.portals
            .iter()
            .any(|p| p.object == object && p.phase == PortalPhase::Open)
    }

    pub fn portal_phase(&self, object: u32) -> PortalPhase {
        self.portals
            .iter()
            .find(|p| p.object == object)
            .map_or(PortalPhase::Closed, |p| p.phase)
    }

    /// orig: `Portal.canSpawnWave`.
    pub fn portal_can_spawn(&self, object: u32) -> bool {
        self.portals
            .iter()
            .find(|p| p.object == object)
            .is_none_or(|p| p.can_spawn())
    }

    /// A portal's hit points ran out. orig: `Portal.HandleOnDeath()` →
    /// `Crumble()`, `EnemyManager.PortalDestroyed()`.
    pub fn portal_destroyed(
        &mut self,
        object: u32,
        cliff: bool,
        rebuild_days: f32,
        events: &mut impl FnMut(GreedEvent),
    ) {
        let p = self.portal_mut(object);
        if !p.can_spawn() {
            return;
        }
        p.queue.clear();
        p.phase = if cliff {
            PortalPhase::Rebuilding {
                left: rebuild_days * 24.0 * SECONDS_PER_HOUR,
            }
        } else {
            PortalPhase::Crumbled { t: 10.0 }
        };
        events(GreedEvent::PortalDestroyed { object, cliff });
        if cliff {
            self.cliffs_down += 1;
            if self.cliffs_down >= 2 {
                events(GreedEvent::CliffsDestroyed);
            }
        }
        if self.retaliations_queued == 0 {
            self.retaliation_t = RETALIATION_DELAY_HOURS * SECONDS_PER_HOUR;
        }
        self.retaliations_queued += 1;
    }

    /// The enemies a wave sends, queued at `portal` (opening it).
    /// orig: `Portal.SpawnEnemies()`.
    fn queue_wave(&mut self, portal: u32, enemies: Vec<EnemyKind>, boss: bool, close_after: bool) {
        if enemies.is_empty() {
            return;
        }
        self.next_wave += 1;
        let wave = self.next_wave;
        let p = self.portal_mut(portal);
        if p.phase != PortalPhase::Open {
            p.phase = PortalPhase::Open;
            p.next_in = 0.5;
        }
        p.interval = (15.0 / enemies.len() as f32).min(1.0);
        p.queue.extend(enemies.into_iter().map(|e| (e, wave, boss)));
        p.close_after = close_after;
    }

    /// orig: EnemyManager.SpawnWave. Defense waves call the portal directly.
    fn admit_wave(
        &mut self,
        portal: u32,
        wave: u16,
        day: u32,
        ctx: &GreedCtx,
        events: &mut impl FnMut(GreedEvent),
    ) -> bool {
        if !ctx.portals.iter().any(|&(id, _)| id == portal) || !self.portal_can_spawn(portal) {
            return false;
        }
        let enemies = Self::enemies(ctx.world, ctx.level_index, wave, day, ctx.boat);
        let boss = ctx.world.waves[wave as usize].boss_wave;
        self.queue_wave(portal, enemies, boss, true);
        if boss {
            events(GreedEvent::BossWaveSpawned { portal });
        }
        true
    }

    /// orig: `Kingdom.ClosestSpawner(side, checkOtherSideIfNoneFound: true)`.
    fn closest_portal(
        world: &WorldData,
        level: &Level,
        campfire: f32,
        side: i8,
    ) -> Option<(u32, f32)> {
        let all = Self::portals_in(world, level);
        let on = |s: i8| {
            all.iter()
                .copied()
                .filter(|p| {
                    if s < 0 {
                        p.1 < campfire
                    } else {
                        p.1 > campfire
                    }
                })
                .min_by(|a, b| (a.1 - campfire).abs().total_cmp(&(b.1 - campfire).abs()))
        };
        on(side).or_else(|| on(-side))
    }

    /// The enemies a wave sends on `day`. orig: `EnemyManager.GetEnemies()`.
    /// `boat`: the boat is in the water (more greed comes, except in boss waves).
    pub fn enemies(
        world: &WorldData,
        level_index: usize,
        wave: u16,
        day: u32,
        boat: bool,
    ) -> Vec<EnemyKind> {
        let w = &world.waves[wave as usize];
        let cfg = &world.levels[level_index];
        let mut mult = 1.0;
        let d = day as i32;
        if cfg.wave_multiplier_timeframe.0 <= d && d <= cfg.wave_multiplier_timeframe.1 {
            mult *= cfg.wave_timed_multiplier;
        }
        if !w.boss_wave && boat {
            mult *= cfg.wave_boat_multiplier;
        }
        let count =
            |c: &crate::data::Curve| libm::floorf(c.eval(day as f32) * mult).max(0.0) as usize;
        let mut out = Vec::new();
        out.extend(core::iter::repeat_n(EnemyKind::Troll, count(&w.trolls)));
        out.extend(core::iter::repeat_n(
            EnemyKind::ToughTroll,
            count(&w.tough_trolls),
        ));
        out.extend(core::iter::repeat_n(EnemyKind::Squid, count(&w.squids)));
        out.extend(core::iter::repeat_n(EnemyKind::Boss, count(&w.bosses)));
        out.extend(core::iter::repeat_n(
            EnemyKind::KillerBoss,
            count(&w.killer_bosses),
        ));
        out
    }

    fn wave_for(
        world: &WorldData,
        level_index: usize,
        kind: WaveKind,
        season: Season,
    ) -> Option<u16> {
        let k = match kind {
            WaveKind::Regular => 0,
            WaveKind::Boss => 1,
            WaveKind::Recovery => 2,
        };
        world.levels[level_index].waves[k][season_index(season)]
    }

    /// A wave is due today: work out when it must spawn to reach the border
    /// at `waveAtCastle`. orig: `Director.AddWaveEvents()` + `ScheduleWaveToday()`.
    #[allow(clippy::too_many_arguments)]
    pub fn schedule(
        &mut self,
        world: &WorldData,
        level: &Level,
        level_index: usize,
        kind: WaveKind,
        season: Season,
        day: u32,
        campfire: f32,
        borders: Borders,
        rng: &mut Rng,
    ) {
        let Some(wave) = Self::wave_for(world, level_index, kind, season) else {
            return;
        };
        let both = world.waves[wave as usize].both_sides;
        let sides: Vec<i8> = if both {
            alloc::vec![-1, 1]
        } else if rng.value() < 0.5 {
            alloc::vec![-1]
        } else {
            alloc::vec![1]
        };
        for side in sides {
            let Some((portal, px)) = Self::closest_portal(world, level, campfire, side) else {
                continue;
            };
            // orig: arrival at waveAtCastle, which is ≤ 12 so it means the next night.
            let arrival = if WAVE_AT_CASTLE > 12.0 {
                WAVE_AT_CASTLE
            } else {
                WAVE_AT_CASTLE + 24.0
            };
            // orig: EnemyManager.GetWaveTravelTime: distance to the border at the slowest member's speed.
            let w = &world.waves[wave as usize];
            // orig: GetSlowestInWave.
            let d = day as f32;
            let mut slowest: f32 = 5.0;
            if w.trolls.eval(d) >= 1.0 || w.tough_trolls.eval(d) >= 1.0 {
                slowest = troll::WALK;
            }
            if w.bosses.eval(d) >= 1.0 || w.killer_bosses.eval(d) >= 1.0 {
                slowest = slowest.min(boss::WALK);
            }
            if w.squids.eval(d) >= 1.0 {
                slowest = slowest.min(squid::SPEED * 1.2);
            }
            let border = borders.side(if px < campfire { -1 } else { 1 });
            let travel = (border - px).abs() / slowest / SECONDS_PER_HOUR;
            let spawn = day as f32 * 24.0 + arrival - travel;
            self.scheduled.push(ScheduledWave {
                open_at: spawn - PORTAL_OPEN_LEAD,
                spawn_at: spawn,
                portal,
                wave,
                day,
                opened: false,
            });
        }
    }

    /// orig: `EnemyManager.isDangerous`.
    pub fn is_dangerous(&self, borders: Borders) -> bool {
        self.retaliations_queued > 0
            || self.trolls.iter().any(|t| {
                t.attacking
                    && (t.boss_wave
                        || (t.x > borders.left - SAFE_BORDER_DISTANCE
                            && t.x < borders.right + SAFE_BORDER_DISTANCE))
            })
    }

    /// Dawn: trolls that retreat at dawn head home. orig: `EnemyManager.HandleOnDayStart()`.
    pub fn dawn(&mut self) {
        for t in &mut self.trolls {
            if !t.boss_wave {
                t.should_retreat = true;
            }
        }
    }

    /// orig: `EnemyManager.Retreat()` (game over).
    pub fn retreat_all(&mut self) {
        for t in &mut self.trolls {
            t.should_retreat = true;
        }
    }

    pub fn spawn_troll(
        &mut self,
        kind: EnemyKind,
        x: f32,
        wave: u32,
        boss_wave: bool,
        rng: &mut Rng,
    ) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let mut mover = Mover::default();
        mover.facing = 1;
        self.trolls.push(Troll {
            id,
            kind,
            x,
            y: 0.0,
            vy: 0.0,
            mover,
            hp: match kind {
                EnemyKind::ToughTroll => troll::TOUGH_HEALTH,
                EnemyKind::Squid => squid::HP,
                EnemyKind::Boss => boss::HP,
                EnemyKind::KillerBoss => boss::KILLER_HP,
                EnemyKind::Troll => 1,
            },
            state: TrollState::Chase { t: 0.0 },
            loot: None,
            wave,
            boss_wave,
            should_retreat: false,
            attacking: true,
            angry: 0.0,
            born: 0.0,
            wave_target_offset: rng.range(0.0, 10.0),
            distance_from_wave_center: 0.0,
            // orig: OnEnable: SetSpeedMultiplier(Random.Range(0.2, 0.7), 2).
            speed_mult: rng.range(0.2, 0.7),
            speed_mult_t: 2.0,
            target: None,
            anim_t: 0.0,
            victims: 0,
            bash: None,
            puke: None,
            since_puke: boss::PUKE_INTERVAL,
            puke_now: false,
            reached_center: false,
        });
        id
    }

    /// Damages a troll (arrows). orig: `Damageable.ReceiveDamage()`.
    pub fn hurt(&mut self, i: usize, damage: i32, events: &mut impl FnMut(GreedEvent)) {
        let t = &mut self.trolls[i];
        if t.is_dead() {
            return;
        }
        t.hp -= damage;
        // orig: Boss.HandleOnReceiveDamage: puke soon.
        t.puke_now = true;
        if t.hp <= 0 {
            t.state = TrollState::Dead { t: 0.0 };
            t.mover.stop();
            t.attacking = false;
            events(GreedEvent::Killed(t.id));
            if t.victims > 0 {
                events(GreedEvent::Released {
                    x: t.x,
                    count: t.victims,
                });
                t.victims = 0;
            }
        }
    }

    /// Fires an arrow. orig: `Archer.Shoot()` → `Arrow` with an impulse.
    pub fn shoot(&mut self, x: f32, y: f32, vx: f32, vy: f32, missed: bool, perfect: bool) {
        self.arrows.push(Arrow {
            x,
            y,
            vx,
            vy,
            missed,
            stuck: 0.0,
            from_x: x,
            damage: if perfect { 2 } else { 1 },
        });
    }

    /// orig: `Ballista.Fire()` → `Bolt.Launch()`.
    pub fn fire_bolt(&mut self, x: f32, y: f32, vx: f32, vy: f32, damage: i32, max_hits: u8) {
        self.bolts.push(Bolt {
            x,
            y,
            vx,
            vy,
            damage,
            max_hits,
            hit: Vec::new(),
            stuck: 0.0,
        });
    }

    fn update_bolts(&mut self, dt: f32, events: &mut impl FnMut(GreedEvent)) {
        let mut hits: Vec<(usize, i32)> = Vec::new();
        for b in &mut self.bolts {
            if b.stuck > 0.0 {
                b.stuck += dt;
                continue;
            }
            b.vy -= BOLT_GRAVITY * dt;
            b.x += b.vx * dt;
            b.y += b.vy * dt;
            if b.y <= 0.0 {
                b.y = 0.0;
                b.stuck = dt;
                continue;
            }
            // orig: Bolt.HitObject: each troll once, up to maxHits.
            for (i, t) in self.trolls.iter().enumerate() {
                if b.hit.len() >= b.max_hits as usize {
                    break;
                }
                if !t.is_dead() && !b.hit.contains(&t.id) && point_in(&t.box_at(), b.x, b.y) {
                    b.hit.push(t.id);
                    hits.push((i, b.damage));
                }
            }
        }
        for (i, d) in hits {
            self.hurt(i, d, events);
        }
        // orig: Pool.Despawn(gameObject, 10) once stuck.
        self.bolts.retain(|b| b.stuck < 10.0);
    }

    /// Drops the crown off the monarch's head. orig: `Player.LoseCrown()`.
    pub fn drop_crown(&mut self, x: f32, vx: f32, vy: f32) {
        self.crown = Some(Crown {
            x,
            y: 1.3,
            vx,
            vy,
            t: 0.0,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        dt: f32,
        hours_now: f32,
        ctx: &GreedCtx,
        level: &Level,
        rng: &mut Rng,
        mut events: impl FnMut(GreedEvent),
    ) {
        // Scheduled waves: open the portal, then queue the trolls.
        let mut i = 0;
        while i < self.scheduled.len() {
            let s = self.scheduled[i];
            if !s.opened && hours_now >= s.open_at {
                self.scheduled[i].opened = true;
                // orig: Portal.Open() only from Closed.
                let p = self.portal_mut(s.portal);
                if p.phase == PortalPhase::Closed {
                    p.phase = PortalPhase::Open;
                    events(GreedEvent::PortalOpened(s.portal));
                }
            }
            if hours_now >= s.spawn_at {
                self.scheduled.remove(i);
                self.admit_wave(s.portal, s.wave, s.day, ctx, &mut events);
                continue;
            }
            i += 1;
        }
        self.update_portals(dt, ctx, rng, &mut events);
        // Portal spawn queues. orig: Portal.Update.
        let mut spawns: Vec<(EnemyKind, f32, u32, bool)> = Vec::new();
        for p in &mut self.portals {
            if p.phase != PortalPhase::Open {
                continue;
            }
            if p.queue.is_empty() {
                // orig: Close() after a wave (regular portals only).
                if p.close_after {
                    p.close_after = false;
                    let cliff = ctx
                        .portal_infos
                        .iter()
                        .any(|i| i.object == p.object && i.cliff);
                    if !cliff {
                        p.phase = PortalPhase::Closed;
                    }
                }
                continue;
            }
            p.next_in -= dt;
            if p.next_in <= 0.0 {
                let Some(x) = level.index_of(p.object).map(|i| level.objects[i].x) else {
                    p.queue.clear();
                    continue;
                };
                // orig: a random entry of the queue.
                let k = rng.range_i32(0, p.queue.len() as i32) as usize;
                let (kind, wave, boss) = p.queue.remove(k);
                spawns.push((kind, x, wave, boss));
                p.next_in = p.interval;
            }
        }
        for (kind, x, wave, boss) in spawns {
            let id = self.spawn_troll(kind, x, wave, boss, rng);
            events(GreedEvent::Spawned(id));
        }

        self.update_cohesion(dt);
        self.update_trolls(dt, ctx, rng, &mut events);
        self.update_arrows(dt, ctx, &mut events);
        self.update_bolts(dt, &mut events);
        self.update_crown(dt);
    }

    fn portal_mut(&mut self, object: u32) -> &mut PortalState {
        if let Some(i) = self.portals.iter().position(|p| p.object == object) {
            return &mut self.portals[i];
        }
        self.portals.push(PortalState {
            object,
            queue: Vec::new(),
            next_in: 0.0,
            interval: 1.0,
            phase: PortalPhase::Closed,
            close_after: false,
            defense: Defense::Watch { t: 0.0 },
        });
        self.portals.last_mut().expect("just pushed")
    }

    /// Rubble, rebuilding, defence waves and retaliation.
    /// orig: `Portal.Update()` / `SpawnCheck()`, `EnemyManager.Update()`.
    fn update_portals(
        &mut self,
        dt: f32,
        ctx: &GreedCtx,
        rng: &mut Rng,
        events: &mut impl FnMut(GreedEvent),
    ) {
        for info in ctx.portal_infos {
            self.portal_mut(info.object);
        }
        let mut defence: Vec<u32> = Vec::new();
        for p in &mut self.portals {
            match p.phase {
                PortalPhase::Crumbled { t } => {
                    let t = t - dt;
                    if t <= 0.0 {
                        events(GreedEvent::PortalRemoved(p.object));
                    }
                    p.phase = PortalPhase::Crumbled { t };
                    continue;
                }
                PortalPhase::Rebuilding { left } => {
                    let left = left - dt;
                    if left <= 0.0 {
                        // orig: Damageable.Reset, Closed, then Open().
                        p.phase = PortalPhase::Open;
                        p.defense = Defense::Watch { t: 0.0 };
                        self.cliffs_down = self.cliffs_down.saturating_sub(1);
                        events(GreedEvent::PortalRebuilt(p.object));
                    } else {
                        p.phase = PortalPhase::Rebuilding { left };
                    }
                    continue;
                }
                _ => {}
            }
            let Some(info) = ctx.portal_infos.iter().find(|i| i.object == p.object) else {
                continue;
            };
            let threatened = ctx
                .threats
                .iter()
                .any(|&x| (x - info.x).abs() <= info.defense_range);
            p.defense = match p.defense {
                Defense::Watch { t } => {
                    let t = t - dt;
                    if t > 0.0 {
                        Defense::Watch { t }
                    } else if p.phase == PortalPhase::Open && threatened {
                        Defense::Provoked {
                            t: rng.range(info.defense_delay.0, info.defense_delay.1),
                        }
                    } else {
                        Defense::Watch { t: 1.0 }
                    }
                }
                Defense::Provoked { t } | Defense::Spawning { t } => {
                    let t = t - dt;
                    if t > 0.0 {
                        if matches!(p.defense, Defense::Provoked { .. }) {
                            Defense::Provoked { t }
                        } else {
                            Defense::Spawning { t }
                        }
                    } else if threatened {
                        defence.push(p.object);
                        Defense::Spawning {
                            t: info.defense_cooldown,
                        }
                    } else {
                        Defense::Watch { t: 1.0 }
                    }
                }
            };
        }
        // orig: Portal.SpawnDefenseWave: the season's defence wave, today's size.
        let s = season_index(ctx.season);
        if let Some(wave) = ctx.world.levels[ctx.level_index].defence_waves[s] {
            for object in defence {
                let enemies = Self::enemies(ctx.world, ctx.level_index, wave, ctx.day, ctx.boat);
                self.queue_wave(object, enemies, false, false);
            }
        }
        // orig: EnemyManager.Update → SpawnRetaliation every retaliationDelayHours.
        if self.retaliations_queued > 0 {
            self.retaliation_t -= dt;
            if self.retaliation_t <= 0.0 {
                self.retaliation_t = RETALIATION_DELAY_HOURS * SECONDS_PER_HOUR;
                self.retaliations_queued -= 1;
                let side = if rng.value() < 0.5 { 1 } else { -1 };
                if let Some(wave) = ctx.world.levels[ctx.level_index].retaliation_waves[s] {
                    if let Some(portal) = self.closest_live_portal(ctx, side) {
                        if self.admit_wave(portal, wave, self.retaliations_spawned, ctx, events) {
                            events(GreedEvent::Retaliation);
                        }
                    }
                }
                self.retaliations_spawned += 1;
            }
        }
    }

    /// orig: `Kingdom.ClosestSpawner(side, checkOtherSideIfNoneFound: true)` + `canSpawnWave`.
    fn closest_live_portal(&self, ctx: &GreedCtx, side: i8) -> Option<u32> {
        let on = |s: i8| {
            ctx.portals
                .iter()
                .filter(|p| (p.1 < ctx.campfire) == (s < 0))
                .min_by(|a, b| {
                    (a.1 - ctx.campfire)
                        .abs()
                        .total_cmp(&(b.1 - ctx.campfire).abs())
                })
                .map(|p| p.0)
        };
        on(side)
            .or_else(|| on(-side))
            .filter(|&p| self.portal_can_spawn(p))
    }

    /// Every 3 s, how far each troll is ahead of its wave's middle.
    /// orig: `EnemyManager.UpdateWavePositions()`.
    fn update_cohesion(&mut self, dt: f32) {
        self.cohesion_t -= dt;
        if self.cohesion_t > 0.0 {
            return;
        }
        self.cohesion_t = 3.0;
        let mut waves: Vec<(u32, f32, f32)> = Vec::new();
        for t in &self.trolls {
            match waves.iter_mut().find(|w| w.0 == t.wave) {
                Some(w) => {
                    w.1 = w.1.min(t.x);
                    w.2 = w.2.max(t.x);
                }
                None => waves.push((t.wave, t.x, t.x)),
            }
        }
        for t in &mut self.trolls {
            if let Some(w) = waves.iter().find(|w| w.0 == t.wave) {
                t.distance_from_wave_center = t.x - (w.1 + w.2) / 2.0;
            }
        }
    }

    fn update_trolls(
        &mut self,
        dt: f32,
        ctx: &GreedCtx,
        rng: &mut Rng,
        events: &mut impl FnMut(GreedEvent),
    ) {
        let mut taken: Vec<usize> = Vec::new();
        let mut gone: Vec<usize> = Vec::new();
        let mut pukes: Vec<(f32, i8, u32)> = Vec::new();
        for (ti, t) in self.trolls.iter_mut().enumerate() {
            if t.kind != EnemyKind::Troll && t.kind != EnemyKind::ToughTroll {
                t.anim_t += dt;
                let done = if t.kind == EnemyKind::Squid {
                    update_squid(t, dt, ctx, events)
                } else {
                    update_boss(t, dt, ctx, rng, &mut pukes, events)
                };
                if done {
                    gone.push(ti);
                }
                continue;
            }
            t.born += dt;
            t.angry -= dt;
            t.anim_t += dt;
            if t.speed_mult_t > 0.0 {
                t.speed_mult_t -= dt;
                if t.speed_mult_t <= 0.0 {
                    t.speed_mult = 1.0;
                }
            }
            // Transitions. orig: Troll FSM (checked every tick while chasing).
            if let TrollState::Chase { .. } | TrollState::GrabLoot { .. } = t.state {
                if t.loot.is_some() || t.should_retreat {
                    // orig: ShouldRetreat
                    t.state = TrollState::Retreat { t: 0.0 };
                    t.attacking = false;
                } else if let Some(target) = pick_target(t, ctx, true) {
                    start_charge(t, target, events);
                } else if let (TrollState::Chase { .. }, Some(l)) =
                    (t.state, nearest_loot(t, ctx, &taken))
                {
                    t.state = TrollState::GrabLoot { x: l.x };
                } else if let (TrollState::Chase { .. }, Some(target)) =
                    (t.state, pick_target(t, ctx, false))
                {
                    start_charge(t, target, events);
                }
            }
            match t.state {
                TrollState::Chase { t: tt } => {
                    if tt <= 0.0 {
                        // orig: Troll.Chase: head for the monarch, or the dropped crown.
                        let goal = if ctx.monarch_has_crown {
                            Some(ctx.monarch_x)
                        } else {
                            None
                        };
                        let dist = goal.map_or(rng.value() - 0.5, |g| g - t.x);
                        let dir: i8 = if dist > 0.0 { 1 } else { -1 };
                        let walk =
                            if t.distance_from_wave_center * dir as f32 > t.wave_target_offset {
                                troll::WALK * 0.3
                            } else {
                                troll::WALK
                            };
                        let speed = if t.angry > 0.0 { troll::RUN } else { walk };
                        t.mover.set_speed(speed * t.speed_mult, dir);
                        if t.born > 5.0 && rng.value() < 0.3 {
                            // orig: Mover.Jump((4, 3), dir) + half speed for 1.2 s.
                            t.mover.vx = 4.0 * dir as f32;
                            t.vy = 3.0;
                            t.speed_mult = 0.5;
                            t.speed_mult_t = 1.2;
                        }
                        t.state = TrollState::Chase {
                            t: rng.range(3.0, 5.0),
                        };
                    } else {
                        t.state = TrollState::Chase { t: tt - dt };
                    }
                }
                TrollState::GrabLoot { x } => {
                    t.mover.set_goal(x, troll::RUN);
                    if !t.mover.moving_to_goal {
                        t.state = TrollState::Chase { t: 0.0 };
                    }
                }
                TrollState::Charge {
                    t: ct,
                    landed,
                    hit,
                    recover,
                } => {
                    let ct = ct + dt;
                    let mut hit = hit;
                    let mut landed = landed;
                    if !landed && !hit {
                        // orig: OnTriggerStay2D / FixedUpdate cast while charging.
                        if let Some(info) =
                            ctx.targets.iter().find(|i| t.box_at().overlaps(&i.rect))
                        {
                            hit = true;
                            events(GreedEvent::Hit {
                                by: t.id,
                                target: info.target,
                                damage: troll::HIT_DAMAGE,
                            });
                            let dir = t.mover.facing as f32;
                            if info.wall {
                                // orig: PauseMotion then wallBounceForce.
                                t.mover.vx = troll::WALL_BOUNCE.0 * dir * rng.range(0.8, 1.2);
                                t.vy = troll::WALL_BOUNCE.1;
                            } else {
                                t.mover.vx *= 0.8;
                            }
                        }
                    }
                    if !landed && ct > 0.1 && !t.airborne() {
                        // orig: touching the ground ends the leap.
                        landed = true;
                        t.mover.vx *= 0.3;
                        t.mover.set_speed(0.0, 0);
                    }
                    if hit && !landed && !t.airborne() {
                        landed = true;
                        t.mover.set_speed(0.0, 0);
                    }
                    if landed {
                        let recover = recover + dt;
                        if recover >= troll::ATTACK_COOLDOWN {
                            t.state = TrollState::Chase { t: 0.0 };
                        } else {
                            t.state = TrollState::Charge {
                                t: ct,
                                landed,
                                hit,
                                recover,
                            };
                        }
                    } else {
                        t.state = TrollState::Charge {
                            t: ct,
                            landed,
                            hit,
                            recover,
                        };
                    }
                }
                TrollState::Retreat { t: rt } => {
                    // orig: Troll.Retreat: back to the nearest portal on this side, then vanish.
                    let side = if t.x < ctx.campfire { -1 } else { 1 };
                    match retreat_portal(ctx, t.x, side) {
                        Some(px) => {
                            t.mover.set_goal(px, troll::RUN);
                            if !t.mover.moving_to_goal {
                                gone.push(ti);
                            }
                        }
                        None => {
                            t.mover.set_speed(troll::RUN, side);
                            if rt > 9.0 {
                                gone.push(ti);
                            }
                        }
                    }
                    t.state = TrollState::Retreat { t: rt + dt };
                }
                TrollState::Dead { t: dt0 } => {
                    t.mover.stop();
                    if dt0 > 1.5 {
                        gone.push(ti);
                    }
                    t.state = TrollState::Dead { t: dt0 + dt };
                }
            }

            // Loot pickup on touch. orig: Troll.OnTriggerStay2D → PickupLoot.
            if t.loot.is_none() && !t.is_dead() && !matches!(t.state, TrollState::Charge { .. }) {
                if let Some(l) = ctx
                    .loot
                    .iter()
                    .find(|l| !taken.contains(&l.index) && t.box_at().overlaps(&loot_rect(l)))
                {
                    t.loot = Some(l.loot);
                    taken.push(l.index);
                    events(GreedEvent::TookLoot {
                        by: t.id,
                        loot: l.loot,
                        index: l.index,
                    });
                    if l.loot == Loot::Crown {
                        events(GreedEvent::CrownTaken);
                    }
                }
            }

            // Physics: leaps are ballistic; on the ground the mover drives.
            let was = t.x;
            if t.airborne() {
                t.vy -= GRAVITY * dt;
                t.y += t.vy * dt;
                t.x += t.mover.vx * dt;
                if t.y <= 0.0 {
                    t.y = 0.0;
                    t.vy = 0.0;
                }
            } else {
                t.mover.update(&mut t.x, dt);
            }
            // orig: standing walls are solid to the greed's Rigidbody2D
            // (squids fly over them).
            if t.kind != EnemyKind::Squid {
                // A hair of overlap, so a body against the wall touches it
                // (its charge connects).
                let half = t.box_at().w / 2.0 - 0.05;
                for w in ctx.targets.iter().filter(|w| w.wall) {
                    let (left, right) = (w.rect.x - half, w.rect.x + w.rect.w + half);
                    let stop = if was <= left && t.x > left {
                        Some(left)
                    } else if was >= right && t.x < right {
                        Some(right)
                    } else {
                        None
                    };
                    if let Some(x) = stop {
                        t.x = x;
                        t.mover.vx = 0.0;
                    }
                }
            }
        }
        // orig: Boss.Puke: trolls tumble out in front of it.
        for (x, dir, wave) in pukes {
            for _ in 0..boss::PUKE_AMOUNT {
                let off = rng.range(boss::PUKE_OFFSET.0, boss::PUKE_OFFSET.1);
                let id = self.spawn_troll(EnemyKind::Troll, x + dir as f32 * off, wave, false, rng);
                events(GreedEvent::Spawned(id));
            }
        }
        gone.sort_unstable();
        gone.dedup();
        for &i in gone.iter().rev() {
            let t = self.trolls.remove(i);
            if !t.is_dead() {
                events(GreedEvent::Escaped {
                    by: t.id,
                    loot: t.loot,
                });
            }
        }
    }

    fn update_arrows(&mut self, dt: f32, ctx: &GreedCtx, events: &mut impl FnMut(GreedEvent)) {
        let mut hits: Vec<(usize, i32)> = Vec::new();
        for a in &mut self.arrows {
            if a.stuck > 0.0 {
                a.stuck += dt;
                continue;
            }
            a.vy -= GRAVITY * dt;
            a.x += a.vx * dt;
            a.y += a.vy * dt;
            if a.y <= 0.0 || (a.missed && a.y < -1.0) {
                if !a.missed {
                    a.y = 0.0;
                }
                a.stuck = dt;
                continue;
            }
            if a.missed {
                continue;
            }
            // orig: Arrow.OnTriggerEnter2D: damage the first troll hit.
            if let Some(i) = self
                .trolls
                .iter()
                .position(|t| !t.is_dead() && point_in(&t.box_at(), a.x, a.y))
            {
                hits.push((i, a.damage));
                a.stuck = 2.0;
                continue;
            }
            // orig: the portal's trigger takes arrows too (Damageable, Arrow).
            if let Some(p) = ctx.portal_infos.iter().find(|p| {
                p.by_arrows
                    && a.x >= p.hit_box.0
                    && a.x <= p.hit_box.1
                    && a.y >= p.hit_box.2
                    && a.y <= p.hit_box.3
            }) {
                if self
                    .portals
                    .iter()
                    .any(|q| q.object == p.object && q.can_spawn())
                {
                    a.stuck = 2.0;
                    events(GreedEvent::PortalHit {
                        object: p.object,
                        damage: a.damage,
                    });
                }
            }
        }
        for (i, damage) in hits {
            self.hurt(i, damage, events);
        }
        self.arrows.retain(|a| a.stuck < 2.0);
    }

    fn update_crown(&mut self, dt: f32) {
        let Some(c) = &mut self.crown else { return };
        c.t += dt;
        c.vy -= CROWN_GRAVITY * dt;
        c.x += c.vx * dt;
        c.y += c.vy * dt;
        if c.y <= 0.0 {
            c.y = 0.0;
            c.vy = 0.0;
            c.vx *= 0.9;
        }
    }
}

fn season_index(season: Season) -> usize {
    match season {
        Season::Summer => 0,
        Season::Autumn => 1,
        Season::Winter => 2,
    }
}

/// orig: `Squid` FSM: fly at `baseAltitude` toward the campfire, swoop on
/// citizens in reach, carry off two, then head home to a portal.
/// Returns true when it vanishes.
fn update_squid(
    t: &mut Troll,
    dt: f32,
    ctx: &GreedCtx,
    events: &mut impl FnMut(GreedEvent),
) -> bool {
    if let TrollState::Dead { t: d } = t.state {
        // Falls and lies there a while.
        t.y = (t.y - 3.0 * dt).max(0.0);
        t.state = TrollState::Dead { t: d + dt };
        return d > 3.0;
    }
    // orig: ShouldRetreat: told to, full hands, or at the campfire.
    let retreating = t.should_retreat
        || t.victims >= squid::CARRY
        || (ctx.campfire - t.x).abs() < 2.0
        || matches!(t.state, TrollState::Retreat { .. });
    let mut altitude = squid::ALTITUDE;
    if retreating {
        if !matches!(t.state, TrollState::Retreat { .. }) {
            t.state = TrollState::Retreat { t: 0.0 };
            t.attacking = false;
        }
        let side = if t.x < ctx.campfire { -1 } else { 1 };
        match retreat_portal(ctx, t.x, side) {
            Some(px) => {
                t.mover.set_goal(px, squid::SPEED);
                if (px - t.x).abs() < 3.0 {
                    // orig: Despawn: the victims are gone with it.
                    events(GreedEvent::Escaped {
                        by: t.id,
                        loot: None,
                    });
                    return true;
                }
            }
            None => {
                altitude = 10.0;
                t.mover.set_speed(squid::SPEED, side);
                if let TrollState::Retreat { t: rt } = t.state {
                    t.state = TrollState::Retreat { t: rt + dt };
                    if rt > 10.0 {
                        return true;
                    }
                }
            }
        }
    } else {
        // orig: Chase: the closest grabbable citizen within scanRange, else the campfire.
        let prey = ctx
            .targets
            .iter()
            .filter(|i| matches!(i.target, Target::Unit(_)) && (i.x - t.x).abs() <= squid::SCAN)
            .min_by(|a, b| (a.x - t.x).abs().total_cmp(&(b.x - t.x).abs()));
        match prey {
            Some(p) => {
                // Stay high if a wall stands between.
                let wall_between = ctx
                    .targets
                    .iter()
                    .any(|w| w.wall && (w.x - t.x) * (w.x - p.x) < 0.0);
                altitude = if wall_between { squid::ALTITUDE } else { 0.0 };
                t.mover.set_goal(p.x, squid::SPEED);
            }
            None => t.mover.set_goal(ctx.campfire, squid::SPEED),
        }
        // orig: ShouldGrabCitizen: a citizen in its hit box.
        let dir = t.mover.facing as f32;
        let (gx, gy, gw, gh) = squid::GRAB;
        let (a, b) = (t.x + gx * dir, t.x + (gx + gw) * dir);
        let grab = Rect {
            x: a.min(b),
            y: t.y + gy,
            w: gw,
            h: gh,
        };
        if let Some(i) = ctx
            .targets
            .iter()
            .find(|i| matches!(i.target, Target::Unit(_)) && i.rect.overlaps(&grab))
        {
            if let Target::Unit(u) = i.target {
                t.victims += 1;
                events(GreedEvent::Grabbed { by: t.id, unit: u });
            }
        }
    }
    // Flapping: ease toward the wanted altitude. orig: FlyUp lift vs gravity.
    t.y += (altitude - t.y) * (2.0 * dt).min(1.0);
    t.mover.update(&mut t.x, dt);
    false
}

/// orig: `Boss` FSM: march to the campfire, then after the monarch; bash
/// whatever's in front (walls, citizens, the monarch); spit out trolls
/// when hurt or near the monarch; die instead of retreating.
fn update_boss(
    t: &mut Troll,
    dt: f32,
    ctx: &GreedCtx,
    rng: &mut Rng,
    pukes: &mut Vec<(f32, i8, u32)>,
    events: &mut impl FnMut(GreedEvent),
) -> bool {
    let _ = rng;
    if let TrollState::Dead { t: d } = t.state {
        t.state = TrollState::Dead { t: d + dt };
        // orig: Pool.Despawn(gameObject, 10).
        return d > 10.0;
    }
    if t.should_retreat {
        // orig: Boss.Update: ReceiveDamage(hitPoints) on retreat.
        t.hp = 0;
        t.state = TrollState::Dead { t: 0.0 };
        t.attacking = false;
        events(GreedEvent::Killed(t.id));
        return false;
    }
    t.since_puke += dt;
    let dir = t.mover.facing as f32;
    let in_front = |x: f32| {
        let d = (x - t.x) * dir;
        (0.0..=boss::REACH).contains(&d)
    };
    if let Some((bt, landed)) = t.bash {
        let bt = bt + dt;
        let mut landed = landed;
        if !landed && bt >= boss::BASH_IMPACT {
            landed = true;
            // orig: OnAnimBashImpact: everything Ogre-damageable in the box.
            let mut hits = Vec::new();
            for i in ctx.targets {
                let d = (i.x - t.x) * dir;
                if (0.0..=boss::REACH).contains(&d) {
                    hits.push(i.target);
                }
            }
            if ctx.monarch_has_crown && in_front(ctx.monarch_x) && !hits.contains(&Target::Monarch)
            {
                hits.push(Target::Monarch);
            }
            for target in hits {
                events(GreedEvent::Hit {
                    by: t.id,
                    target,
                    damage: boss::HIT_DAMAGE,
                });
            }
        }
        t.bash = if bt >= boss::BASH_TIME {
            None
        } else {
            Some((bt, landed))
        };
        return false;
    }
    if let Some(pt) = t.puke {
        let pt = pt + dt;
        if pt >= 0.5 && pt - dt < 0.5 {
            pukes.push((t.x, t.mover.facing, t.wave));
        }
        t.puke = if pt >= 1.5 { None } else { Some(pt) };
        return false;
    }
    // orig: ShouldPuke (interrupts Chase).
    let monarch_ahead =
        (ctx.monarch_x - t.x) * dir >= 0.0 && (ctx.monarch_x - t.x).abs() <= boss::PUKE_RANGE;
    if t.since_puke > boss::PUKE_INTERVAL && (t.puke_now || monarch_ahead) {
        t.puke_now = false;
        t.since_puke = 0.0;
        t.mover.stop();
        t.puke = Some(0.0);
        return false;
    }
    // orig: ShouldBash: an obstacle, the monarch or a citizen in front.
    let target_ahead = ctx.targets.iter().any(|i| in_front(i.x)) || in_front(ctx.monarch_x);
    if target_ahead {
        t.mover.stop();
        t.bash = Some((0.0, false));
        t.anim_t = 0.0;
        return false;
    }
    // orig: Chase: the campfire first, then the crowned monarch.
    if (t.x - ctx.campfire).abs() < 0.5 {
        t.reached_center = true;
    }
    let goal = if t.reached_center && ctx.monarch_has_crown {
        ctx.monarch_x
    } else {
        ctx.campfire
    };
    t.mover.set_goal(goal, boss::WALK);
    t.mover.update(&mut t.x, dt);
    false
}

fn point_in(r: &Rect, x: f32, y: f32) -> bool {
    x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
}

fn loot_rect(l: &LootInfo) -> Rect {
    Rect::centered(0.15, 0.25, 0.3).at(l.x, l.y, false)
}

/// orig: `Troll.ShouldChargePriorityTarget()` (walls ahead and archers) and
/// `ShouldChargeOtherTarget()` (everyone else), within `chargeRange`.
fn pick_target(t: &Troll, ctx: &GreedCtx, priority: bool) -> Option<TargetInfo> {
    if t.is_dead()
        || matches!(
            t.state,
            TrollState::Charge { .. } | TrollState::Retreat { .. }
        )
    {
        return None;
    }
    let dir = t.mover.facing as f32;
    let candidates = ctx.targets.iter().filter(|i| {
        if priority {
            // Walls: the wall scanner only looks ahead (rangeBehind = 0).
            (i.wall && (i.x - t.x) * dir >= 0.0 && (i.x - t.x).abs() <= troll::CHARGE_RANGE)
                || (i.priority && (i.x - t.x).abs() < troll::CHARGE_RANGE)
        } else {
            !i.wall && !i.priority && (i.x - t.x).abs() < troll::CHARGE_RANGE
        }
    });
    candidates
        .min_by(|a, b| (a.x - t.x).abs().total_cmp(&(b.x - t.x).abs()))
        .copied()
}

fn start_charge(t: &mut Troll, target: TargetInfo, events: &mut impl FnMut(GreedEvent)) {
    let dir: i8 = if t.x < target.x { 1 } else { -1 };
    t.angry = troll::ANGRY_COOLDOWN;
    t.target = Some(target.target);
    t.mover.set_speed(troll::CHARGE_SPEED, dir);
    // orig: Mover.Jump(chargeJumpForce).
    t.mover.vx = troll::CHARGE_JUMP.0 * dir as f32;
    t.vy = troll::CHARGE_JUMP.1;
    t.state = TrollState::Charge {
        t: 0.0,
        landed: false,
        hit: false,
        recover: 0.0,
    };
    events(GreedEvent::Charge(t.id));
}

/// orig: `Troll.ShouldGrabLoot()`: any droppable within `lootRange`.
fn nearest_loot<'a>(t: &Troll, ctx: &'a GreedCtx, taken: &[usize]) -> Option<&'a LootInfo> {
    ctx.loot
        .iter()
        .filter(|l| !taken.contains(&l.index) && (l.x - t.x).abs() <= troll::LOOT_RANGE)
        .min_by(|a, b| (a.x - t.x).abs().total_cmp(&(b.x - t.x).abs()))
}

/// orig: `Kingdom.ClosestSpawnerOnSameSideOfCampfire()`.
fn retreat_portal(ctx: &GreedCtx, x: f32, side: i8) -> Option<f32> {
    ctx.portals
        .iter()
        .map(|p| p.1)
        .filter(|&px| {
            if side < 0 {
                px < ctx.campfire
            } else {
                px > ctx.campfire
            }
        })
        .min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Curve, LevelConfigDef, WaveDef};
    use alloc::string::ToString;
    use alloc::vec;

    fn flat(v: f32) -> Curve {
        Curve {
            keys: vec![[0.0, v, 0.0, 0.0], [100.0, v, 0.0, 0.0]],
        }
    }

    #[test]
    fn curve_eval_matches_unity_shapes() {
        let c = Curve {
            keys: vec![
                [0.0, 0.0, 0.0, 1.0],
                [2.0, 2.0, 1.0, f32::INFINITY],
                [3.0, 5.0, 0.0, 0.0],
            ],
        };
        assert!((c.eval(1.0) - 1.0).abs() < 1e-5, "{}", c.eval(1.0));
        // Stepped tangent: holds the left value until the next key.
        assert_eq!(c.eval(2.5), 2.0);
        assert_eq!(c.eval(10.0), 5.0);
        assert_eq!(c.eval(-1.0), 0.0);
    }

    #[test]
    fn wave_size_uses_the_day_and_early_multiplier() {
        let w = WorldData {
            waves: vec![WaveDef {
                name: "r".to_string(),
                trolls: flat(4.0),
                tough_trolls: flat(1.0),
                ..Default::default()
            }],
            levels: vec![LevelConfigDef {
                name: "t".to_string(),
                autumn_start: (0, 0),
                winter_start: (0, 0),
                days_before_game_over: -1,
                starting_coins: 0,
                min_level_width: 0,
                group_counts: vec![],
                waves: Default::default(),
                wave_timed_multiplier: 0.5,
                wave_multiplier_timeframe: (0, 5),
                season_change_days: 3.0,
                income_multiplier: 1.0,
                free_boat_parts: 0,
                wave_boat_multiplier: 1.0,
                force_all_unlocks: false,
                defence_waves: [None; 3],
                retaliation_waves: [None; 3],
            }],
            ..Default::default()
        };
        let early = Greed::enemies(&w, 0, 0, 2, false);
        assert_eq!(early.iter().filter(|&&e| e == EnemyKind::Troll).count(), 2);
        assert_eq!(
            early
                .iter()
                .filter(|&&e| e == EnemyKind::ToughTroll)
                .count(),
            0
        );
        let later = Greed::enemies(&w, 0, 0, 9, false);
        assert_eq!(later.len(), 5);
    }

    #[test]
    fn launch_reaches_the_target() {
        let (vx, vy) = crate::units::tests_support::launch(4.0, -0.05, ARROW_SPEED, true);
        // Max range is v²/g ≈ 4.8, so aim at 4 and check it passes near the target.
        let (mut x, mut y, mut vy2) = (0.0f32, 0.0f32, vy);
        while x < 4.0 && y > -3.0 {
            vy2 -= GRAVITY / 600.0;
            x += vx / 600.0;
            y += vy2 / 600.0;
        }
        assert!((y + 0.05).abs() < 0.15, "y at target {y}");
    }
}
