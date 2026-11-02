//! The travelling merchant and his mule: once the castle stands he brings a
//! load of coins every morning; paying him a coin sends him back for more.
//!
//! orig: `MerchantSpawner`, `Merchant` (FSM), `Mule`.

use crate::coins::{Coins, Policy, Who};
use crate::data::{Layer, WorldData};
use crate::kingdom::Borders;
use crate::level::{self, Level};
use crate::rng::Rng;
use crate::units::Mover;

/// orig: Merchant prefab.
const WALK: f32 = 0.75;
const PLAYER_SPOT_RANGE: f32 = 2.0;
const SEARCH_DISTANCE: f32 = 10.0;
const DROP_RANGE: (f32, f32) = (-1.0, 1.0);
/// orig: Mule prefab `walkSpeed`; it follows to within 0.5.
const MULE_WALK: f32 = 0.8;
const MULE_GAP: f32 = 0.5;

/// orig: `Merchant.State`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MerchantState {
    /// On the way in (or waiting by the fire for the morning).
    GoToCastle,
    /// Waiting for the monarch to come close; then dropping coins (`left`, next in `t`).
    ReadyToDrop { dropping: Option<(u8, f32)> },
    /// Standing somewhere free, `t` more seconds.
    WaitForPayment { t: f32 },
    /// Moving to a free spot.
    Searching,
    /// The monarch is near; he can be paid.
    Halt,
    /// Off to restock, then waiting `t` seconds there.
    GoToSpawner { t: Option<f32> },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Merchant {
    /// His level object (the payable).
    pub object: u32,
    pub mover: Mover,
    pub state: MerchantState,
    pub mule_x: f32,
    pub mule: Mover,
    /// Coins on the mule. orig: `_cargo`.
    pub cargo: u8,
    spawner: u32,
    side: i8,
    pub skin: [f32; 3],
    pub anim_t: f32,
}

impl Merchant {
    /// orig: `Mule.SetPack()`: packs on the way in and while dropping.
    pub fn mule_packed(&self) -> bool {
        matches!(
            self.state,
            MerchantState::GoToCastle
                | MerchantState::ReadyToDrop { .. }
                | MerchantState::GoToSpawner { .. }
        )
    }
}

pub struct MerchantCtx<'a> {
    pub world: &'a WorldData,
    pub castle_built: bool,
    pub campfire: f32,
    pub borders: Borders,
    pub safe: bool,
    /// Just became safe this tick. orig: `Kingdom.OnBecameSafe`.
    pub became_safe: bool,
    pub monarch_x: f32,
    pub monarch_running: bool,
    pub payables: &'a [(u32, f32)],
    /// orig: `LevelConfig.incomeMultiplier`.
    pub income: f32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merchants {
    pub list: alloc::vec::Vec<Merchant>,
    /// Spawners that already sent theirs. orig: `MerchantSpawner._hasSpawned`.
    spawned: alloc::vec::Vec<u32>,
}

impl Merchants {
    /// orig: `Merchant.CanPay()`: halted near the monarch, by safe day.
    pub fn can_pay(&self, object: u32, safe: bool) -> bool {
        safe && self
            .list
            .iter()
            .any(|m| m.object == object && m.state == MerchantState::Halt)
    }

    /// orig: `Merchant.Pay()`: off to restock.
    pub fn pay(&mut self, object: u32) {
        if let Some(m) = self.list.iter_mut().find(|m| m.object == object) {
            m.state = MerchantState::GoToSpawner { t: None };
        }
    }

    /// A troll hit the mule. orig: `Mule.HandleOnReceiveDamage()` →
    /// `Merchant.OnMuleDamage()`.
    pub fn mule_hit(&mut self, object: u32, coins: &mut Coins, rng: &mut Rng) {
        let Some(m) = self.list.iter_mut().find(|m| m.object == object) else {
            return;
        };
        if !m.mule_packed() {
            return;
        }
        if m.cargo > 0 {
            m.cargo -= 1;
            drop_coin(coins, m.mule_x, rng);
        }
        if m.cargo == 0 {
            m.state = MerchantState::Searching;
        }
    }

    pub fn update(
        &mut self,
        dt: f32,
        ctx: &MerchantCtx,
        level: &mut Level,
        coins: &mut Coins,
        rng: &mut Rng,
    ) {
        // orig: MerchantSpawner.SpawnMerchant once the castle is built.
        if ctx.castle_built {
            let spawners: alloc::vec::Vec<(u32, f32, u16)> = level
                .objects
                .iter()
                .filter(|o| !self.spawned.contains(&o.id))
                .filter_map(|o| {
                    let p = ctx.world.protos[o.proto as usize].merchant_spawner?;
                    let ratio = ctx.world.layer(o.layer).ratio;
                    Some((o.id, if ratio != 0.0 { o.x / ratio } else { o.x }, p))
                })
                .collect();
            for (id, x, proto) in spawners {
                self.spawned.push(id);
                let p = &ctx.world.protos[proto as usize];
                let o = level::instantiate(
                    ctx.world,
                    proto,
                    Layer::Game,
                    x,
                    crate::build::WALK_Y,
                    p.root_z,
                    1.0,
                );
                let object = level.add(o);
                self.list.push(Merchant {
                    object,
                    mover: Mover::new(),
                    state: MerchantState::GoToCastle,
                    mule_x: x + 1.0,
                    mule: Mover::new(),
                    cargo: cargo(ctx, proto),
                    spawner: id,
                    side: if x > ctx.campfire { 1 } else { -1 },
                    skin: crate::units::random_skin(rng),
                    anim_t: 0.0,
                });
            }
        }
        for m in &mut self.list {
            let Some(oi) = level.index_of(m.object) else {
                continue;
            };
            let x = level.objects[oi].x;
            let proto = level.objects[oi].proto;
            let near = (ctx.monarch_x - x).abs() <= PLAYER_SPOT_RANGE;
            let prev = core::mem::discriminant(&m.state);
            // orig: HandleOnBecameSafe: in GoToCastle inside the walls → ReadyToDrop.
            if ctx.became_safe && m.state == MerchantState::GoToCastle && ctx.borders.contains(x) {
                m.state = MerchantState::ReadyToDrop { dropping: None };
            }
            m.state = match m.state {
                MerchantState::GoToCastle => {
                    m.mover.set_goal(ctx.campfire + m.side as f32, WALK);
                    MerchantState::GoToCastle
                }
                MerchantState::ReadyToDrop { dropping: None } => {
                    if near && !ctx.monarch_running {
                        m.mover.stop();
                        MerchantState::ReadyToDrop {
                            dropping: Some((m.cargo, 0.0)),
                        }
                    } else {
                        MerchantState::ReadyToDrop { dropping: None }
                    }
                }
                // orig: one coin every 0.1 s from the mule, for the monarch.
                MerchantState::ReadyToDrop {
                    dropping: Some((left, t)),
                } => {
                    let t = t - dt;
                    if left == 0 {
                        m.cargo = 0;
                        MerchantState::Searching
                    } else if t <= 0.0 {
                        drop_coin(coins, m.mule_x, rng);
                        m.cargo = m.cargo.saturating_sub(1);
                        MerchantState::ReadyToDrop {
                            dropping: Some((left - 1, 0.1)),
                        }
                    } else {
                        MerchantState::ReadyToDrop {
                            dropping: Some((left, t)),
                        }
                    }
                }
                // orig: WaitForPayment: a spot clear of other price tags, then wait 10 s.
                MerchantState::Searching => {
                    if m.mover.moving_to_goal {
                        MerchantState::Searching
                    } else {
                        let crowded = ctx
                            .payables
                            .iter()
                            .any(|&(id, px)| id != m.object && (px - x).abs() < 0.25);
                        if crowded {
                            m.mover
                                .set_goal(x + m.side as f32 * SEARCH_DISTANCE / 2.0, WALK);
                            MerchantState::Searching
                        } else {
                            MerchantState::WaitForPayment { t: 10.0 }
                        }
                    }
                }
                MerchantState::WaitForPayment { t } => {
                    if near {
                        // orig: ShouldHalt.
                        m.mover.stop();
                        MerchantState::Halt
                    } else if t - dt <= 0.0 {
                        MerchantState::Searching
                    } else {
                        MerchantState::WaitForPayment { t: t - dt }
                    }
                }
                MerchantState::Halt => {
                    if near {
                        MerchantState::Halt
                    } else {
                        MerchantState::WaitForPayment { t: 10.0 }
                    }
                }
                MerchantState::GoToSpawner { t: None } => {
                    let home = level.index_of(m.spawner).map(|i| {
                        let o = &level.objects[i];
                        let r = ctx.world.layer(o.layer).ratio;
                        if r != 0.0 {
                            o.x / r
                        } else {
                            o.x
                        }
                    });
                    match home {
                        Some(h) => {
                            m.mover.set_goal(h, WALK);
                            if m.mover.moving_to_goal {
                                MerchantState::GoToSpawner { t: None }
                            } else {
                                let supply = ctx.world.protos[proto as usize]
                                    .merchant
                                    .map_or(2.0, |d| d.1);
                                MerchantState::GoToSpawner {
                                    t: Some(supply * crate::time::SECONDS_PER_HOUR),
                                }
                            }
                        }
                        None => MerchantState::GoToSpawner { t: Some(0.0) },
                    }
                }
                // orig: supply time over → GoToCastle with a fresh load.
                MerchantState::GoToSpawner { t: Some(t) } => {
                    if t - dt <= 0.0 {
                        m.cargo = cargo(ctx, proto);
                        MerchantState::GoToCastle
                    } else {
                        MerchantState::GoToSpawner { t: Some(t - dt) }
                    }
                }
            };
            if core::mem::discriminant(&m.state) == prev {
                m.anim_t += dt;
            } else {
                m.anim_t = 0.0;
            }
            let o = &mut level.objects[oi];
            m.mover.update(&mut o.x, dt);
            o.flip = m.mover.facing < 0;
            // orig: Mule.Follow: SetGoal(merchant, offset 0.5) keeps it within 0.5.
            let goal = m.mule_x.clamp(o.x - MULE_GAP, o.x + MULE_GAP);
            m.mule.set_goal(goal, MULE_WALK);
            m.mule.update(&mut m.mule_x, dt);
        }
    }
}

/// orig: `GoToCastle()`: `numCoins` × the island's `incomeMultiplier`.
fn cargo(ctx: &MerchantCtx, proto: u16) -> u8 {
    let n = ctx.world.protos[proto as usize].merchant.map_or(8, |d| d.0);
    libm::roundf(n as f32 * ctx.income) as u8
}

/// orig: `Merchant.DropCoin()`: from the mule, for the monarch.
fn drop_coin(coins: &mut Coins, x: f32, rng: &mut Rng) {
    coins.drop(
        Who::Thing,
        (x, 0.25),
        (rng.range(DROP_RANGE.0, DROP_RANGE.1), 2.0),
        Policy::Nobody,
        Some(Who::Monarch),
        None,
    );
}
