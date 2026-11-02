//! Statues the monarch pays to bless a profession for a while.
//!
//! orig: `Statue` (charges, overgrowth), `Kingdom.SetCharged()` (the
//! `*StatueCharged` flags). The mason shrine is in `state` (it only flips
//! `Tech::stone`).

use alloc::vec::Vec;

use crate::data::{Deity, WorldData};
use crate::level::Level;
use crate::time::SECONDS_PER_HOUR;

/// orig: `Statue.Overgrow()`: the moss comes back 20 s after the blessing ends.
const OVERGROW_DELAY: f32 = 20.0;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Statue {
    pub object: u32,
    /// orig: `_currentCharge` (0..maxCharge).
    pub charge: f32,
    /// orig: `chargeDurationInDays` (grows for the time statue).
    duration: f32,
    pub charged: bool,
    /// Seconds until the overgrown sprite returns.
    overgrow: Option<f32>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Statues {
    pub list: Vec<Statue>,
}

/// A statue started or stopped blessing. orig: `Kingdom.SetCharged()`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blessing {
    pub deity: Deity,
    pub on: bool,
}

impl Statues {
    /// orig: `Kingdom.*StatueCharged`.
    pub fn charged(&self, world: &WorldData, level: &Level, deity: Deity) -> bool {
        self.list.iter().any(|s| {
            s.charged
                && level
                    .index_of(s.object)
                    .and_then(|i| {
                        world.protos[level.objects[i].proto as usize]
                            .statue
                            .as_ref()
                    })
                    .is_some_and(|d| d.deity == deity)
        })
    }

    fn def<'w>(
        world: &'w WorldData,
        level: &Level,
        object: u32,
    ) -> Option<&'w crate::data::StatueDef> {
        let o = &level.objects[level.index_of(object)?];
        world.protos[o.proto as usize].statue.as_ref()
    }

    /// orig: `Statue.CanPay()`: room for another charge (the time statue only when empty).
    pub fn can_pay(&self, world: &WorldData, level: &Level, object: u32) -> bool {
        let Some(d) = Self::def(world, level, object) else {
            return false;
        };
        let charge = self
            .list
            .iter()
            .find(|s| s.object == object)
            .map_or(0.0, |s| s.charge);
        if d.deity == Deity::Time {
            charge <= 0.0
        } else {
            charge < d.max_charge - 0.5
        }
    }

    /// orig: `Statue.Pay()`: one more charge, rounded up.
    pub fn pay(&mut self, world: &WorldData, level: &Level, object: u32) {
        let Some(d) = Self::def(world, level, object) else {
            return;
        };
        let max = d.max_charge;
        if let Some(s) = self.list.iter_mut().find(|s| s.object == object) {
            s.charge = libm::ceilf((s.charge + 1.0).min(max));
        }
    }

    pub fn update(
        &mut self,
        dt: f32,
        world: &WorldData,
        level: &mut Level,
        mut blessing: impl FnMut(Blessing),
    ) {
        self.list.retain(|s| level.index_of(s.object).is_some());
        for o in &level.objects {
            if let Some(d) = &world.protos[o.proto as usize].statue {
                if !self.list.iter().any(|s| s.object == o.id) {
                    self.list.push(Statue {
                        object: o.id,
                        charge: 0.0,
                        duration: d.charge_days,
                        charged: false,
                        overgrow: None,
                    });
                }
            }
        }
        let day = SECONDS_PER_HOUR * 24.0;
        for s in &mut self.list {
            let Some(oi) = level.index_of(s.object) else {
                continue;
            };
            let Some(d) = world.protos[level.objects[oi].proto as usize]
                .statue
                .clone()
            else {
                continue;
            };
            s.charge = (s.charge - dt / (day * s.duration.max(0.01))).max(0.0);
            // orig: Statue.UpdateCharge.
            if !s.charged && s.charge > 0.0 {
                s.charged = true;
                s.overgrow = None;
                blessing(Blessing {
                    deity: d.deity,
                    on: true,
                });
            } else if s.charged && s.charge <= 0.0 {
                s.charged = false;
                s.overgrow = Some(OVERGROW_DELAY);
                if d.deity == Deity::Time {
                    s.duration *= d.charge_multiplier;
                }
                blessing(Blessing {
                    deity: d.deity,
                    on: false,
                });
            }
            let parts = &mut level.objects[oi].parts;
            // orig: charges[i].alpha = min(1, charge − i).
            for (i, &p) in d.charge_parts.iter().enumerate() {
                if let Some(part) = parts.get_mut(p as usize) {
                    let a = (s.charge - i as f32).clamp(0.0, 1.0);
                    part.color = Some([255, 255, 255, (a * 255.0) as u8]);
                    part.visible = a > 0.0;
                }
            }
            // The root sprite: clean while blessed, moss again 20 s after.
            if let Some(t) = &mut s.overgrow {
                *t -= dt;
                if *t <= 0.0 {
                    s.overgrow = None;
                    if let (Some(p), Some(sp)) = (parts.get_mut(0), d.overgrown) {
                        p.sprite = Some(sp);
                    }
                }
            }
            if s.charged {
                if let (Some(p), Some(sp)) = (parts.get_mut(0), d.clean) {
                    p.sprite = Some(sp);
                }
            }
        }
    }
}
