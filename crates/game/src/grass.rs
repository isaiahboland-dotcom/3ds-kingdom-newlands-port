//! Grass: patches spread from seeds until something blocks them, grow
//! taller toward the middle of a meadow, wither in winter, and grow
//! thickets (where rabbits live) outside the walls. The steed eats where
//! the grass is up.
//!
//! orig: `Grass`, `World.ExpandGrass()` / `CanSpawnThicket()`,
//! `ClearSpaceOnStart` (clearGrass), `Player` ground check.

use alloc::vec::Vec;

use crate::build::under_construction;
use crate::data::{Gaps, Layer, WorldData};
use crate::kingdom::Borders;
use crate::level::{self, Level};
use crate::rng::Rng;

/// orig: `Grass.MaxStage`.
pub const MAX_STAGE: u8 = 7;
/// orig: the Grass prefab's GAPS region (x −0.05, width 0.6).
const REGION: (f32, f32) = (-0.05, 0.6);
/// orig: `Grass.SpawnThicket()` local y.
const THICKET_Y: f32 = 0.88;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    pub x: f32,
    pub stage: u8,
    pub variation: u8,
    pub alive: bool,
    /// Neighbour patch indices. orig: `leftPatch` / `rightPatch`.
    left: Option<usize>,
    right: Option<usize>,
    /// Thicket object id. orig: `_thicket`.
    pub thicket: Option<u32>,
    /// orig: `GrassUpdate()` wait.
    t: f32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meadows {
    /// Indexed by patch id; dead patches stay as tombstones.
    pub patches: Vec<Patch>,
    /// World y of the patches (the seeds' y).
    pub y: f32,
}

/// What blocks grass: NotGrassable regions other than grass, and forests.
fn blocked(world: &WorldData, level: &Level, forests: &[(f32, f32)], a: f32, b: f32) -> bool {
    if forests.iter().any(|&(l, r)| a < r && b > l) {
        return true;
    }
    level.objects.iter().any(|o| {
        let p = &world.protos[o.proto as usize];
        match p.gaps {
            Some(g) if g.layers & Gaps::NOT_GRASSABLE != 0 && !p.grass => {
                let (l, r) = g.span(o.x, o.flip);
                a < r && b > l && !under_construction(world, o)
            }
            _ => false,
        }
    })
}

impl Meadows {
    /// Takes the grass seeds out of the level and grows them to the edges.
    /// orig: `Grass.Awake()` + `World.ExpandGrass()` (unless it's winter).
    pub fn from_level(
        world: &WorldData,
        level: &mut Level,
        forests: &[(f32, f32)],
        winterness: f32,
        rng: &mut Rng,
    ) -> Self {
        let mut m = Meadows::default();
        let Some(def) = &world.grass else { return m };
        let mut i = 0;
        while i < level.objects.len() {
            let o = &level.objects[i];
            if world.protos[o.proto as usize].grass && o.layer == Layer::Game {
                m.y = o.y;
                let x = o.x;
                level.objects.remove(i);
                m.add(x, def.update_interval, rng);
                continue;
            }
            i += 1;
        }
        if winterness < 0.5 {
            let seeds = m.patches.len();
            for s in 0..seeds {
                m.expand(world, level, forests, s, true, rng);
                m.update_height(s, 1, true);
            }
        }
        m
    }

    fn add(&mut self, x: f32, interval: f32, rng: &mut Rng) -> usize {
        self.patches.push(Patch {
            x,
            stage: 0,
            variation: rng.range_i32(0, 8) as u8,
            alive: true,
            left: None,
            right: None,
            thicket: None,
            // orig: first wait is Random.value × updateInterval.
            t: rng.value() * interval,
        });
        self.patches.len() - 1
    }

    /// The live patch whose region covers `x`.
    pub fn at(&self, x: f32) -> Option<&Patch> {
        self.patches
            .iter()
            .find(|p| p.alive && x >= p.x + REGION.0 && x <= p.x + REGION.0 + REGION.1)
    }

    /// orig: `Player` ground check: grass if the patch there has grown.
    pub fn is_grass(&self, x: f32) -> bool {
        self.at(x).is_some_and(|p| p.stage > 0)
    }

    /// orig: `Grass.Expand()` / `CreateGrassPatch()`, iteratively when recursive.
    fn expand(
        &mut self,
        world: &WorldData,
        level: &Level,
        forests: &[(f32, f32)],
        start: usize,
        recursive: bool,
        rng: &mut Rng,
    ) {
        let Some(def) = &world.grass else { return };
        let spacing = def.spacing;
        let mut stack = alloc::vec![start];
        while let Some(i) = stack.pop() {
            for left in [true, false] {
                let p = &self.patches[i];
                if (left && p.left.is_some()) || (!left && p.right.is_some()) {
                    continue;
                }
                let off = if left { -spacing } else { spacing };
                let a = p.x + off + spacing * 0.25;
                let b = a + spacing * 0.5;
                let nx = p.x + off;
                // Deviation: stop at the ends of the ground (the original's bounds test never fails).
                if level.width > 0.0 && (nx < level.left || nx > level.left + level.width) {
                    continue;
                }
                if blocked(world, level, forests, a, b) {
                    continue;
                }
                let existing = self
                    .patches
                    .iter()
                    .position(|q| q.alive && a < q.x + REGION.0 + REGION.1 && b > q.x + REGION.0);
                let (n, created) = match existing {
                    Some(n) if n != i => (n, false),
                    Some(_) => continue,
                    None => (self.add(nx, def.update_interval, rng), true),
                };
                if left {
                    self.patches[n].right = Some(i);
                    self.patches[i].left = Some(n);
                } else {
                    self.patches[n].left = Some(i);
                    self.patches[i].right = Some(n);
                }
                if recursive && created {
                    stack.push(n);
                }
            }
        }
    }

    fn stage_of(&self, i: Option<usize>) -> i32 {
        i.map_or(-1, |i| self.patches[i].stage as i32)
    }

    /// orig: `Grass.UpdateHeight()`: at most one taller than the lower neighbour.
    fn update_height(&mut self, start: usize, delta: i32, recursive: bool) {
        let mut stack = alloc::vec![start];
        let mut guard = 0;
        while let Some(i) = stack.pop() {
            guard += 1;
            if guard > 100_000 {
                break;
            }
            let p = &self.patches[i];
            let cap = (self.stage_of(p.left).min(self.stage_of(p.right)) + 1).min(MAX_STAGE as i32);
            let new = cap.min(p.stage as i32 + delta).max(0) as u8;
            if new == p.stage {
                continue;
            }
            self.patches[i].stage = new;
            if recursive {
                let p = &self.patches[i];
                stack.extend(p.left);
                stack.extend(p.right);
            }
        }
    }

    /// Removes grass under a new building or field. orig: `ClearSpaceOnStart.ClearNow()`.
    pub fn clear(&mut self, a: f32, b: f32, level: &mut Level) {
        let hit: Vec<usize> = (0..self.patches.len())
            .filter(|&i| {
                let p = &self.patches[i];
                p.alive && a < p.x + REGION.0 + REGION.1 && b > p.x + REGION.0
            })
            .collect();
        for i in hit {
            self.destroy(i, level);
        }
    }

    /// orig: `Grass.OnDestroy()`.
    fn destroy(&mut self, i: usize, level: &mut Level) {
        let p = &mut self.patches[i];
        p.alive = false;
        let (l, r) = (p.left.take(), p.right.take());
        let thicket = p.thicket.take();
        if let Some(l) = l {
            self.patches[l].right = None;
            self.update_height(l, 0, false);
        }
        if let Some(r) = r {
            self.patches[r].left = None;
            self.update_height(r, 0, false);
        }
        if let Some(t) = thicket {
            if let Some(oi) = level.index_of(t) {
                level.objects.remove(oi);
            }
        }
    }

    /// Thicket objects and their x. Rabbits spawn from these.
    pub fn thickets(&self) -> impl Iterator<Item = (u32, f32)> + '_ {
        self.patches
            .iter()
            .filter(|p| p.alive)
            .filter_map(|p| p.thicket.map(|t| (t, p.x)))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        dt: f32,
        world: &WorldData,
        level: &mut Level,
        forests: &[(f32, f32)],
        borders: Borders,
        winterness: f32,
        rng: &mut Rng,
    ) {
        let Some(def) = &world.grass else { return };
        for i in 0..self.patches.len() {
            if !self.patches[i].alive {
                continue;
            }
            self.patches[i].t -= dt;
            if self.patches[i].t > 0.0 {
                continue;
            }
            self.patches[i].t = def.update_interval;
            if winterness < 0.5 {
                self.expand(world, level, forests, i, false, rng);
                self.update_height(i, 1, false);
                if self.patches[i].stage == MAX_STAGE && self.can_spawn_thicket(world, i, borders) {
                    if self.patches[i].thicket.is_none() {
                        if let Some(proto) = def.thicket {
                            let p = &world.protos[proto as usize];
                            let z = p.fixed.and_then(|f| f.fix_z).unwrap_or(p.root_z);
                            let mut o = level::instantiate(
                                world,
                                proto,
                                Layer::Game,
                                self.patches[i].x,
                                THICKET_Y,
                                z,
                                1.0,
                            );
                            level::randomize(world, &mut o, rng);
                            self.patches[i].thicket = Some(level.add(o));
                        }
                    }
                } else {
                    self.remove_thicket(i, level);
                }
            } else {
                // orig: Grass.Decay(): shrink; bare edge patches vanish.
                self.update_height(i, -1, false);
                let p = &self.patches[i];
                if p.stage == 0 && p.left.is_none() != p.right.is_none() {
                    self.destroy(i, level);
                }
            }
            if self.patches[i].stage < MAX_STAGE {
                self.remove_thicket(i, level);
            }
        }
    }

    fn remove_thicket(&mut self, i: usize, level: &mut Level) {
        if let Some(t) = self.patches[i].thicket.take() {
            if let Some(oi) = level.index_of(t) {
                level.objects.remove(oi);
            }
        }
    }

    /// orig: `World.CanSpawnThicket()`: outside the walls, `thicketSpacing` apart.
    fn can_spawn_thicket(&self, world: &WorldData, i: usize, borders: Borders) -> bool {
        let x = self.patches[i].x;
        if borders.contains(x) {
            return false;
        }
        let spacing = world.grass.as_ref().map_or(6.0, |g| g.thicket_spacing);
        !self
            .patches
            .iter()
            .enumerate()
            .any(|(j, p)| j != i && p.alive && p.thicket.is_some() && (p.x - x).abs() < spacing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{GrassDef, Proto};
    use alloc::string::ToString;
    use alloc::vec;

    fn world() -> WorldData {
        let mut w = WorldData::default();
        w.protos.push(Proto {
            name: "Grass".to_string(),
            grass: true,
            ..Default::default()
        });
        w.protos.push(Proto {
            name: "Wall".to_string(),
            gaps: Some(Gaps {
                x: -0.75,
                width: 1.5,
                layers: Gaps::NOT_GRASSABLE,
            }),
            ..Default::default()
        });
        w.grass = Some(GrassDef {
            sprites: vec![0; 64],
            spacing: 0.5,
            update_interval: 15.0,
            z: -0.01,
            thicket: None,
            thicket_spacing: 6.0,
        });
        w
    }

    #[test]
    fn grass_spreads_to_blockers_and_grows_toward_the_middle() {
        let w = world();
        let mut level = Level::empty(-50.0, 100.0);
        level.add(level::instantiate(&w, 0, Layer::Game, 0.0, 1.0, 0.0, 1.0));
        level.add(level::instantiate(&w, 1, Layer::Game, -5.0, 0.0, 0.0, 1.0));
        level.add(level::instantiate(&w, 1, Layer::Game, 5.0, 0.0, 0.0, 1.0));
        let mut rng = Rng::new(3);
        let m = Meadows::from_level(&w, &mut level, &[], 0.0, &mut rng);
        let xs: Vec<f32> = m.patches.iter().map(|p| p.x).collect();
        let (lo, hi) = xs
            .iter()
            .fold((f32::MAX, f32::MIN), |a, &x| (a.0.min(x), a.1.max(x)));
        assert!(lo > -4.5 && lo < -3.5, "{lo}");
        // orig quirk: the probe sits 0.125 right of the new patch both ways.
        assert!(hi > 3.0 && hi < 4.0, "{hi}");
        assert!(level
            .objects
            .iter()
            .all(|o| !w.protos[o.proto as usize].grass));
        // Edges stay bare; the middle grows by one per update.
        let mut m = m;
        let borders = Borders {
            left: -100.0,
            right: 100.0,
        };
        for _ in 0..(16 * 60 * 8) {
            m.update(1.0 / 60.0, &w, &mut level, &[], borders, 0.0, &mut rng);
        }
        assert_eq!(m.at(-0.1).unwrap().stage, MAX_STAGE);
        let edge = m.patches.iter().find(|p| p.x == lo).unwrap();
        assert_eq!(edge.stage, 0);
        assert!(m.is_grass(0.0) && !m.is_grass(-20.0));
    }
}
