//! Steeds standing free around the island, waiting to be ridden.
//!
//! orig: `SteedSpawn` (spawns at level start), `Steed` in `Mode.Free`
//! (`Idle()`: wander, eat, rest), `Steed.Pay()` → `Player.Ride()`.

use alloc::vec::Vec;

use crate::data::{Layer, WorldData};
use crate::level::{self, Level};
use crate::rng::Rng;
use crate::units::Mover;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FreeState {
    /// Walking to a spot near home. orig: `_walking`.
    Walking,
    /// Eating for `eatDuration`. orig: `Steed.Eat()`.
    Eating { t: f32 },
    /// Resting 10–15 s.
    Resting { t: f32 },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct FreeSteed {
    /// Its level object (the payable).
    pub object: u32,
    pub steed: u16,
    home: f32,
    pub mover: Mover,
    pub state: FreeState,
    pub anim_t: f32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mounts {
    pub free: Vec<FreeSteed>,
}

impl Mounts {
    /// orig: `SteedSpawn.SpawnSteeds()`: `amount` different steeds from its list.
    pub fn spawn_initial(&mut self, world: &WorldData, level: &mut Level, rng: &mut Rng) {
        let spawns: Vec<(f32, u8, Vec<u16>)> = level
            .objects
            .iter()
            .filter_map(|o| {
                let (n, list) = world.protos[o.proto as usize].steed_spawn.clone()?;
                let ratio = world.layer(o.layer).ratio;
                // Spawners can sit in a parallax layer; the steed stands in the game layer.
                let x = if ratio != 0.0 { o.x / ratio } else { o.x };
                Some((x, n, list))
            })
            .collect();
        for (x, n, mut list) in spawns {
            for _ in 0..n {
                if list.is_empty() {
                    break;
                }
                let i = rng.range_i32(0, list.len() as i32) as usize;
                let s = list.remove(i);
                self.place(world, level, s, x, 1);
            }
        }
    }

    /// Puts steed `s` down free at `x`. orig: `Steed.SetMode(Free)`.
    pub fn place(
        &mut self,
        world: &WorldData,
        level: &mut Level,
        s: u16,
        x: f32,
        facing: i8,
    ) -> u32 {
        let def = &world.steeds[s as usize];
        let p = &world.protos[def.proto as usize];
        let z = p.fixed.and_then(|f| f.fix_z).unwrap_or(p.root_z);
        let o = level::instantiate(world, def.proto, Layer::Game, x, 0.0, z, facing as f32);
        let object = level.add(o);
        let mut mover = Mover::new();
        mover.facing = facing;
        self.free.push(FreeSteed {
            object,
            steed: s,
            home: x,
            mover,
            state: FreeState::Resting { t: 0.0 },
            anim_t: 0.0,
        });
        object
    }

    /// orig: `Steed.CanPay()`: not while it's walking.
    pub fn can_pay(&self, object: u32) -> bool {
        self.free
            .iter()
            .any(|f| f.object == object && f.state != FreeState::Walking)
    }

    /// Takes a free steed off the field to be ridden; returns which steed.
    pub fn take(&mut self, level: &mut Level, object: u32) -> Option<u16> {
        let i = self.free.iter().position(|f| f.object == object)?;
        let f = self.free.remove(i);
        if let Some(oi) = level.index_of(object) {
            level.objects.remove(oi);
        }
        Some(f.steed)
    }

    /// orig: `Steed.Idle()`.
    pub fn update(&mut self, dt: f32, world: &WorldData, level: &mut Level, rng: &mut Rng) {
        for f in &mut self.free {
            let def = &world.steeds[f.steed as usize];
            let prev = core::mem::discriminant(&f.state);
            f.state = match f.state {
                FreeState::Walking if !f.mover.moving_to_goal => {
                    f.mover.stop();
                    FreeState::Eating {
                        t: def.eat_duration,
                    }
                }
                FreeState::Eating { t } if t - dt <= 0.0 => FreeState::Resting {
                    t: rng.range(10.0, 15.0),
                },
                FreeState::Eating { t } => FreeState::Eating { t: t - dt },
                FreeState::Resting { t } if t - dt <= 0.0 => {
                    let goal = f.home + rng.range(-def.wander_range, def.wander_range);
                    f.mover.set_goal(goal, def.walk_speed / 2.0);
                    FreeState::Walking
                }
                FreeState::Resting { t } => FreeState::Resting { t: t - dt },
                s => s,
            };
            if core::mem::discriminant(&f.state) == prev {
                f.anim_t += dt;
            } else {
                f.anim_t = 0.0;
            }
            if let Some(oi) = level.index_of(f.object) {
                let o = &mut level.objects[oi];
                f.mover.update(&mut o.x, dt);
                o.flip = f.mover.facing < 0;
            }
        }
    }
}
