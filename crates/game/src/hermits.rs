//! Hermits: paid out of their cabins, they wait near the castle until the
//! monarch carries one to a finished tower, which they turn into a baker's,
//! a ballista or a knight's tower.
//!
//! orig: `Cabin`, `Hermit` (FSM), `Player.PickUpPassenger()` /
//! `DumpPassenger()`, `PayableUpgrade.passengerUpgrades`.

use alloc::vec::Vec;

use crate::data::{HermitKind, Layer, ProtoId, WorldData};
use crate::kingdom::Borders;
use crate::level::{self, Level};
use crate::rng::Rng;
use crate::units::Mover;

/// orig: Hermit prefab `walkSpeed`, `maxFreeSpotSearchDistance`, collider width.
const WALK: f32 = 0.5;
const SEARCH_DISTANCE: f32 = 8.0;
const WIDTH: f32 = 1.0;
/// orig: `Hermit.Building()`: five hops 0.4 s apart.
const BUILD_TIME: f32 = 2.0;

/// orig: `Hermit.State`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HermitState {
    /// Walking back inside the walls.
    GoToCastle,
    /// Standing somewhere no other price tag is, `t` seconds more.
    Waiting {
        t: f32,
    },
    /// Moving away from a crowded spot.
    Searching,
    /// The monarch is about to pay for it.
    Halt,
    Building {
        t: f32,
    },
    /// Riding behind the monarch.
    Riding,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Hermit {
    pub object: u32,
    pub kind: HermitKind,
    pub mover: Mover,
    pub state: HermitState,
    pub anim_t: f32,
    /// orig: `OnEnable`: recolorB = RandomSkinColor().
    pub skin: [f32; 3],
    /// orig: `_wasInsideKingdom` (for dismounting at night).
    was_inside: bool,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hermits {
    pub list: Vec<Hermit>,
    /// Cabins whose hermit already left. orig: `Cabin._hermitIsHome`.
    pub left_home: Vec<u32>,
}

/// What hermits need from the rest of the game.
pub struct HermitCtx<'a> {
    pub borders: Borders,
    pub campfire: f32,
    pub daytime: bool,
    pub monarch_x: f32,
    /// The payable the monarch is paying for or looking at.
    pub selected: Option<u32>,
    /// Other price tags' x (for finding a free spot).
    pub payables: &'a [(u32, f32)],
}

impl Hermits {
    /// orig: `Cabin.CanPay()`.
    pub fn cabin_can_pay(&self, cabin: u32) -> bool {
        !self.left_home.contains(&cabin)
    }

    /// orig: `Cabin.Pay()`: the hermit steps out of the door.
    pub fn open_cabin(&mut self, world: &WorldData, level: &mut Level, cabin: u32, rng: &mut Rng) {
        let Some(ci) = level.index_of(cabin) else {
            return;
        };
        let c = &level.objects[ci];
        let Some(proto) = world.protos[c.proto as usize].cabin else {
            return;
        };
        let dir = if c.flip { -1.0 } else { 1.0 };
        let x = c.x + 0.875 * dir;
        self.left_home.push(cabin);
        self.spawn(world, level, proto, x, rng);
    }

    /// A hermit of prefab `proto` at `x`, heading for the castle.
    pub fn spawn(
        &mut self,
        world: &WorldData,
        level: &mut Level,
        proto: ProtoId,
        x: f32,
        rng: &mut Rng,
    ) {
        let Some(kind) = world.protos[proto as usize].hermit else {
            return;
        };
        let p = &world.protos[proto as usize];
        let z = p.fixed.and_then(|f| f.fix_z).unwrap_or(p.root_z);
        let o = level::instantiate(world, proto, Layer::Game, x, crate::build::WALK_Y, z, 1.0);
        let object = level.add(o);
        self.list.push(Hermit {
            object,
            kind,
            mover: Mover::new(),
            state: HermitState::GoToCastle,
            anim_t: 0.0,
            skin: crate::units::random_skin(rng),
            was_inside: false,
        });
    }

    /// orig: `Hermit.CanPay()`: not while building or riding.
    pub fn can_pay(&self, object: u32) -> bool {
        self.list.iter().any(|h| {
            h.object == object
                && !matches!(h.state, HermitState::Building { .. } | HermitState::Riding)
        })
    }

    pub fn kind_of(&self, object: u32) -> Option<HermitKind> {
        self.list
            .iter()
            .find(|h| h.object == object)
            .map(|h| h.kind)
    }

    /// orig: `Hermit.Mount()`.
    pub fn mount(&mut self, object: u32) {
        if let Some(h) = self.list.iter_mut().find(|h| h.object == object) {
            h.state = HermitState::Riding;
            h.was_inside = false;
            h.mover.stop();
        }
    }

    /// orig: `Hermit.Dismount()`: hops off where the monarch is.
    pub fn dismount(&mut self, level: &mut Level, object: u32, x: f32, rng: &mut Rng) {
        if let Some(h) = self.list.iter_mut().find(|h| h.object == object) {
            h.state = HermitState::GoToCastle;
            h.mover.stop();
            h.mover.vx = rng.range(-2.0, 2.0);
            if let Some(i) = level.index_of(object) {
                level.objects[i].x = x;
            }
        }
    }

    /// orig: `Hermit.Build()` (after `OnPassengerUsed`).
    pub fn build(&mut self, object: u32) {
        if let Some(h) = self.list.iter_mut().find(|h| h.object == object) {
            h.state = HermitState::Building { t: BUILD_TIME };
            h.mover.stop();
        }
    }

    /// Updates every hermit. Returns true when the riding one wants off.
    /// orig: `Hermit.Update()` FSM and `ShouldDismount()`.
    pub fn update(&mut self, dt: f32, ctx: &HermitCtx, level: &mut Level, rng: &mut Rng) -> bool {
        let mut dismount = false;
        for h in &mut self.list {
            let Some(oi) = level.index_of(h.object) else {
                continue;
            };
            let x = level.objects[oi].x;
            let prev = core::mem::discriminant(&h.state);
            // orig: ShouldHalt interrupts GoToCastle and SearchPickupSpot.
            if matches!(
                h.state,
                HermitState::GoToCastle | HermitState::Waiting { .. } | HermitState::Searching
            ) && ctx.selected == Some(h.object)
            {
                h.mover.stop();
                h.state = HermitState::Halt;
            }
            h.state = match h.state {
                HermitState::Riding => {
                    level.objects[oi].x = ctx.monarch_x;
                    // orig: ShouldDismount: leaving the kingdom (±5) at night.
                    if !ctx.daytime {
                        let inside = ctx.borders.left - 5.0 < x && x < ctx.borders.right + 5.0;
                        if h.was_inside && !inside {
                            dismount = true;
                        }
                        h.was_inside = inside;
                    }
                    HermitState::Riding
                }
                HermitState::Halt => {
                    if ctx.selected == Some(h.object) {
                        HermitState::Halt
                    } else {
                        HermitState::GoToCastle
                    }
                }
                HermitState::Building { t } => {
                    let t = t - dt;
                    if t <= 0.0 {
                        HermitState::GoToCastle
                    } else {
                        HermitState::Building { t }
                    }
                }
                HermitState::GoToCastle => {
                    if x < ctx.borders.left || x > ctx.borders.right {
                        h.mover.set_goal(ctx.campfire + rng.range(-2.0, 2.0), WALK);
                    }
                    if h.mover.moving_to_goal {
                        HermitState::GoToCastle
                    } else {
                        HermitState::Searching
                    }
                }
                // orig: SearchPickupSpot: stay 10 s where no other price tag is,
                // else walk 4 outward and look again.
                HermitState::Searching => {
                    if h.mover.moving_to_goal {
                        HermitState::Searching
                    } else {
                        let crowded = ctx
                            .payables
                            .iter()
                            .any(|&(id, px)| id != h.object && (px - x).abs() < WIDTH);
                        if crowded {
                            let out = if x < ctx.campfire { -1.0 } else { 1.0 };
                            h.mover.set_goal(x + out * SEARCH_DISTANCE / 2.0, WALK);
                            HermitState::Searching
                        } else {
                            HermitState::Waiting { t: 10.0 }
                        }
                    }
                }
                HermitState::Waiting { t } => {
                    let t = t - dt;
                    if t <= 0.0 {
                        HermitState::Searching
                    } else {
                        HermitState::Waiting { t }
                    }
                }
            };
            if core::mem::discriminant(&h.state) == prev {
                h.anim_t += dt;
            } else {
                h.anim_t = 0.0;
            }
            if h.state != HermitState::Riding {
                let o = &mut level.objects[oi];
                h.mover.update(&mut o.x, dt);
                o.flip = h.mover.facing < 0;
            }
        }
        dismount
    }
}
