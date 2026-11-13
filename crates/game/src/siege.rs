//! The workshop's catapults: builders push one out to the wall, crank it,
//! and it lobs boulders at greed in front of it.
//!
//! orig: `PayableWorkshop`, `Catapult`, `Boulder` (friendly).

use alloc::vec::Vec;

use crate::build::{under_construction, WALK_Y};
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
    /// Lodged in the ground; gone after a while.
    pub stuck: Option<f32>,
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
}

/// Something a boulder hit.
pub enum BoulderHit {
    Troll(u32, i32),
}

impl Siege {
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
        enemies: &[(u32, f32)],
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
                    let target = enemies.iter().any(|&(_, ex)| {
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
        self.update_boulders(dt, existing_boulders, &new_flight_times, enemies, rng)
    }

    /// orig: `Boulder.HitObject()`: a friendly boulder hurts every greed it
    /// touches; on the ground it bounces once, then sticks or bursts.
    fn update_boulders(
        &mut self,
        dt: f32,
        existing: usize,
        new_flight_times: &[f32],
        enemies: &[(u32, f32)],
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
                for &(id, ex) in enemies {
                    if (ex - b.x).abs() < 0.4 && b.y < 0.8 && !b.hit.contains(&id) {
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
            &[(100, 12.0)],
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
