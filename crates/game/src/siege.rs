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
}

/// Something a boulder hit.
pub enum BoulderHit {
    Troll(u32, i32),
}

impl Siege {
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
    ) -> Vec<BoulderHit> {
        self.catapults
            .retain(|c| level.index_of(c.object).is_some());
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
            let s = c.side as f32;
            // orig: IsInPosition: within 2.5 of its side's border.
            let in_position = (o.x - (borders.side(c.side) - s * BORDER_DISTANCE)) * s > 0.0;
            let workers = pushers.iter().find(|p| p.0 == c.object).map_or(0, |p| p.1);
            let o = &mut level.objects[i];
            match o.mode {
                mode::MOVING => {
                    if in_position {
                        o.mode = mode::RELOADING;
                    } else {
                        o.x += s * PUSH_SPEED * workers as f32 * dt;
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
                    if target {
                        o.mode = mode::FIRING;
                        c.fire_t = 0.0;
                        c.released = false;
                    } else if !in_position {
                        o.mode = mode::MOVING;
                        o.aim = 0.0;
                    }
                }
                _ => {
                    c.fire_t += dt;
                    if !c.released && c.fire_t >= RELEASE_AT {
                        c.released = true;
                        // orig: ReleaseProjectileFromAnimation: along (dir, 1.5) at projectileForce.
                        let speed = rng.range(d.force.0, d.force.1);
                        let len = libm::sqrtf(1.0 + 1.5 * 1.5);
                        self.boulders.push(Boulder {
                            x: o.x + d.spoon.0 * s,
                            y: o.y + d.spoon.1 - WALK_Y,
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
                    }
                    if c.fire_t >= FIRE_TIME {
                        // orig: FiringDoneFromAnimation.
                        o.aim = 0.0;
                        o.mode = if in_position {
                            mode::RELOADING
                        } else {
                            mode::MOVING
                        };
                    }
                }
            }
        }
        self.update_boulders(dt, enemies, rng)
    }

    /// orig: `Boulder.HitObject()`: a friendly boulder hurts every greed it
    /// touches; on the ground it bounces once, then sticks or bursts.
    fn update_boulders(
        &mut self,
        dt: f32,
        enemies: &[(u32, f32)],
        rng: &mut Rng,
    ) -> Vec<BoulderHit> {
        let g = crate::coins::GRAVITY;
        let mut hits = Vec::new();
        let mut burst = Vec::new();
        for (i, b) in self.boulders.iter_mut().enumerate() {
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
