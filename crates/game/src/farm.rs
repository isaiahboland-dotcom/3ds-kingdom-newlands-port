//! Farmhouses seed farmland on both sides; farmers work it; ripe fields
//! drop coins; neglected fields rot and disappear.
//!
//! orig: `Farmhouse`, `Farmland`, `Kingdom.ReassignFarms()` (in `units`).
//! Farmland chains are linked lists in the original (`_child`), kept here as
//! object ids into the level.

use alloc::vec::Vec;

use crate::build::under_construction;
use crate::coins::{Coins, Policy, Who};
use crate::data::{FarmlandDef, Gaps, Layer, WorldData};
use crate::level::{self, Level};
use crate::rng::Rng;

/// Seconds between a ripe field's coins. orig: `Farmland.DropCoins()`.
const COIN_INTERVAL: f32 = 0.1;
/// orig: `Farmland.DropCoins()` `farmlandWidth`.
const COIN_SPREAD: f32 = 2.0;
/// orig: `Farmland.Update()`: a rotten field without a child goes after this.
const REMOVE_CHECK: f32 = 10.0;

/// orig: `Farmland`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Farmland {
    /// Its level object.
    pub object: u32,
    /// orig: `farmhouse` (object id).
    pub farmhouse: Option<u32>,
    /// The next field outward. orig: `_child`.
    child: Option<u32>,
    /// orig: `_direction`.
    dir: i8,
    pub farmable: bool,
    /// orig: `currentFarmPoints`.
    pub points: i32,
    /// orig: `_decayCountdown`, `_hasDecayed`.
    decay: f32,
    decayed: bool,
    /// Visible once first worked. orig: `_isFarmed`.
    farmed: bool,
}

/// orig: `Farmhouse` runtime state.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
struct House {
    object: u32,
    left_seed: Option<u32>,
    right_seed: Option<u32>,
    /// orig: `CheckSeeds()` wait.
    t: f32,
}

/// What the rest of the game needs from the world this tick.
pub struct FarmCtx<'a> {
    pub world: &'a WorldData,
    /// orig: `World.winterness`.
    pub winterness: f32,
    /// orig: `Kingdom.farmerStatueCharged`.
    pub statue: bool,
    pub forests: &'a [(f32, f32)],
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Farms {
    pub lands: Vec<Farmland>,
    houses: Vec<House>,
    /// Coins still to drop from harvests: (seconds until, field x).
    drops: Vec<(f32, f32)>,
    /// The set of farmable fields changed. orig: `Kingdom.AddFarmland()` /
    /// `RemoveFarmland()` → `ReassignFarms()`.
    pub changed: bool,
}

impl Farms {
    fn land(&self, object: u32) -> Option<usize> {
        self.lands.iter().position(|l| l.object == object)
    }

    fn house_def(
        world: &WorldData,
        level: &Level,
        house: u32,
    ) -> Option<crate::data::FarmhouseDef> {
        let o = &level.objects[level.index_of(house)?];
        world.protos[o.proto as usize].farmhouse
    }

    fn max_farmlands(ctx: &FarmCtx, level: &Level, house: u32) -> usize {
        Self::house_def(ctx.world, level, house).map_or(0, |d| {
            d.max_farmlands as usize
                + if ctx.statue {
                    d.statue_extra_farmlands as usize
                } else {
                    0
                }
        })
    }

    fn count(&self, house: u32) -> usize {
        self.lands
            .iter()
            .filter(|l| l.farmhouse == Some(house))
            .count()
    }

    /// orig: `Farmhouse.CanSpawnNewFarmland()`.
    fn can_spawn(&self, ctx: &FarmCtx, level: &Level, house: u32) -> bool {
        self.count(house) < Self::max_farmlands(ctx, level, house)
    }

    /// orig: `Farmhouse.HasTooManyFarmlands()`.
    fn too_many(&self, ctx: &FarmCtx, level: &Level, house: u32) -> bool {
        self.count(house) > Self::max_farmlands(ctx, level, house)
    }

    /// The farmhouse was upgraded into a new object; its fields follow.
    /// orig: `Farmhouse.HandleOnPayableUpgraded()`.
    pub fn remap_house(&mut self, old: u32, new: u32) {
        for h in &mut self.houses {
            if h.object == old {
                h.object = new;
            }
        }
        for l in &mut self.lands {
            if l.farmhouse == Some(old) {
                l.farmhouse = Some(new);
            }
        }
    }

    /// orig: `Farmland.Link()`, following the chain outward.
    fn link(&mut self, ctx: &FarmCtx, level: &Level, mut object: u32, house: u32, dir: i8) {
        let mut guard = 0;
        while let Some(i) = self.land(object) {
            guard += 1;
            if guard > 64 {
                break;
            }
            self.changed = true;
            let l = &mut self.lands[i];
            l.farmable = true;
            l.farmhouse = Some(house);
            l.dir = dir;
            let Some(child) = l.child else { break };
            if !self.can_spawn(ctx, level, house) {
                break;
            }
            let back = self
                .land(child)
                .map(|c| (self.lands[c].child, self.lands[c].farmhouse));
            match back {
                None => {
                    self.lands[i].child = None;
                    break;
                }
                Some((cc, ch)) if cc == Some(object) || (ch.is_some() && ch != Some(house)) => {
                    self.lands[i].child = None;
                    break;
                }
                Some(_) => object = child,
            }
        }
    }

    /// orig: `Farmland.Unlink()`, down the chain.
    fn unlink(&mut self, mut object: u32) {
        let mut guard = 0;
        while let Some(i) = self.land(object) {
            guard += 1;
            if guard > 64 {
                break;
            }
            self.changed = true;
            let l = &mut self.lands[i];
            l.farmable = false;
            l.farmhouse = None;
            match l.child {
                Some(c) => object = c,
                None => break,
            }
        }
    }

    /// A field at `x`, or the one already there; `None` if something else
    /// blocks it. orig: `Farmland.CreateOrFindExisting()` (an overlap test
    /// against the NotFarmable layer).
    fn create_or_find(
        &mut self,
        ctx: &FarmCtx,
        level: &mut Level,
        x: f32,
        rng: &mut Rng,
    ) -> Option<u32> {
        let def = ctx.world.farmland.as_ref()?;
        let half = def.width * 0.5;
        let (a, b) = (x - half, x + half);
        // Off the island (deviation: the original has no ground check).
        if level.width > 0.0 && (a < level.left || b > level.left + level.width) {
            return None;
        }
        if ctx.forests.iter().any(|&(l, r)| a < r && b > l) {
            return None;
        }
        let mut existing = None;
        for o in &level.objects {
            let p = &ctx.world.protos[o.proto as usize];
            let Some(g) = p.gaps else { continue };
            if g.layers & Gaps::NOT_FARMABLE == 0 || under_construction(ctx.world, o) {
                continue;
            }
            let (l, r) = g.span(o.x, o.flip);
            if a < r && b > l {
                if o.proto == def.proto {
                    existing = Some(o.id);
                } else {
                    return None;
                }
            }
        }
        if existing.is_some() {
            return existing;
        }
        Some(self.spawn(ctx.world, def, level, x, rng))
    }

    fn spawn(
        &mut self,
        world: &WorldData,
        def: &FarmlandDef,
        level: &mut Level,
        x: f32,
        rng: &mut Rng,
    ) -> u32 {
        let p = &world.protos[def.proto as usize];
        let z = p.fixed.and_then(|f| f.fix_z).unwrap_or(p.root_z);
        let mut o = level::instantiate(world, def.proto, Layer::Game, x, 0.0, z, 1.0);
        level::randomize(world, &mut o, rng);
        // orig: Farmland.Awake hides every renderer until first farmed.
        for part in &mut o.parts {
            part.visible = false;
        }
        let id = level.add(o);
        self.lands.push(Farmland {
            object: id,
            farmhouse: None,
            child: None,
            dir: 1,
            farmable: false,
            points: 0,
            decay: 0.0,
            decayed: false,
            farmed: false,
        });
        id
    }

    fn set_frame(
        def: &FarmlandDef,
        level: &mut Level,
        object: u32,
        frame: Option<usize>,
        frozen: bool,
    ) {
        let Some(oi) = level.index_of(object) else {
            return;
        };
        let (front, back) = match frame {
            Some(f) => {
                let f = f.min(def.frames.len().saturating_sub(1));
                (def.frames.get(f).copied(), def.back_frames.get(f).copied())
            }
            None if frozen => def.frozen,
            None => def.decayed,
        };
        let parts = &mut level.objects[oi].parts;
        if let (Some(p), Some(s)) = (parts.get_mut(def.front_part as usize), front) {
            p.sprite = Some(s);
        }
        if let (Some(p), Some(s)) = (parts.get_mut(def.back_part as usize), back) {
            p.sprite = Some(s);
        }
    }

    /// Frame for the current points. orig: `stages.Evaluate(lastKeyTime × points / farmPoints)`.
    fn stage(def: &FarmlandDef, points: i32) -> usize {
        let last = def.stages.keys.last().map_or(1.0, |k| k[0]);
        let v = def
            .stages
            .eval(last * points as f32 / def.farm_points.max(1) as f32);
        libm::roundf(v).max(0.0) as usize
    }

    /// A farmer finished half a stroke on `object`. orig: `Farmland.ReceiveFarming()`.
    pub fn receive_farming(
        &mut self,
        ctx: &FarmCtx,
        level: &mut Level,
        object: u32,
        rng: &mut Rng,
    ) {
        let Some(def) = ctx.world.farmland.as_ref() else {
            return;
        };
        let Some(i) = self.land(object) else { return };
        let l = &mut self.lands[i];
        if !l.farmed {
            l.farmed = true;
            if let Some(oi) = level.index_of(object) {
                for part in &mut level.objects[oi].parts {
                    part.visible = part.sprite.is_some();
                }
            }
        }
        l.decay = def.decay_interval;
        l.decayed = false;
        l.points += 1;
        if l.points >= def.farm_points {
            if let Some(oi) = level.index_of(object) {
                let x = level.objects[oi].x;
                for k in 0..def.coin_yield {
                    self.drops.push((k as f32 * COIN_INTERVAL, x));
                }
            }
            self.lands[i].points = 0;
        }
        let l = &self.lands[i];
        let winter = ctx.winterness > 0.5;
        if let Some(h) = l.farmhouse {
            if winter || (self.too_many(ctx, level, h) && l.child.is_none()) {
                self.unlink(object);
            }
        }
        let points = self.lands[i].points;
        Self::set_frame(def, level, object, Some(Self::stage(def, points)), false);

        let l = &self.lands[i];
        let Some(house) = l.farmhouse else { return };
        if winter || !l.farmable || !self.can_spawn(ctx, level, house) {
            return;
        }
        let dir = l.dir;
        match l.child.filter(|&c| self.land(c).is_some()) {
            None => {
                let Some(oi) = level.index_of(object) else {
                    return;
                };
                let x = level.objects[oi].x + def.spacing * dir as f32;
                if let Some(f) = self.create_or_find(ctx, level, x, rng) {
                    let fh = self.land(f).and_then(|j| self.lands[j].farmhouse);
                    if fh.is_none() || fh == Some(house) {
                        let i = self.land(object).unwrap_or(i);
                        self.lands[i].child = Some(f);
                        self.link(ctx, level, f, house, dir);
                    }
                }
            }
            Some(c) => self.link(ctx, level, c, house, dir),
        }
    }

    pub fn update(
        &mut self,
        dt: f32,
        ctx: &FarmCtx,
        level: &mut Level,
        coins: &mut Coins,
        rng: &mut Rng,
    ) {
        let Some(def) = ctx.world.farmland.as_ref() else {
            return;
        };

        // Farmhouses come and go with construction. orig: Farmhouse.Start / OnDisable.
        self.houses.retain(|h| level.index_of(h.object).is_some());
        for o in &level.objects {
            if ctx.world.protos[o.proto as usize].farmhouse.is_some()
                && !under_construction(ctx.world, o)
                && !self.houses.iter().any(|h| h.object == o.id)
            {
                self.houses.push(House {
                    object: o.id,
                    left_seed: None,
                    right_seed: None,
                    t: 0.0,
                });
            }
        }

        // orig: Farmhouse.CheckSeeds(), every updateInterval.
        for hi in 0..self.houses.len() {
            let h = self.houses[hi].object;
            let Some(oi) = level.index_of(h) else {
                continue;
            };
            if under_construction(ctx.world, &level.objects[oi]) {
                continue;
            }
            let Some(hd) = ctx.world.protos[level.objects[oi].proto as usize].farmhouse else {
                continue;
            };
            self.houses[hi].t -= dt;
            if self.houses[hi].t > 0.0 {
                continue;
            }
            self.houses[hi].t = hd.update_interval;
            if ctx.winterness >= 0.5 {
                continue;
            }
            let hx = level.objects[oi].x;
            for (side, offset) in [(-1i8, hd.left_seed_offset), (1, hd.right_seed_offset)] {
                if !self.can_spawn(ctx, level, h) {
                    break;
                }
                let seed = if side < 0 {
                    self.houses[hi].left_seed
                } else {
                    self.houses[hi].right_seed
                };
                if seed.is_some_and(|s| self.land(s).is_some()) {
                    continue;
                }
                if let Some(f) = self.create_or_find(ctx, level, hx + offset, rng) {
                    if self
                        .land(f)
                        .is_some_and(|j| self.lands[j].farmhouse.is_none())
                    {
                        if side < 0 {
                            self.houses[hi].left_seed = Some(f);
                        } else {
                            self.houses[hi].right_seed = Some(f);
                        }
                        self.link(ctx, level, f, h, side);
                    }
                }
            }
        }

        // orig: Farmland.Update(): rot after decayInterval, then vanish.
        let winter = ctx.winterness > 0.5;
        let mut i = 0;
        while i < self.lands.len() {
            let l = &mut self.lands[i];
            l.decay -= dt;
            if l.decay >= 0.0 {
                i += 1;
                continue;
            }
            if !l.decayed {
                l.decay = def.decay_interval;
                l.decayed = true;
                let ripe = l.points > def.farm_points / 2;
                l.points = 0;
                let object = l.object;
                if ripe {
                    Self::set_frame(def, level, object, None, winter);
                } else {
                    Self::set_frame(def, level, object, Some(0), false);
                }
                i += 1;
            } else {
                l.decay = REMOVE_CHECK;
                let alive_child = l.child.is_some_and(|c| level.index_of(c).is_some());
                if alive_child {
                    i += 1;
                    continue;
                }
                let object = l.object;
                // orig: OnDisable → Kingdom.RemoveFarmland, Unlink.
                self.unlink(object);
                self.lands.retain(|l| l.object != object);
                if let Some(oi) = level.index_of(object) {
                    level.objects.remove(oi);
                }
                self.changed = true;
            }
        }

        // orig: Farmland.DropCoins(): one every 0.1 s, from the field.
        let (fx, fy) = def.coin_drop_force;
        for d in &mut self.drops {
            d.0 -= dt;
        }
        while let Some(k) = self.drops.iter().position(|d| d.0 <= 0.0) {
            let (_, x) = self.drops.remove(k);
            let ox = (rng.value() - 0.5) * COIN_SPREAD;
            coins.drop(
                Who::Thing,
                (x + ox, 0.0),
                (rng.range(-fx, fx), fy),
                Policy::Nobody,
                None,
                None,
            );
        }
    }

    /// Fields farmers can be sent to: (object id, x).
    pub fn farmable(&self, level: &Level) -> Vec<(u32, f32)> {
        self.lands
            .iter()
            .filter(|l| l.farmable)
            .filter_map(|l| {
                level
                    .index_of(l.object)
                    .map(|i| (l.object, level.objects[i].x))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Curve, FarmhouseDef, Proto};
    use alloc::string::ToString;
    use alloc::vec;

    fn world() -> WorldData {
        let mut w = WorldData::default();
        w.protos.push(Proto {
            name: "Farmland".to_string(),
            gaps: Some(Gaps {
                x: -1.0,
                width: 2.0,
                layers: Gaps::NOT_FARMABLE,
            }),
            ..Default::default()
        });
        w.protos.push(Proto {
            name: "Farmhouse1".to_string(),
            farmhouse: Some(FarmhouseDef {
                update_interval: 10.0,
                max_farmlands: 4,
                statue_extra_farmlands: 2,
                left_seed_offset: -1.5,
                right_seed_offset: 2.5,
                level: 1,
            }),
            gaps: Some(Gaps {
                x: -0.4,
                width: 1.8,
                layers: Gaps::NOT_FARMABLE,
            }),
            ..Default::default()
        });
        w.farmland = Some(FarmlandDef {
            proto: 0,
            farm_points: 4,
            coin_yield: 6,
            decay_interval: 240.0,
            coin_drop_force: (1.5, 3.5),
            frames: vec![],
            back_frames: vec![],
            stages: Curve::default(),
            spacing: 2.1,
            width: 2.0,
            ..Default::default()
        });
        w
    }

    #[test]
    fn farmhouse_seeds_and_fields_grow_outward_to_the_limit() {
        let w = world();
        let mut level = Level::empty(-50.0, 100.0);
        let house = level.add(level::instantiate(&w, 1, Layer::Game, 0.0, 0.0, 0.0, 1.0));
        let ctx = FarmCtx {
            world: &w,
            winterness: 0.0,
            statue: false,
            forests: &[],
        };
        let mut farms = Farms::default();
        let mut coins = Coins::default();
        let mut rng = Rng::new(1);
        farms.update(1.0 / 60.0, &ctx, &mut level, &mut coins, &mut rng);
        assert_eq!(farms.count(house), 2, "two seeds");
        // Farming the right seed spreads the chain rightward.
        for _ in 0..10 {
            let right: Vec<u32> = farms
                .farmable(&level)
                .into_iter()
                .filter(|f| f.1 > 0.0)
                .map(|f| f.0)
                .collect();
            for f in right {
                farms.receive_farming(&ctx, &mut level, f, &mut rng);
            }
        }
        assert_eq!(farms.count(house), 4, "capped at maxFarmlands");
        // A harvest drops coinYield coins over 0.6 s.
        for _ in 0..60 {
            farms.update(1.0 / 60.0, &ctx, &mut level, &mut coins, &mut rng);
        }
        assert!(coins.list.len() >= 6);
    }

    #[test]
    fn fields_do_not_overlap_buildings() {
        let w = world();
        let mut level = Level::empty(-50.0, 100.0);
        level.add(level::instantiate(&w, 1, Layer::Game, 0.0, 0.0, 0.0, 1.0));
        // Another farmhouse right where the right seed would go.
        level.add(level::instantiate(&w, 1, Layer::Game, 2.5, 0.0, 0.0, 1.0));
        let ctx = FarmCtx {
            world: &w,
            winterness: 0.0,
            statue: false,
            forests: &[],
        };
        let mut farms = Farms::default();
        let mut rng = Rng::new(1);
        let f = farms.create_or_find(&ctx, &mut level, 2.5, &mut rng);
        assert_eq!(f, None);
        let g = farms.create_or_find(&ctx, &mut level, -5.0, &mut rng);
        assert!(g.is_some());
        assert_eq!(farms.create_or_find(&ctx, &mut level, -4.5, &mut rng), g);
    }
}
