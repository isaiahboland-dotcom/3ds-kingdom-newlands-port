//! Treasure chests and the castle's banker.
//!
//! orig: `Chest` (spills its coins when the monarch stops by), `Banker`
//! (keeps the coins the monarch drops, pays daily interest and pays out a
//! third of the stash when the monarch stands with him).

use alloc::vec::Vec;

use crate::coins::{Coins, Policy, Who};
use crate::data::{BankerDef, WorldData};
use crate::level::Level;
use crate::rng::Rng;
use crate::units::Mover;

/// Half-width of a chest's trigger and the monarch's reach.
const CHEST_REACH: f32 = 0.5;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChestState {
    Closed,
    /// The lid opens (0.4 s).
    Opening {
        t: f32,
    },
    /// Coins still to spill, one every 0.1 s.
    Spilling {
        left: u8,
        t: f32,
    },
    /// Empty, fading after 10 s.
    Empty {
        t: f32,
    },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Chest {
    pub object: u32,
    pub state: ChestState,
    pub anim_t: f32,
}

/// orig: `Banker` states.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BankerState {
    /// Standing 10 s (`t`), then strolling near the castle.
    Idle {
        walking: bool,
        t: f32,
    },
    GrabCoin,
    /// Off to the castle door with a full purse, then in coin by coin.
    DropOff {
        left: Option<u32>,
        t: f32,
    },
    /// Hiding in the castle (night or greed nearby).
    Hide {
        inside: bool,
    },
    /// Out of the door again.
    Emerge {
        t: f32,
    },
    /// Paying the monarch: coins left, next in `t`.
    Payout {
        left: u32,
        t: f32,
    },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Banker {
    pub x: f32,
    pub mover: Mover,
    pub state: BankerState,
    /// In hand. orig: its `Wallet`.
    pub coins: u32,
    /// In the castle vault. orig: `_stashedCoins`.
    pub stash: u32,
    /// orig: `_canPayoutToday`, `_lastPayout`.
    can_payout_today: bool,
    since_payout: f32,
    pub skin: [f32; 3],
    pub anim_t: f32,
    def: BankerDef,
}

impl Banker {
    pub fn hidden(&self) -> bool {
        matches!(self.state, BankerState::Hide { inside: true })
    }
}

/// What the banker needs from the kingdom.
pub struct BankCtx {
    pub campfire: f32,
    pub daytime: bool,
    pub safe: bool,
    /// Greed within 5. orig: `_enemyScanner` (range 5).
    pub enemies_near: bool,
    pub monarch_x: f32,
    pub monarch_running: bool,
    pub monarch_coins: u32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Treasure {
    pub chests: Vec<Chest>,
    pub banker: Option<Banker>,
}

impl Treasure {
    pub fn spawn_banker(&mut self, def: BankerDef, x: f32, rng: &mut Rng) {
        if self.banker.is_some() {
            return;
        }
        self.banker = Some(Banker {
            x,
            mover: Mover::new(),
            state: BankerState::Idle {
                walking: false,
                t: 0.0,
            },
            coins: 0,
            stash: 0,
            can_payout_today: true,
            since_payout: 20.0,
            skin: crate::units::random_skin(rng),
            anim_t: 0.0,
            def,
        });
    }

    /// The banker's purse has room and he grabs only the monarch's coins.
    pub fn banker_room(&self) -> Option<(f32, u32)> {
        let b = self.banker.as_ref()?;
        let grabbing = matches!(b.state, BankerState::Idle { .. } | BankerState::GrabCoin);
        (grabbing && b.coins < b.def.max_coins).then_some((b.x, b.def.max_coins - b.coins))
    }

    pub fn credit_banker(&mut self) {
        if let Some(b) = &mut self.banker {
            b.coins += 1;
        }
    }

    /// orig: `Banker.HandleOnDayStart()`: interest, and a payout allowed again.
    pub fn dawn(&mut self) {
        if let Some(b) = &mut self.banker {
            let interest = libm::ceilf(b.def.daily_interest * b.stash as f32) as i32;
            b.stash += interest.min(b.def.max_interest).max(0) as u32;
            b.can_payout_today = true;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        dt: f32,
        world: &WorldData,
        level: &mut Level,
        ctx: &BankCtx,
        income: f32,
        coins: &mut Coins,
        rng: &mut Rng,
    ) {
        self.update_chests(dt, world, level, ctx, income, coins, rng);
        if let Some(b) = &mut self.banker {
            update_banker(b, dt, ctx, coins, rng);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn update_chests(
        &mut self,
        dt: f32,
        world: &WorldData,
        level: &mut Level,
        ctx: &BankCtx,
        income: f32,
        coins: &mut Coins,
        rng: &mut Rng,
    ) {
        for o in &level.objects {
            if world.protos[o.proto as usize].chest.is_some()
                && !self.chests.iter().any(|c| c.object == o.id)
            {
                self.chests.push(Chest {
                    object: o.id,
                    state: ChestState::Closed,
                    anim_t: 0.0,
                });
            }
        }
        let mut gone = Vec::new();
        for c in &mut self.chests {
            let Some(oi) = level.index_of(c.object) else {
                gone.push(c.object);
                continue;
            };
            let o = &level.objects[oi];
            let Some((n, spill)) = world.protos[o.proto as usize].chest else {
                continue;
            };
            let prev = core::mem::discriminant(&c.state);
            c.state = match c.state {
                // orig: OnTriggerStay2D: the monarch, not galloping.
                ChestState::Closed => {
                    if (ctx.monarch_x - o.x).abs() < CHEST_REACH && !ctx.monarch_running {
                        ChestState::Opening { t: 0.4 }
                    } else {
                        ChestState::Closed
                    }
                }
                ChestState::Opening { t } if t - dt <= 0.0 => ChestState::Spilling {
                    left: libm::roundf(n as f32 * income) as u8,
                    t: 0.0,
                },
                ChestState::Opening { t } => ChestState::Opening { t: t - dt },
                ChestState::Spilling { left: 0, .. } => ChestState::Empty { t: 10.0 },
                ChestState::Spilling { left, t } if t - dt <= 0.0 => {
                    coins.drop(
                        Who::Thing,
                        (o.x, 0.25),
                        (rng.range(spill.0, spill.1), 3.0),
                        Policy::Nobody,
                        Some(Who::Monarch),
                        None,
                    );
                    ChestState::Spilling {
                        left: left - 1,
                        t: 0.1,
                    }
                }
                ChestState::Spilling { left, t } => ChestState::Spilling { left, t: t - dt },
                ChestState::Empty { t } => {
                    if t - dt <= 0.0 {
                        gone.push(c.object);
                    }
                    ChestState::Empty { t: t - dt }
                }
            };
            if core::mem::discriminant(&c.state) == prev {
                c.anim_t += dt;
            } else {
                c.anim_t = 0.0;
            }
        }
        // orig: FadeOut(1, Destroy) 10 s after emptying.
        for id in gone {
            self.chests.retain(|c| c.object != id);
            if let Some(i) = level.index_of(id) {
                level.objects.remove(i);
            }
        }
    }
}

/// orig: `Banker` FSM. Hide interrupts anything (and can't be interrupted);
/// from Idle: DropOff with a full purse, GrabCoin for the monarch's coins,
/// Payout when the monarch stands with him.
fn update_banker(b: &mut Banker, dt: f32, ctx: &BankCtx, coins: &mut Coins, rng: &mut Rng) {
    let d = b.def;
    let door = ctx.campfire + 0.3;
    b.since_payout += dt;
    let prev = core::mem::discriminant(&b.state);
    let hiding = matches!(b.state, BankerState::Hide { .. });
    if !hiding && (ctx.enemies_near || !ctx.daytime) {
        b.mover.set_goal(door, d.run);
        b.state = BankerState::Hide { inside: false };
    }
    b.state = match b.state {
        BankerState::Hide { inside: false } => {
            if b.mover.moving_to_goal {
                BankerState::Hide { inside: false }
            } else {
                // orig: purse into the stash, then in through the door.
                b.stash += b.coins;
                b.coins = 0;
                b.mover.stop();
                BankerState::Hide { inside: true }
            }
        }
        BankerState::Hide { inside: true } => {
            if ctx.safe && !ctx.enemies_near {
                b.x = door;
                BankerState::Emerge { t: 1.3 }
            } else {
                BankerState::Hide { inside: true }
            }
        }
        BankerState::Emerge { t } => {
            b.mover.set_speed(d.walk, 1);
            if t - dt <= 0.0 {
                b.mover.stop();
                BankerState::Idle {
                    walking: false,
                    t: 0.0,
                }
            } else {
                BankerState::Emerge { t: t - dt }
            }
        }
        BankerState::Idle { walking, t } => {
            let near = (ctx.monarch_x - b.x).abs() < 0.5;
            if b.coins >= d.max_coins {
                b.can_payout_today = false;
                b.mover.set_goal(door + 0.5, d.run);
                BankerState::DropOff { left: None, t: 0.0 }
            } else if let Some(c) = coins.list.iter_mut().find(|c| {
                c.is_free() && c.dropped_by_monarch() && (c.x - b.x).abs() <= d.coin_scan_range
            }) {
                if c.try_claim(Who::Banker, 5.0) {
                    if b.coins >= 2 {
                        b.can_payout_today = false;
                    }
                    b.mover.set_goal(c.x, d.run);
                    BankerState::GrabCoin
                } else {
                    BankerState::Idle { walking, t }
                }
            } else if near
                && b.can_payout_today
                && b.since_payout > d.payout_interval
                && !ctx.monarch_running
            {
                // orig: Payout: a third of the stash, keeping the monarch under playerMaxCoins.
                let n = (libm::ceilf(d.payout_portion * b.stash as f32) as u32)
                    .min(d.player_max_coins.saturating_sub(ctx.monarch_coins));
                if n <= 2 {
                    BankerState::Idle { walking, t }
                } else {
                    b.mover.stop();
                    b.since_payout = 0.0;
                    BankerState::Payout { left: n, t: 0.3 }
                }
            } else if walking {
                if b.mover.moving_to_goal {
                    BankerState::Idle { walking, t }
                } else {
                    BankerState::Idle {
                        walking: false,
                        t: 0.0,
                    }
                }
            } else if t + dt >= 10.0 {
                b.mover.set_goal(
                    ctx.campfire + rng.range(-d.wander_range, d.wander_range),
                    d.walk,
                );
                BankerState::Idle {
                    walking: true,
                    t: 0.0,
                }
            } else {
                b.mover.stop();
                BankerState::Idle {
                    walking: false,
                    t: t + dt,
                }
            }
        }
        BankerState::GrabCoin => {
            if b.mover.moving_to_goal {
                BankerState::GrabCoin
            } else {
                BankerState::Idle {
                    walking: false,
                    t: 0.0,
                }
            }
        }
        BankerState::DropOff { left: None, t } => {
            if b.mover.moving_to_goal {
                BankerState::DropOff { left: None, t }
            } else {
                BankerState::DropOff {
                    left: Some(b.coins),
                    t: 0.2,
                }
            }
        }
        // orig: one coin into the castle every 0.2 s.
        BankerState::DropOff {
            left: Some(left),
            t,
        } => {
            if left == 0 {
                BankerState::Idle {
                    walking: false,
                    t: 0.0,
                }
            } else if t - dt <= 0.0 {
                b.coins = b.coins.saturating_sub(1);
                b.stash += 1;
                BankerState::DropOff {
                    left: Some(left - 1),
                    t: 0.2,
                }
            } else {
                BankerState::DropOff {
                    left: Some(left),
                    t: t - dt,
                }
            }
        }
        BankerState::Payout { left, t } => {
            if left == 0 {
                BankerState::Idle {
                    walking: false,
                    t: 0.0,
                }
            } else if t - dt <= 0.0 {
                b.stash = b.stash.saturating_sub(1);
                let toward = if ctx.monarch_x >= b.x { 1.0 } else { -1.0 };
                coins.drop(
                    Who::Banker,
                    (b.x, 0.2),
                    (rng.range(1.1, 2.1) * toward, 2.5),
                    Policy::Nobody,
                    Some(Who::Monarch),
                    Some(0.4),
                );
                BankerState::Payout {
                    left: left - 1,
                    t: 0.1,
                }
            } else {
                BankerState::Payout { left, t: t - dt }
            }
        }
    };
    if core::mem::discriminant(&b.state) == prev {
        b.anim_t += dt;
    } else {
        b.anim_t = 0.0;
    }
    if !b.hidden() {
        b.mover.update(&mut b.x, dt);
    }
}
