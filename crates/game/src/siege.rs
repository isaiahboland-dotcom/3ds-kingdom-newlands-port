//! The workshop's catapults: builders push one out to the wall, crank it,
//! and it lobs boulders at greed in front of it.
//!
//! orig: `PayableWorkshop`, `Catapult`, `Boulder` (friendly).

use alloc::vec::Vec;

use crate::build::{under_construction, WALK_Y};
use crate::coins::Rect;
use crate::data::WorldData;
use crate::kingdom::Borders;
use crate::level::{self, Level};
use crate::rng::Rng;

/// orig: `Catapult.ArmMaxAngle`, `BorderDistance`.
const ARM_MAX: f32 = 68.0;
const BORDER_DISTANCE: f32 = 2.5;
/// orig: `Update()` while Moving: 0.15 per pushing builder.
const PUSH_SPEED: f32 = 0.15;
/// The fire clip: the boulder leaves partway, the arm settles after.
const RELEASE_AT: f32 = 0.3;
const FIRE_TIME: f32 = 1.0;

/// Timing in game seconds and a prefab-local Spoon pose at projectile release.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CatapultPhysics {
    pub body: crate::recoil::Definition,
    pub spoon: [f32; 2],
    pub release: f32,
    pub impact: f32,
    pub done: f32,
}

impl CatapultPhysics {
    pub fn valid(&self) -> bool {
        self.body.valid()
            && self.spoon.iter().all(|p| p.is_finite() && p.abs() <= 64.0)
            && self.release.is_finite()
            && self.impact.is_finite()
            && self.done.is_finite()
            && 0.0 < self.release
            && self.release < self.impact
            && self.impact < self.done
            && self.done <= 10.0
    }
}

impl WorldData {
    pub fn catapult_physics(&self, proto: u16) -> Option<&CatapultPhysics> {
        self.catapult_physics
            .binary_search_by_key(&proto, |(id, _)| *id)
            .ok()
            .map(|i| &self.catapult_physics[i].1)
    }
}

/// orig: `Catapult.SetState` plays the cue upon entering Fire.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CatapultEvent {
    Fired { object: u32, x: f32 },
}

/// orig: `Catapult.State` (stored on the level object as `mode`).
pub mod mode {
    pub const MOVING: u8 = 0;
    pub const RELOADING: u8 = 1;
    pub const READY: u8 = 2;
    pub const FIRING: u8 = 3;
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Catapult {
    pub object: u32,
    pub side: i8,
    /// The workshop counter that built it. orig: `PayableWorkshop._catapult`.
    pub counter: u32,
    /// Time into the firing animation.
    pub fire_t: f32,
    released: bool,
}

/// A boulder in flight. orig: `Boulder` launched by a catapult.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Boulder {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub spin: f32,
    damage: i32,
    stuck_probability: f32,
    /// orig: `ghostTime`: passes through things at first.
    ghost: f32,
    bounces: u8,
    hit: Vec<u32>,
    /// Lodged in the ground. The float is the legacy save's age field.
    pub stuck: Option<f32>,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub runtime: BoulderRuntime,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoulderRuntime {
    /// Runtime identity for a cached projectile scanner result.
    pub id: u64,
    pub held_by: Option<u32>,
    pub hostile: bool,
    fixed_pending: f32,
    angular_velocity: f32,
    launch_side: i8,
    on_ground: bool,
    after_release: Option<f32>,
    returned_hits: Vec<BoulderTargetKind>,
    /// Time since the last admitted physical callback. None is ready.
    contact_age: Option<f32>,
}

impl Boulder {
    #[cfg(test)]
    pub(crate) fn test_lodged(x: f32) -> Self {
        Self {
            x,
            y: 0.9375 - WALK_Y,
            vx: 0.0,
            vy: 0.0,
            spin: 0.0,
            damage: 15,
            stuck_probability: 1.0,
            ghost: 0.0,
            bounces: 1,
            hit: Vec::new(),
            stuck: Some(0.0),
            runtime: BoulderRuntime::default(),
        }
    }

    /// The PC scanner checks layer Projectiles before testing the object's state.
    pub fn scan_eligible(&self) -> bool {
        self.ghost <= 0.0
    }

    /// `Boss.Throw` parents the lodged boulder to its hand and calls SetFake.
    pub fn hold_by(&mut self, boss: u32) -> bool {
        if self.stuck.is_none() || self.runtime.held_by.is_some() || !self.scan_eligible() {
            return false;
        }
        self.stuck = None;
        self.vx = 0.0;
        self.vy = 0.0;
        self.runtime.angular_velocity = 0.0;
        self.runtime.on_ground = false;
        self.runtime.held_by = Some(boss);
        true
    }

    /// `OnAnimBoulderRelease`: an unnormalized (side, 1) times throwForce.Random.
    /// The previous SetFake zeroed velocity, so mass-scaled impulse sets velocity.
    pub fn release_from_boss(
        &mut self,
        at: [f32; 2],
        side: i8,
        speed: f32,
        after_release: f32,
        definition: &crate::data::BossThrowDef,
        rng: &mut Rng,
    ) {
        debug_assert!(self.runtime.held_by.is_some());
        self.x = at[0];
        self.y = at[1];
        self.vx = side as f32 * speed;
        self.vy = speed;
        self.damage = definition.boulder_damage;
        self.ghost = definition.boulder_ghost;
        self.bounces = 0;
        self.hit.clear();
        self.runtime = BoulderRuntime {
            id: self.runtime.id,
            hostile: true,
            // World-space rotation. The scene converts it to screen-space.
            angular_velocity: -side as f32 * 180.0 * (rng.value() + 1.0),
            launch_side: side,
            after_release: Some(after_release),
            contact_age: self.runtime.contact_age,
            ..Default::default()
        };
    }

    fn age_contact(&mut self, dt: f32) {
        self.runtime.contact_age = self
            .runtime
            .contact_age
            .map(|age| age + dt)
            .filter(|&age| age <= 0.1);
    }

    fn admit_contact(&mut self) -> bool {
        if self.runtime.contact_age.is_some() {
            return false;
        }
        self.runtime.contact_age = Some(0.0);
        true
    }

    fn effect_side(&self) -> i8 {
        if self.vx > 0.0 {
            1
        } else {
            -1
        }
    }

    fn impact(&self, y: f32, side: i8, collision_sound: bool) -> BoulderHit {
        BoulderHit::Impact {
            x: self.x,
            y,
            shake_y: self.y,
            side,
            collision_sound,
        }
    }

    fn burst(&self, side: i8, impact: bool) -> BoulderHit {
        BoulderHit::Burst {
            x: self.x,
            y: self.y,
            side,
            impact,
        }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Siege {
    pub catapults: Vec<Catapult>,
    pub boulders: Vec<Boulder>,
    #[cfg_attr(feature = "serde", serde(skip))]
    recoils: Vec<crate::recoil::State>,
    #[cfg_attr(feature = "serde", serde(skip))]
    enhanced_clocks: Vec<u32>,
    #[cfg_attr(feature = "serde", serde(skip))]
    next_boulder_id: u64,
}

#[cfg(feature = "save")]
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct BoulderSave {
    next_id: u64,
    projectiles: Vec<BoulderRuntime>,
}

/// Distinct collision objects count against the returned boulder's hit limit.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoulderTargetKind {
    Monarch,
    Unit(u32),
    Object(u32),
    Enemy(u32),
    Wildlife(u32),
}

#[derive(Clone, Copy, Debug)]
pub struct BoulderTarget {
    pub target: BoulderTargetKind,
    pub rect: Rect,
    pub physical: bool,
    pub takes_damage: bool,
    pub huge: bool,
}

pub struct BoulderWall {
    pub object: u32,
    /// Ground-relative, world-space vertices of the authored wall collider.
    pub polygon: Vec<[f32; 2]>,
}

#[derive(Clone, Copy, Debug)]
pub struct BoulderEnemy {
    pub id: u32,
    pub x: f32,
    /// Huge damage trigger. Its admission shatters even a friendly boulder.
    pub huge: Option<Rect>,
}

/// Something a boulder hit, or a returned boulder breaking.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BoulderHit {
    Troll(u32, i32),
    Returned {
        target: BoulderTargetKind,
        damage: i32,
        direction: f32,
    },
    Impact {
        x: f32,
        y: f32,
        shake_y: f32,
        side: i8,
        collision_sound: bool,
    },
    Burst {
        x: f32,
        y: f32,
        side: i8,
        impact: bool,
    },
}

impl Siege {
    pub(crate) fn identify_boulders(&mut self) {
        for b in &mut self.boulders {
            if b.runtime.id == 0 {
                self.next_boulder_id += 1;
                b.runtime.id = self.next_boulder_id;
            }
        }
    }

    #[cfg(feature = "save")]
    pub(crate) fn save_boulders(&self) -> Option<BoulderSave> {
        let default = BoulderRuntime::default();
        (self.next_boulder_id != 0 || self.boulders.iter().any(|b| b.runtime != default)).then(
            || BoulderSave {
                next_id: self.next_boulder_id,
                projectiles: self.boulders.iter().map(|b| b.runtime.clone()).collect(),
            },
        )
    }

    #[cfg(feature = "save")]
    pub(crate) fn restore_boulders(&mut self, saved: BoulderSave) -> bool {
        if saved.next_id == u64::MAX || self.boulders.len() != saved.projectiles.len() {
            return false;
        }
        let valid = saved
            .projectiles
            .iter()
            .zip(&self.boulders)
            .enumerate()
            .all(|(i, (r, b))| {
                r.id <= saved.next_id
                    && (r.id == 0 || !saved.projectiles[..i].iter().any(|old| old.id == r.id))
                    && [b.x, b.y, b.vx, b.vy, b.spin, b.ghost]
                        .into_iter()
                        .all(f32::is_finite)
                    && (0.0..0.1).contains(&r.fixed_pending)
                    && r.angular_velocity.abs() <= 360.0
                    && r.contact_age.is_none_or(|t| (0.0..=0.1).contains(&t))
                    && r.after_release
                        .is_none_or(|t| r.hostile && (0.0..=60.0).contains(&t))
                    && r.returned_hits.len() <= u8::MAX as usize
                    && r.returned_hits
                        .iter()
                        .enumerate()
                        .all(|(i, k)| !r.returned_hits[..i].contains(k))
                    && (r.held_by.is_none()
                        || (!r.hostile
                            && b.stuck.is_none()
                            && b.vx == 0.0
                            && b.vy == 0.0
                            && !saved.projectiles[..i]
                                .iter()
                                .any(|old| old.held_by == r.held_by)))
                    && if r.hostile {
                        matches!(r.launch_side, -1 | 1)
                            && b.stuck.is_none()
                            && b.damage > 0
                            && b.ghost >= 0.0
                    } else {
                        r.launch_side == 0 && r.returned_hits.is_empty()
                    }
            });
        if !valid {
            return false;
        }
        self.next_boulder_id = saved.next_id;
        for (boulder, runtime) in self.boulders.iter_mut().zip(saved.projectiles) {
            boulder.runtime = runtime;
        }
        true
    }

    #[cfg(feature = "save")]
    pub(crate) fn max_boulder_id(&self) -> u64 {
        self.next_boulder_id
    }

    #[cfg(feature = "save")]
    pub(crate) fn discard_throwback(&mut self) {
        self.boulders
            .retain(|b| b.runtime.held_by.is_none() && !b.runtime.hostile);
        for b in &mut self.boulders {
            b.runtime = BoulderRuntime::default();
        }
        self.next_boulder_id = 0;
    }

    pub fn recoil_pose(
        &self,
        object: u32,
        definition: &crate::recoil::Definition,
    ) -> crate::animation::Affine2 {
        self.recoils
            .iter()
            .find(|r| r.object == object)
            .map_or(crate::animation::Affine2::IDENTITY, |r| r.pose(definition))
    }

    pub fn wheel_pose(
        &self,
        object: u32,
        wheel: usize,
        definition: &crate::recoil::Definition,
    ) -> crate::animation::Affine2 {
        self.recoils
            .iter()
            .find(|r| r.object == object)
            .map_or(crate::animation::Affine2::IDENTITY, |r| {
                r.wheel_pose(wheel, definition)
            })
    }

    /// Remove the body and all its physics immediately on same-tick death.
    pub fn remove(&mut self, object: u32) {
        self.catapults.retain(|catapult| catapult.object != object);
        self.recoils.retain(|state| state.object != object);
        self.enhanced_clocks.retain(|&id| id != object);
    }

    #[cfg(feature = "save")]
    pub(crate) fn save_recoil(&self) -> Option<(&[crate::recoil::State], &[u32])> {
        (!self.recoils.is_empty() || !self.enhanced_clocks.is_empty())
            .then_some((&self.recoils, &self.enhanced_clocks))
    }

    #[cfg(feature = "save")]
    pub(crate) fn restore_recoil(
        &mut self,
        states: Vec<crate::recoil::State>,
        enhanced_clocks: Vec<u32>,
        world: &WorldData,
        level: &Level,
    ) -> bool {
        let catapult = |id| {
            let cat = self.catapults.iter().find(|c| c.object == id)?;
            let o = &level.objects[level.index_of(id)?];
            (o.mode <= mode::FIRING && cat.fire_t.is_finite() && cat.fire_t >= 0.0)
                .then_some((cat, o))
        };
        let valid_clocks = enhanced_clocks.iter().enumerate().all(|(i, &id)| {
            !enhanced_clocks[..i].contains(&id)
                && catapult(id).is_some_and(|(cat, o)| {
                    o.mode != mode::FIRING
                        || cat.fire_t <= world.catapult_physics(o.proto).map_or(10.0, |p| p.done)
                })
        });
        let valid_states = states.iter().enumerate().all(|(i, r)| {
            !states[..i].iter().any(|p| p.object == r.object)
                && enhanced_clocks.contains(&r.object)
                && catapult(r.object).is_some_and(|(cat, o)| {
                    r.side == cat.side
                        && r.sane()
                        && world.catapult_physics(o.proto).is_none_or(|p| {
                            r.valid(&p.body)
                                && (r.ground_y + o.y - p.body.ground_height).abs() < 0.001
                        })
                })
        });
        if valid_clocks && valid_states {
            self.recoils = states;
            self.enhanced_clocks = enhanced_clocks;
            true
        } else {
            false
        }
    }

    /// orig: the Workshop's `PayableWorkshop` children: one counter per side.
    pub fn place_counters(world: &WorldData, level: &mut Level, workshop: u32) {
        let Some(i) = level.index_of(workshop) else {
            return;
        };
        let o = &level.objects[i];
        let (layer, x, y, z) = (o.layer, o.x, o.y, o.z);
        let counters = world.protos[o.proto as usize].workshop.clone();
        for (proto, dx) in counters {
            let c = level::instantiate(world, proto, layer, x + dx, y, z, 1.0);
            level.add(c);
        }
    }

    /// orig: `PayableWorkshop.CanPay()`: no catapult of its own yet.
    pub fn counter_can_pay(&self, counter: u32) -> bool {
        !self.catapults.iter().any(|c| c.counter == counter)
    }

    /// orig: `PayableWorkshop.Pay()`: a catapult (to be built) beside it.
    pub fn order(&mut self, world: &WorldData, level: &mut Level, counter: u32) {
        let Some(i) = level.index_of(counter) else {
            return;
        };
        let o = &level.objects[i];
        let Some((side, proto)) = world.protos[o.proto as usize].workshop_side else {
            return;
        };
        let p = &world.protos[proto as usize];
        let z = p.fixed.and_then(|f| f.fix_z).unwrap_or(p.root_z);
        let x = o.x + side as f32 * 1.2;
        let mut c = level::instantiate(world, proto, o.layer, x, 1.0, z, side as f32);
        if p.building.as_ref().is_some_and(|b| b.build_points > 0) {
            c.build = Some(0.0);
        }
        c.mode = mode::MOVING;
        let object = level.add(c);
        self.catapults.push(Catapult {
            object,
            side,
            counter,
            fire_t: 0.0,
            released: false,
        });
    }

    /// orig: `Catapult.OnWorkFinish()` while reloading: a turn of the crank.
    pub fn crank(world: &WorldData, level: &mut Level, object: u32) {
        let Some(i) = level.index_of(object) else {
            return;
        };
        let o = &mut level.objects[i];
        let Some(d) = world.protos[o.proto as usize].catapult else {
            return;
        };
        if o.mode == mode::RELOADING {
            o.aim += d.crank_rate * ARM_MAX;
        }
    }

    /// `pushers` counts the builders at each catapult. Returns boulder hits.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        dt: f32,
        world: &WorldData,
        level: &mut Level,
        borders: Borders,
        pushers: &[(u32, usize)],
        enemies: &[BoulderEnemy],
        rng: &mut Rng,
        mut events: impl FnMut(CatapultEvent),
    ) -> Vec<BoulderHit> {
        let existing_boulders = self.boulders.len();
        let mut new_flight_times = Vec::new();
        self.catapults
            .retain(|c| level.index_of(c.object).is_some());
        self.recoils.retain(|r| {
            level
                .index_of(r.object)
                .is_some_and(|i| world.catapult_physics(level.objects[i].proto).is_some())
        });
        self.enhanced_clocks
            .retain(|id| self.catapults.iter().any(|catapult| catapult.object == *id));
        for c in &mut self.catapults {
            let Some(i) = level.index_of(c.object) else {
                continue;
            };
            let o = &level.objects[i];
            let Some(d) = world.protos[o.proto as usize].catapult else {
                continue;
            };
            if under_construction(world, o) {
                continue;
            }
            let authored = world.catapult_physics(o.proto);
            let physics = authored.map(|p| &p.body);
            let mut recoil = physics.map(|physics| {
                let ri = match self.recoils.iter().position(|r| r.object == c.object) {
                    Some(i) => i,
                    None => {
                        let mut state = crate::recoil::State::new(c.object, c.side, physics);
                        state.ground_y = physics.ground_height - o.y;
                        self.recoils.push(state);
                        self.recoils.len() - 1
                    }
                };
                &mut self.recoils[ri]
            });
            // The base save stores the old elapsed firing clock. When switching
            // packs, map it across the release/completion boundaries exactly once.
            let was_enhanced = self.enhanced_clocks.contains(&c.object);
            if authored.is_some() != was_enhanced {
                if o.mode == mode::FIRING {
                    c.fire_t = if let Some(p) = authored {
                        migrate_fire(c, RELEASE_AT, FIRE_TIME, p.release, p.done)
                    } else {
                        // A previous enhanced clock already consumed release if
                        // its flag is set. Restart only the unconsumed phase.
                        if c.released {
                            RELEASE_AT
                        } else {
                            0.0
                        }
                    };
                }
                if authored.is_some() {
                    self.enhanced_clocks.push(c.object);
                } else {
                    self.enhanced_clocks.retain(|&id| id != c.object);
                }
            }
            let mut advanced = 0.0;
            let s = c.side as f32;
            // orig: IsInPosition: within 2.5 of its side's border.
            let in_position = (o.x - (borders.side(c.side) - s * BORDER_DISTANCE)) * s > 0.0;
            let workers = pushers.iter().find(|p| p.0 == c.object).map_or(0, |p| p.1);
            let o = &mut level.objects[i];
            match o.mode {
                mode::MOVING => {
                    if in_position {
                        o.mode = mode::RELOADING;
                        if let Some(r) = recoil.as_mut() {
                            r.stop();
                        }
                    } else {
                        // PC Update sets chassis velocity; wheel contacts and
                        // hinges then determine its actual displacement.
                        let speed = s * PUSH_SPEED * workers as f32;
                        if let Some(r) = recoil.as_mut() {
                            r.drive(speed);
                        } else {
                            o.x += speed * dt;
                        }
                    }
                }
                mode::RELOADING => {
                    if o.aim > ARM_MAX {
                        o.mode = mode::READY;
                    }
                }
                mode::READY => {
                    // orig: the scanner sees scanRange.max ahead and nothing nearer than min.
                    let target = enemies.iter().any(|&BoulderEnemy { x: ex, .. }| {
                        let dd = (ex - o.x) * s;
                        dd >= d.scan.0 && dd <= d.scan.1
                    });
                    if target && (in_position || authored.is_none()) {
                        o.mode = mode::FIRING;
                        c.fire_t = 0.0;
                        c.released = false;
                        if authored.is_some() {
                            events(CatapultEvent::Fired {
                                object: c.object,
                                x: o.x,
                            });
                        }
                    }
                    // PC checks this even when Fire() was called above. A
                    // moving border can cancel firing before its first marker.
                    if !in_position && (authored.is_some() || !target) {
                        o.mode = mode::MOVING;
                        o.aim = 0.0;
                    }
                }
                _ => {
                    let before = c.fire_t;
                    c.fire_t += dt;
                    let release = authored.map_or(RELEASE_AT, |p| p.release);
                    let crossed = |at| before < at && c.fire_t >= at;
                    let marker_dt = |at: f32| (at - before).clamp(0.0, dt);
                    if !c.released && (crossed(release) || before >= release) {
                        let until = marker_dt(release);
                        advance_recoil(&mut recoil, physics, until - advanced);
                        advanced = until;
                        c.released = true;
                        // orig: ReleaseProjectileFromAnimation: along (dir, 1.5) at projectileForce.
                        let speed = rng.range(d.force.0, d.force.1);
                        let len = libm::sqrtf(1.0 + 1.5 * 1.5);
                        let spoon = authored.map_or([d.spoon.0, d.spoon.1], |p| p.spoon);
                        let spoon = recoil
                            .as_ref()
                            .zip(physics)
                            .map_or([spoon[0] * s, spoon[1]], |(r, p)| {
                                r.pose(p).point([spoon[0] * s, spoon[1]])
                            });
                        self.boulders.push(Boulder {
                            x: o.x + spoon[0],
                            y: o.y + spoon[1] - WALK_Y,
                            vx: s / len * speed,
                            vy: 1.5 / len * speed,
                            spin: 0.0,
                            damage: d.boulder_damage,
                            stuck_probability: d.stuck_probability,
                            ghost: 0.3,
                            bounces: 0,
                            hit: Vec::new(),
                            stuck: None,
                            runtime: BoulderRuntime::default(),
                        });
                        new_flight_times.push(if authored.is_some() { dt - until } else { dt });
                    }
                    if let Some(p) = authored.filter(|p| crossed(p.impact)) {
                        let until = marker_dt(p.impact);
                        advance_recoil(&mut recoil, physics, until - advanced);
                        advanced = until;
                        if let Some(r) = recoil.as_mut() {
                            r.impact(&p.body);
                        }
                    }
                    let done = authored.map_or(FIRE_TIME, |p| p.done);
                    if c.fire_t >= done {
                        let until = marker_dt(done);
                        advance_recoil(&mut recoil, physics, until - advanced);
                        advanced = until;
                        // orig: FiringDoneFromAnimation.
                        o.aim = 0.0;
                        let physical_x = o.x
                            + recoil
                                .as_ref()
                                .zip(physics)
                                .map_or(0.0, |(r, d)| r.pose(d).translation[0]);
                        let in_position =
                            (physical_x - (borders.side(c.side) - s * BORDER_DISTANCE)) * s > 0.0;
                        o.mode = if in_position {
                            if let Some(r) = recoil.as_mut() {
                                r.stop();
                            }
                            mode::RELOADING
                        } else {
                            mode::MOVING
                        };
                    }
                }
            }
            advance_recoil(&mut recoil, physics, dt - advanced);
            if let (Some(r), Some(d)) = (recoil.as_mut(), physics) {
                // Gameplay targeting, workers, saves and rendering all use the
                // physical root. Keep solver coordinates local for ARM precision.
                let shift = r.pose(d).translation;
                o.x += shift[0];
                o.y += shift[1];
                r.shift_origin(shift);
            }
        }
        if let Some(definition) = &world.boss_throw {
            self.update_boulders(
                dt,
                existing_boulders,
                &new_flight_times,
                enemies,
                definition,
                rng,
            )
        } else {
            self.update_legacy_boulders(dt, existing_boulders, &new_flight_times, enemies, rng)
        }
    }

    /// orig: `Boulder.HitObject()`: a friendly boulder hurts every greed it
    /// touches; on the ground it bounces once, then sticks or bursts.
    fn update_legacy_boulders(
        &mut self,
        dt: f32,
        existing: usize,
        new_flight_times: &[f32],
        enemies: &[BoulderEnemy],
        rng: &mut Rng,
    ) -> Vec<BoulderHit> {
        let g = crate::coins::GRAVITY;
        let mut hits = Vec::new();
        let mut burst = Vec::new();
        for (i, b) in self.boulders.iter_mut().enumerate() {
            let dt = if i < existing {
                dt
            } else {
                new_flight_times[i - existing]
            };
            if b.runtime.held_by.is_some() || b.runtime.hostile {
                continue;
            }
            if let Some(t) = &mut b.stuck {
                *t += dt;
                if *t > 10.0 {
                    burst.push(i);
                }
                continue;
            }
            b.ghost -= dt;
            b.vy -= g * dt;
            b.x += b.vx * dt;
            b.y += b.vy * dt;
            b.spin -= b.vx.signum() * 360.0 * dt;
            if b.ghost <= 0.0 {
                for &BoulderEnemy { id, x, .. } in enemies {
                    if (x - b.x).abs() < 0.4 && b.y < 0.8 && !b.hit.contains(&id) {
                        b.hit.push(id);
                        hits.push(BoulderHit::Troll(id, b.damage));
                    }
                }
            }
            if b.y <= 0.0 {
                b.y = 0.0;
                if b.bounces < 1 {
                    b.bounces += 1;
                    let k = rng.range(0.7, 1.0);
                    b.vx *= k;
                    b.vy = -b.vy * 0.5 * k;
                } else if rng.value() < b.stuck_probability {
                    b.stuck = Some(0.0);
                    b.vx = 0.0;
                    b.vy = 0.0;
                } else {
                    burst.push(i);
                }
            }
        }
        for i in burst.into_iter().rev() {
            self.boulders.remove(i);
        }
        hits
    }
    /// orig: `Boulder.HitObject()`: a friendly boulder hurts every greed it
    /// touches; on the ground it bounces once, then sticks or bursts.
    fn update_boulders(
        &mut self,
        dt: f32,
        existing: usize,
        new_flight_times: &[f32],
        enemies: &[BoulderEnemy],
        def: &crate::data::BossThrowDef,
        rng: &mut Rng,
    ) -> Vec<BoulderHit> {
        let g = crate::coins::GRAVITY;
        let radius = def.boulder_radius;
        let floor = def.ground_y - WALK_Y + radius;
        let mut hits = Vec::new();
        let mut gone = Vec::new();
        for (i, b) in self.boulders.iter_mut().enumerate() {
            // A new payload only receives time after its release marker.
            let dt = if i < existing {
                dt
            } else {
                new_flight_times[i - existing]
            };
            if dt <= 0.0 || b.runtime.hostile {
                continue;
            }
            b.age_contact(dt);
            if b.runtime.held_by.is_some() {
                continue;
            }
            if b.stuck.is_some() {
                // Old snapshots could lodge a boulder before its grace ended.
                b.ghost = (b.ghost - dt).max(0.0);
                continue;
            }
            let ghost = b.ghost > 0.0;
            b.ghost -= dt;
            b.vy -= g * dt;
            b.x += b.vx * dt;
            b.y += b.vy * dt;
            b.spin -= b.vx.signum() * 360.0 * dt;
            let side = b.effect_side();
            let entered_ground = b.y <= floor && !b.runtime.on_ground;
            b.runtime.on_ground = b.y <= floor;
            let mut physical_burst = false;
            if b.runtime.on_ground {
                b.y = floor;
                if !ghost && entered_ground && b.admit_contact() {
                    if b.bounces < 1 {
                        b.bounces += 1;
                        let k = rng.range(0.7, 1.0);
                        b.vx *= k;
                        b.vy = -b.vy * 0.5 * k;
                    } else if rng.value() < b.stuck_probability {
                        b.stuck = Some(0.0);
                        b.y = 0.9375 - WALK_Y;
                        b.vx = 0.0;
                        b.vy = 0.0;
                        b.runtime.on_ground = false;
                    } else {
                        physical_burst = true;
                    }
                    hits.push(b.impact(floor, side, !physical_burst));
                } else {
                    b.vy = -b.vy * 0.5;
                }
            }
            let mut burst = physical_burst;
            if !ghost && b.stuck.is_none() && !physical_burst {
                for enemy in enemies {
                    let overlaps = enemy.huge.map_or_else(
                        || (enemy.x - b.x).abs() < 0.4 && b.y < 0.8,
                        |rect| circle_hits_rect([b.x, b.y], radius, rect),
                    );
                    if overlaps && !b.hit.contains(&enemy.id) {
                        b.hit.push(enemy.id);
                        hits.push(BoulderHit::Troll(enemy.id, b.damage));
                        if enemy.huge.is_some() {
                            burst = true;
                            break;
                        }
                    }
                }
            }
            if burst {
                hits.push(b.burst(side, physical_burst));
                gone.push(i);
            }
        }
        for i in gone.into_iter().rev() {
            self.boulders.remove(i);
        }
        hits
    }

    /// Advance enemy boulders after the boss animation markers. Each new shot
    /// receives only the remainder after its release marker.
    pub fn update_returned_boulders(
        &mut self,
        dt: f32,
        d: &crate::data::BossThrowDef,
        targets: &[BoulderTarget],
        walls: &[BoulderWall],
    ) -> Vec<BoulderHit> {
        let mut hits = Vec::new();
        if !(0.0..=60.0).contains(&dt) || !d.valid() {
            return hits;
        }
        let step = d.fixed_step;
        let radius = d.boulder_radius;
        let floor = d.ground_y - WALK_Y + radius;
        self.boulders.retain_mut(|b| {
            if !b.runtime.hostile {
                return true;
            }
            let time = b.runtime.after_release.take().unwrap_or(dt).min(dt);
            let total = time + b.runtime.fixed_pending;
            let steps = libm::floorf(total / step) as u32;
            b.runtime.fixed_pending =
                (total - steps as f32 * step).clamp(0.0, step - f32::EPSILON * step);
            for _ in 0..steps {
                // Ghost switches layer, not body type. Ground contacts still
                // bounce, but HitObject has no effect until the grace ends.
                b.age_contact(step);
                let ghost = b.ghost > 0.0;
                b.ghost = if b.ghost <= step + f32::EPSILON {
                    0.0
                } else {
                    b.ghost - step
                };
                let damping = 1.0 / (1.0 + step * d.boulder_linear_drag);
                b.vx *= damping;
                b.vy = (b.vy - d.gravity * step) * damping;
                b.runtime.angular_velocity /= 1.0 + step * d.boulder_angular_drag;
                b.x += b.vx * step;
                b.y += b.vy * step;
                b.spin = (b.spin - b.runtime.angular_velocity * step) % 360.0;
                let entered_ground = b.y <= floor && !b.runtime.on_ground;
                b.runtime.on_ground = b.y <= floor;
                if b.runtime.on_ground {
                    b.y = floor;
                    bounce_boulder_ground(b, radius, d.boulder_restitution, d.boulder_friction);
                }
                if ghost {
                    continue;
                }
                let direction = if b.vx.abs() > 0.2 {
                    b.vx.signum()
                } else {
                    b.runtime.launch_side as f32
                };
                let side = b.effect_side();
                let mut impact = entered_ground && b.admit_contact();
                if impact {
                    hits.push(b.impact(b.y, side, false));
                }
                let mut burst = impact;
                for target in targets {
                    if !circle_hits_rect([b.x, b.y], radius, target.rect) {
                        continue;
                    }
                    let physical = target.physical && b.admit_contact();
                    if physical {
                        hits.push(b.impact(b.y, side, false));
                    }
                    let admitted = record_returned_hit(
                        b,
                        target.target,
                        target.takes_damage,
                        d.boulder_max_hits,
                        direction,
                        &mut hits,
                    );
                    if physical || (admitted && target.huge) {
                        burst = true;
                        impact |= physical;
                        break;
                    }
                }
                if !burst {
                    if let Some(wall) = walls
                        .iter()
                        .find(|w| circle_hits_polygon([b.x, b.y], radius, &w.polygon))
                    {
                        impact = b.admit_contact();
                        if impact {
                            hits.push(b.impact(b.y, side, false));
                        }
                        record_returned_hit(
                            b,
                            BoulderTargetKind::Object(wall.object),
                            true,
                            d.boulder_max_hits,
                            direction,
                            &mut hits,
                        );
                        burst = impact;
                    }
                }
                if burst {
                    hits.push(b.burst(side, impact));
                    return false;
                }
            }
            true
        });
        hits
    }
}

fn record_returned_hit(
    boulder: &mut Boulder,
    target: BoulderTargetKind,
    takes_damage: bool,
    max_hits: u8,
    direction: f32,
    hits: &mut Vec<BoulderHit>,
) -> bool {
    let seen = &mut boulder.runtime.returned_hits;
    if seen.len() >= max_hits as usize || seen.contains(&target) {
        return false;
    }
    // The PC HashSet counts the collider object even if its damage mask
    // rejects the hit. A Huge surface still shatters an immune boulder.
    seen.push(target);
    if takes_damage {
        hits.push(BoulderHit::Returned {
            target,
            damage: boulder.damage,
            direction,
        });
    }
    true
}

fn circle_hits_rect([x, y]: [f32; 2], radius: f32, rect: Rect) -> bool {
    let dx = x - x.clamp(rect.x, rect.x + rect.w);
    let dy = y - y.clamp(rect.y, rect.y + rect.h);
    dx * dx + dy * dy <= radius * radius
}

fn circle_hits_polygon([x, y]: [f32; 2], radius: f32, points: &[[f32; 2]]) -> bool {
    let mut inside = false;
    for i in 0..points.len() {
        let [ax, ay] = points[i];
        let [bx, by] = points[(i + 1) % points.len()];
        let dx = bx - ax;
        let dy = by - ay;
        let length_sq = dx * dx + dy * dy;
        if length_sq > 0.0 {
            let t = (((x - ax) * dx + (y - ay) * dy) / length_sq).clamp(0.0, 1.0);
            let sx = x - ax - dx * t;
            let sy = y - ay - dy * t;
            if sx * sx + sy * sy <= radius * radius {
                return true;
            }
        }
        if (ay > y) != (by > y) && x < ax + (y - ay) * dx / dy {
            inside = !inside;
        }
    }
    inside
}

/// One circular body against a horizontal plane. Normal restitution uses the
/// shipped Physics2D velocity threshold. Coulomb friction uses Box2D's material
/// mix and the disk inertia (I/m = r²/2); this resolves rolling during ghosting.
fn bounce_boulder_ground(b: &mut Boulder, radius: f32, restitution: f32, friction: f32) {
    if b.vy >= 0.0 {
        return;
    }
    let incoming = -b.vy;
    b.vy = if incoming > 1.0 {
        incoming * restitution
    } else {
        0.0
    };
    let cap = libm::sqrtf(friction * 0.4) * (incoming + b.vy);
    let omega = b.runtime.angular_velocity.to_radians();
    let impulse = (-(b.vx + radius * omega) / 3.0).clamp(-cap, cap);
    b.vx += impulse;
    b.runtime.angular_velocity = (omega + 2.0 * impulse / radius)
        .to_degrees()
        .clamp(-360.0, 360.0);
}

fn advance_recoil(
    state: &mut Option<&mut crate::recoil::State>,
    definition: Option<&crate::recoil::Definition>,
    dt: f32,
) {
    if let (Some(state), Some(definition)) = (state.as_mut(), definition) {
        state.advance(dt, definition);
    }
}

fn migrate_fire(c: &Catapult, from_release: f32, from_done: f32, release: f32, done: f32) -> f32 {
    if c.released {
        let fraction = ((c.fire_t - from_release) / (from_done - from_release)).clamp(0.0, 1.0);
        release + fraction * (done - release)
    } else {
        let fraction = (c.fire_t / from_release).clamp(0.0, 1.0);
        // An unreleased shot is still before the marker even in a malformed
        // older snapshot whose clock reached the boundary.
        (fraction * release).min(release * (1.0 - f32::EPSILON))
    }
}

#[cfg(test)]
mod boulder_tests {
    use super::*;

    fn returned(
        definition: &crate::data::BossThrowDef,
        side: i8,
        at: [f32; 2],
        remainder: f32,
    ) -> Siege {
        let mut siege = Siege::default();
        siege.boulders.push(Boulder::test_lodged(0.0));
        siege.identify_boulders();
        let b = &mut siege.boulders[0];
        assert!(b.hold_by(7));
        b.release_from_boss(at, side, 5.0, remainder, definition, &mut Rng::new(9));
        siege
    }

    fn trigger(target: BoulderTargetKind, takes_damage: bool, huge: bool) -> BoulderTarget {
        BoulderTarget {
            target,
            rect: Rect {
                x: -3.0,
                y: 0.0,
                w: 6.0,
                h: 3.0,
            },
            physical: false,
            takes_damage,
            huge,
        }
    }

    #[test]
    fn lodged_boulders_keep_authored_height_and_remain_catchable() {
        let mut siege = Siege::default();
        let mut b = Boulder::test_lodged(2.0);
        b.stuck = None;
        b.y = 0.01;
        b.vy = -1.0;
        siege.boulders.push(b);
        let mut rng = Rng::new(2);
        siege.update_boulders(0.1, 1, &[], &[], &Default::default(), &mut rng);
        assert_eq!(siege.boulders[0].stuck, Some(0.0));
        assert_eq!(siege.boulders[0].y, 0.9375 - WALK_Y);
        siege.update_boulders(20.0, 1, &[], &[], &Default::default(), &mut rng);
        assert!(siege.boulders[0].hold_by(8));
        assert!(
            siege.boulders[0].scan_eligible(),
            "held object remains on Projectiles"
        );
        assert!(!siege.boulders[0].hold_by(9));
        siege.update_boulders(20.0, 1, &[], &[], &Default::default(), &mut rng);
        assert_eq!(siege.boulders[0].runtime.held_by, Some(8));
        assert_eq!(siege.boulders[0].y, 0.9375 - WALK_Y);
    }

    #[test]
    fn return_uses_unnormalized_velocity_and_only_post_marker_fixed_time() {
        let d = &crate::data::BossThrowDef::default();
        for side in [-1, 1] {
            let mut s = returned(d, side, [2.0, 1.0], 0.005);
            let b = &s.boulders[0];
            assert_eq!([b.vx, b.vy], [side as f32 * 5.0, 5.0]);
            assert_eq!(b.runtime.id, 1, "relaunch reuses the same boulder");
            assert_eq!(b.runtime.held_by, None);
            assert!(b.runtime.hostile);
            assert!(!b.scan_eligible());
            let omega = -side as f32 * 180.0 * (Rng::new(9).value() + 1.0);
            assert_eq!(b.runtime.angular_velocity, omega);
            s.update_returned_boulders(0.1, d, &[], &[]);
            assert_eq!(s.boulders[0].x, 2.0);
            assert_eq!(s.boulders[0].ghost, d.boulder_ghost);
            s.update_returned_boulders(0.016, d, &[], &[]);
            let b = &s.boulders[0];
            let vx = side as f32 * 5.0 / (1.0 + d.fixed_step * d.boulder_linear_drag);
            let vy =
                (5.0 - d.gravity * d.fixed_step) / (1.0 + d.fixed_step * d.boulder_linear_drag);
            assert!((b.x - (2.0 + vx * d.fixed_step)).abs() < 1e-6);
            assert!((b.y - (1.0 + vy * d.fixed_step)).abs() < 1e-6);
            assert!(
                (b.spin + omega * d.fixed_step / (1.0 + d.fixed_step * d.boulder_angular_drag))
                    .abs()
                    < 1e-5
            );
        }
    }

    #[test]
    fn returned_boulder_ghosts_and_deduplicates_collision_objects() {
        let d = &crate::data::BossThrowDef::default();
        let mut s = returned(d, 1, [0.0, 1.0], 0.0);
        let b = &mut s.boulders[0];
        b.vx = 0.0;
        b.vy = 0.0;
        let targets = [trigger(BoulderTargetKind::Unit(2), true, false)];
        assert!(s
            .update_returned_boulders(0.02, d, &targets, &[])
            .is_empty());
        assert!(s.update_returned_boulders(0.1, d, &targets, &[]).is_empty());
        assert!(s.boulders[0].runtime.returned_hits.is_empty());
        s.boulders[0].ghost = 0.0;
        assert!(matches!(
            s.update_returned_boulders(0.02, d, &targets, &[])
                .as_slice(),
            [BoulderHit::Returned {
                target: BoulderTargetKind::Unit(2),
                damage: 15,
                ..
            }]
        ));
        assert!(s
            .update_returned_boulders(0.02, d, &targets, &[])
            .is_empty());
    }
}

#[cfg(test)]
mod animation_tests {
    use super::*;
    use crate::data::{CatapultDef, Layer, Proto};
    use alloc::vec;

    fn fixture() -> (WorldData, Level, Siege, Rng) {
        let world = WorldData {
            protos: vec![Proto {
                catapult: Some(CatapultDef {
                    crank_rate: 0.1,
                    force: (7.0, 7.0),
                    scan: (2.0, 8.0),
                    hit_points: 4,
                    boulder_damage: 15,
                    stuck_probability: 0.5,
                    spoon: (99.0, 99.0),
                }),
                ..Default::default()
            }],
            catapult_physics: vec![(
                0,
                CatapultPhysics {
                    body: crate::recoil::fixture(),
                    spoon: [2.0, 1.0],
                    release: 0.15,
                    impact: 0.16666667,
                    done: 1.1666666,
                },
            )],
            ..Default::default()
        };
        let mut level = Level::empty(-20.0, 40.0);
        let object = level.add(level::instantiate(
            &world,
            0,
            Layer::Game,
            8.0,
            1.0,
            0.0,
            1.0,
        ));
        level.objects[0].mode = mode::READY;
        let siege = Siege {
            catapults: vec![Catapult {
                object,
                side: 1,
                counter: 0,
                fire_t: 0.0,
                released: false,
            }],
            ..Default::default()
        };
        (world, level, siege, Rng::new(80))
    }

    fn advance(
        s: &mut Siege,
        w: &WorldData,
        l: &mut Level,
        rng: &mut Rng,
        dt: f32,
    ) -> Vec<CatapultEvent> {
        let mut events = Vec::new();
        s.update(
            dt,
            w,
            l,
            Borders {
                left: -10.0,
                right: 10.0,
            },
            &[],
            &[BoulderEnemy {
                id: 100,
                x: 12.0,
                huge: None,
            }],
            rng,
            |e| events.push(e),
        );
        events
    }

    #[test]
    fn catapult_releases_from_live_spoon_and_only_advances_flight_after_marker() {
        let (world, mut level, mut siege, mut rng) = fixture();
        let def = world.catapult_physics[0].1;
        let id = siege.catapults[0].object;
        assert_eq!(
            advance(&mut siege, &world, &mut level, &mut rng, 0.0),
            [CatapultEvent::Fired { object: id, x: 8.0 }]
        );
        advance(&mut siege, &world, &mut level, &mut rng, 0.14);
        assert!(siege.boulders.is_empty());
        let mut expected = siege.recoils[0];
        expected.advance(0.01, &def.body);
        let spoon = expected.pose(&def.body).point(def.spoon);
        let root = (level.objects[0].x, level.objects[0].y);
        assert!(advance(&mut siege, &world, &mut level, &mut rng, 0.02).is_empty());
        let boulder = &siege.boulders[0];
        assert!((boulder.x - boulder.vx * 0.01 - root.0 - spoon[0]).abs() < 1e-5);
        assert!((boulder.y - boulder.vy * 0.01 - root.1 - spoon[1] + WALK_Y).abs() < 1e-5);
        assert!((boulder.ghost - 0.29).abs() < 1e-6);
        expected.advance(0.01, &def.body);
        assert!(
            (siege.recoils[0].bodies[0].angular_velocity - expected.bodies[0].angular_velocity)
                .abs()
                < 1e-5
        );
        expected.advance(0.04, &def.body);
        advance(&mut siege, &world, &mut level, &mut rng, 0.04);
        assert!(
            siege.recoils[0].bodies[0].angular_velocity
                < expected.bodies[0].angular_velocity - 0.01
        );
        advance(&mut siege, &world, &mut level, &mut rng, 1.0);
        assert_eq!(level.objects[0].mode, mode::RELOADING);
        assert!(siege.catapults[0].released);
    }

    #[test]
    fn catapult_interruption_cancels_pending_markers_and_removes_dead_body() {
        let (world, mut level, mut siege, mut rng) = fixture();
        advance(&mut siege, &world, &mut level, &mut rng, 0.0);
        advance(&mut siege, &world, &mut level, &mut rng, 0.1);
        level.objects[0].mode = mode::RELOADING;
        let mut expected = siege.recoils[0];
        expected.advance(0.5, &world.catapult_physics[0].1.body);
        advance(&mut siege, &world, &mut level, &mut rng, 0.5);
        assert!(siege.boulders.is_empty());
        assert!(
            (siege.recoils[0].bodies[0].angular_velocity - expected.bodies[0].angular_velocity)
                .abs()
                < 1e-5
        );
        let id = siege.catapults[0].object;
        siege.remove(id);
        assert!(siege.catapults.is_empty());
        assert!(siege.recoils.is_empty());
        assert!(siege.enhanced_clocks.is_empty());
    }

    #[test]
    fn catapult_pack_switch_does_not_replay_release() {
        let (mut world, mut level, mut siege, mut rng) = fixture();
        let enhanced = core::mem::take(&mut world.catapult_physics);
        assert!(advance(&mut siege, &world, &mut level, &mut rng, 0.0).is_empty());
        advance(&mut siege, &world, &mut level, &mut rng, 0.31);
        assert_eq!(siege.boulders.len(), 1);
        assert!(siege.catapults[0].released);
        world.catapult_physics = enhanced;
        advance(&mut siege, &world, &mut level, &mut rng, 0.04);
        assert_eq!(siege.boulders.len(), 1);
        assert!(siege.catapults[0].fire_t > 0.15);
        world.catapult_physics.clear();
        advance(&mut siege, &world, &mut level, &mut rng, 0.04);
        assert_eq!(siege.boulders.len(), 1);
        assert!(siege.recoils.is_empty());
        assert!(siege.enhanced_clocks.is_empty());
    }

    #[test]
    #[cfg(feature = "save")]
    fn catapult_solver_round_trips_and_rejects_invalid_recoil_records() {
        let (world, mut level, mut siege, mut rng) = fixture();
        advance(&mut siege, &world, &mut level, &mut rng, 0.0);
        advance(&mut siege, &world, &mut level, &mut rng, 0.16);
        let mut resumed: Siege =
            postcard::from_bytes(&postcard::to_allocvec(&siege).unwrap()).unwrap();
        assert!(resumed.restore_recoil(
            siege.recoils.clone(),
            siege.enhanced_clocks.clone(),
            &world,
            &level
        ));
        assert_eq!(resumed, siege);
        let mut invalid = siege.recoils.clone();
        invalid[0].pending = f32::NAN;
        assert!(!resumed.restore_recoil(
            invalid,
            siege.enhanced_clocks.clone(),
            &WorldData::default(),
            &level
        ));
        assert_eq!(resumed, siege);
        let mut saved_level = level.clone();
        let mut saved_rng = rng.clone();
        advance(&mut resumed, &world, &mut saved_level, &mut saved_rng, 0.04);
        advance(&mut siege, &world, &mut level, &mut rng, 0.04);
        assert_eq!(resumed, siege);
        assert_eq!(saved_level.objects[0].x, level.objects[0].x);
    }
}
