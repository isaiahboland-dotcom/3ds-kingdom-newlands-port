//! The dog: freed from under a fallen tree with a coin, it follows the
//! monarch, barks toward portals that send out greed, and cowers near them.
//!
//! orig: `DogSpawn`, `Dog`.

use crate::coins::Coins;
use crate::data::WorldData;
use crate::level::Level;
use crate::rng::Rng;
use crate::units::Mover;

/// orig: Dog prefab `moveSpeed`, `maxPlayerDist`; the enemy scanner's range.
const SPEED: f32 = 4.0;
const MAX_PLAYER_DIST: f32 = 10.0;
const COWER_RANGE: f32 = 10.0;

/// orig: `Dog.State`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DogState {
    Follow,
    /// Pausing, then a short wander, then resting `t`.
    Idle {
        wandering: bool,
        t: f32,
    },
    /// Barking toward a portal for `t` more seconds (or running up to the monarch).
    Bark {
        t: f32,
    },
    Cower {
        t: f32,
    },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Dog {
    pub x: f32,
    pub mover: Mover,
    pub state: DogState,
    pub color: [f32; 3],
    /// orig: `_barkingAt` (the portal's x).
    pub barking_at: Option<f32>,
    pub barking: bool,
    pub anim_t: f32,
}

/// What the dog reacts to.
pub struct DogCtx<'a> {
    pub monarch_x: f32,
    pub monarch_running: bool,
    /// Greed x positions.
    pub enemies: &'a [f32],
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dogs {
    pub dog: Option<Dog>,
    /// Dog houses already emptied.
    freed: alloc::vec::Vec<u32>,
}

impl Dogs {
    pub fn freed(&self, house: u32) -> bool {
        self.freed.contains(&house)
    }

    /// orig: `DogSpawn.OnTriggerEnter2D`: a coin the monarch dropped frees it.
    pub fn check_rescue(
        &mut self,
        world: &WorldData,
        level: &mut Level,
        coins: &Coins,
        rng: &mut Rng,
    ) {
        for oi in 0..level.objects.len() {
            let o = &level.objects[oi];
            let Some(def) = world.protos[o.proto as usize].dog_spawn.as_ref() else {
                continue;
            };
            if self.freed.contains(&o.id) {
                continue;
            }
            let x = o.x + def.trapped_x;
            let hit = coins
                .list
                .iter()
                .any(|c| c.dropped_by_monarch() && (c.x - x).abs() < 1.0 && c.y < 1.5);
            if !hit {
                continue;
            }
            self.freed.push(o.id);
            let color = if def.colors.is_empty() {
                [0.8; 3]
            } else {
                def.colors[rng.range_i32(0, def.colors.len() as i32) as usize]
            };
            if let Some(p) = def
                .trapped_part
                .and_then(|p| level.objects[oi].parts.get_mut(p as usize))
            {
                p.visible = false;
            }
            // orig: SetDog: a new dog sends the old one off.
            self.dog = Some(Dog {
                x: x + 0.1,
                mover: Mover::new(),
                state: DogState::Idle {
                    wandering: false,
                    t: 0.75,
                },
                color,
                barking_at: None,
                barking: false,
                anim_t: 0.0,
            });
        }
    }

    /// A dog that came along from the last island. orig: `UnlockManager.Restore()`.
    pub fn adopt(&mut self, x: f32, color: [f32; 3]) {
        self.dog = Some(Dog {
            x,
            mover: Mover::new(),
            state: DogState::Idle {
                wandering: false,
                t: 0.75,
            },
            color,
            barking_at: None,
            barking: false,
            anim_t: 0.0,
        });
    }

    /// orig: `Dog.HandleOnEnemiesSpawned()`: bark at the nearest active portal.
    pub fn enemies_spawned(&mut self, portal_x: f32) {
        if let Some(d) = &mut self.dog {
            if d.barking_at
                .is_none_or(|b| (portal_x - d.x).abs() < (b - d.x).abs())
            {
                d.barking_at = Some(portal_x);
            }
        }
    }

    /// orig: `Dog.HandleOnBecameSafe()`.
    pub fn became_safe(&mut self) {
        if let Some(d) = &mut self.dog {
            d.barking_at = None;
        }
    }

    /// orig: `Dog` FSM: Cower interrupts anything; Idle → Follow when the
    /// monarch gallops or wanders off; Bark from Idle and Follow.
    pub fn update(&mut self, dt: f32, ctx: &DogCtx, rng: &mut Rng) {
        let Some(d) = &mut self.dog else { return };
        let prev = core::mem::discriminant(&d.state);
        let threat = ctx.enemies.iter().any(|&e| (e - d.x).abs() <= COWER_RANGE);
        if threat && !matches!(d.state, DogState::Cower { .. }) {
            d.barking_at = None;
            d.state = DogState::Cower { t: 0.0 };
        }
        let should_follow = ctx.monarch_running || (ctx.monarch_x - d.x).abs() > MAX_PLAYER_DIST;
        d.state = match d.state {
            DogState::Cower { t } => {
                d.mover.set_goal(ctx.monarch_x, SPEED);
                d.barking = true;
                let t = t - dt;
                if t > 0.0 {
                    DogState::Cower { t }
                } else if threat {
                    DogState::Cower {
                        t: rng.range(3.0, 6.0),
                    }
                } else {
                    d.barking = false;
                    DogState::Idle {
                        wandering: false,
                        t: 0.75,
                    }
                }
            }
            DogState::Bark { t } => {
                let t = t - dt;
                if t > 0.0 {
                    DogState::Bark { t }
                } else {
                    d.barking = false;
                    DogState::Idle {
                        wandering: false,
                        t: 0.75,
                    }
                }
            }
            s if d.barking_at.is_some() => {
                // orig: Bark(): near the monarch, bark toward the portal; else run up.
                let _ = s;
                let p = d.barking_at.unwrap_or(d.x);
                let dir = (p - ctx.monarch_x).signum();
                if (ctx.monarch_x - d.x).abs() < 4.0 {
                    d.mover.stop();
                    d.mover.facing = dir as i8;
                    d.barking = true;
                    DogState::Bark { t: 2.5 }
                } else {
                    d.mover.set_goal(ctx.monarch_x + dir * 2.0, SPEED);
                    DogState::Follow
                }
            }
            DogState::Follow => {
                // orig: SetGoal(player, offset 1.5): within 1.5.
                let goal = d.x.clamp(ctx.monarch_x - 1.5, ctx.monarch_x + 1.5);
                d.mover.set_goal(goal, SPEED);
                if d.mover.moving_to_goal {
                    DogState::Follow
                } else {
                    DogState::Idle {
                        wandering: false,
                        t: 0.75,
                    }
                }
            }
            DogState::Idle { wandering, t } => {
                if should_follow && !wandering {
                    DogState::Follow
                } else if wandering {
                    if d.mover.moving_to_goal {
                        DogState::Idle { wandering, t }
                    } else if t - dt > 0.0 {
                        d.mover.stop();
                        DogState::Idle {
                            wandering,
                            t: t - dt,
                        }
                    } else if should_follow {
                        DogState::Follow
                    } else {
                        DogState::Idle {
                            wandering: false,
                            t: 0.75,
                        }
                    }
                } else if t - dt <= 0.0 {
                    d.mover.set_goal(d.x + rng.range(-5.0, 5.0), SPEED);
                    DogState::Idle {
                        wandering: true,
                        t: rng.range(3.0, 10.0),
                    }
                } else {
                    DogState::Idle {
                        wandering,
                        t: t - dt,
                    }
                }
            }
        };
        if core::mem::discriminant(&d.state) == prev {
            d.anim_t += dt;
        } else {
            d.anim_t = 0.0;
        }
        d.mover.update(&mut d.x, dt);
    }
}
