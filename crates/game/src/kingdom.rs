//! Kingdom-wide state: where the campfire is, how far the walls reach, and
//! fair assignment of workers to jobs.
//!
//! orig: `Kingdom` (`campfirePosition`, `CalculateBordersNow`, `isSafe`),
//! `JobAssigner` (Hungarian algorithm).

use alloc::vec;
use alloc::vec::Vec;

use crate::data::WorldData;
use crate::level::Level;
use crate::rng::Rng;
use crate::units::hsb;

/// Half-width of the kingdom before any wall is built. orig: `Kingdom.minKingdomExtents` (scene).
pub const MIN_KINGDOM_EXTENTS: f32 = 4.0;

/// orig: `Kingdom.border` (`Sided<float>`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Borders {
    pub left: f32,
    pub right: f32,
}

impl Borders {
    pub fn contains(&self, x: f32) -> bool {
        x > self.left && x < self.right
    }

    /// The border on `side` (-1 left, 1 right).
    pub fn side(&self, side: i8) -> f32 {
        if side < 0 {
            self.left
        } else {
            self.right
        }
    }
}

/// The kingdom's banner colors. orig: `CoatOfArms` (colors only; the
/// pattern and emblem come with the banners).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoatOfArms {
    pub primary: [f32; 3],
    pub secondary: [f32; 3],
    pub emblem: [f32; 3],
}

/// orig: `ColorFunctions.RandomBannerColor()`.
fn banner_color(rng: &mut Rng, colors_only: bool) -> [f32; 3] {
    if rng.value() < 0.7 || colors_only {
        if rng.value() < 0.5 {
            hsb(rng.value(), 0.6, 0.8)
        } else {
            hsb(rng.value(), 0.7, 0.6)
        }
    } else {
        let v = rng.value();
        if v < 0.4 {
            [0.1; 3]
        } else if v < 0.8 {
            [0.95; 3]
        } else {
            [1.0, 0.77, 0.3]
        }
    }
}

/// orig: `ColorFunctions.ColorToHSB()`.
fn to_hsb(c: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = c;
    let max = r.max(g).max(b);
    if max <= 0.0 {
        return [0.0; 3];
    }
    let min = r.min(g).min(b);
    let d = max - min;
    let mut h = if max > min {
        if g == max {
            (b - r) / d * 60.0 + 120.0
        } else if b == max {
            (r - g) / d * 60.0 + 240.0
        } else if b > g {
            (g - b) / d * 60.0 + 360.0
        } else {
            (g - b) / d * 60.0
        }
    } else {
        0.0
    };
    if h < 0.0 {
        h += 360.0;
    }
    [h / 360.0, d / max, max]
}

/// orig: `ColorFunctions.ColorDistance()`.
fn color_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (a, b) = (to_hsb(a), to_hsb(b));
    let dh = (a[0] - b[0]).abs();
    (dh.min(1.0 - dh) + (a[1] - b[1]).abs() + 2.0 * (a[2] - b[2]).abs()) / 4.0
}

impl CoatOfArms {
    /// orig: `CoatOfArms.Randomize()`.
    pub fn random(rng: &mut Rng) -> Self {
        let primary = banner_color(rng, true);
        let mut secondary = primary;
        let mut n = 0;
        while color_distance(primary, secondary) < 0.3 {
            secondary = banner_color(rng, false);
            n += 1;
            if n > 20 {
                break;
            }
        }
        let mut emblem = primary;
        let mut best = 0.0;
        for _ in 0..4 {
            let c = banner_color(rng, false);
            let score = color_distance(primary, c) * color_distance(secondary, c);
            if score > best {
                emblem = c;
                best = score;
            }
        }
        CoatOfArms {
            primary,
            secondary,
            emblem,
        }
    }
}

/// The coat of arms' pattern and emblem, and who rides: the rest of the
/// original's `CoatOfArms` (`patternIndex`, `emblemIndex`) and `Player.Model`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub queen: bool,
    /// Into `banner_1_pattern_*` (`Holder.patterns`).
    pub pattern: u8,
    /// Into `banner_1_emblem_*` (`Holder.emblems`).
    pub emblem: u8,
}

/// orig: `Holder.patterns`, `Holder.emblems`, and how many of the emblems
/// lead the list with `includeRandomly`.
pub const PATTERNS: u8 = 14;
pub const EMBLEMS: u8 = 21;
const RANDOM_EMBLEMS: u8 = 14;

impl Look {
    /// orig: `Player.RandomAppearance()` and `CoatOfArms.Randomize()`: a
    /// king or queen, any pattern, and one of the ordinary emblems.
    pub fn random(rng: &mut Rng) -> Self {
        Look {
            queen: rng.value() < 0.5,
            pattern: (rng.value() * PATTERNS as f32) as u8 % PATTERNS,
            emblem: (rng.value() * RANDOM_EMBLEMS as f32) as u8 % RANDOM_EMBLEMS,
        }
    }
}

/// orig: `Kingdom.OnLevelLoaded()`: the object tagged "Campfire".
pub fn campfire_x(world: &WorldData, level: &Level) -> f32 {
    level
        .objects
        .iter()
        .find(|o| world.protos[o.proto as usize].tag == "Campfire")
        .map_or(0.0, |o| o.x)
}

/// The outermost wall on each side of the campfire, or the minimum extent.
/// Walls past the closest portal on a side don't count.
/// orig: `Kingdom.CalculateBorderOnSide()`.
pub fn borders(world: &WorldData, level: &Level, campfire: f32) -> Borders {
    let mut portal: [Option<f32>; 2] = [None, None];
    for o in &level.objects {
        if world.protos[o.proto as usize].portal.is_none() {
            continue;
        }
        let i = usize::from(o.x > campfire);
        if portal[i].is_none_or(|p| (o.x - campfire).abs() < (p - campfire).abs()) {
            portal[i] = Some(o.x);
        }
    }
    let mut b = Borders {
        left: campfire - MIN_KINGDOM_EXTENTS,
        right: campfire + MIN_KINGDOM_EXTENTS,
    };
    let (mut left, mut right): (Option<f32>, Option<f32>) = (None, None);
    for o in &level.objects {
        if !world.protos[o.proto as usize].wall {
            continue;
        }
        if o.x < campfire && portal[0].is_none_or(|p| p < o.x) {
            left = Some(left.map_or(o.x, |l| l.min(o.x)));
        } else if o.x > campfire && portal[1].is_none_or(|p| p > o.x) {
            right = Some(right.map_or(o.x, |r| r.max(o.x)));
        }
    }
    if let Some(l) = left {
        b.left = l;
    }
    if let Some(r) = right {
        b.right = r;
    }
    b
}

/// Minimum-cost assignment of `agents` to `jobs` (rectangular; every agent
/// gets a column, columns past `jobs` mean "no job"). Returns the job index
/// per agent (`>= jobs` when unassigned). orig: `JobAssigner.Compute()`
/// (Munkres); ties may resolve differently, the total cost is the same.
pub fn assign(agents: usize, jobs: usize, cost: impl Fn(usize, usize) -> i32) -> Vec<usize> {
    if agents == 0 {
        return Vec::new();
    }
    let n = agents;
    let m = agents.max(jobs);
    let c = |i: usize, j: usize| -> i64 {
        if j < jobs {
            cost(i, j) as i64
        } else {
            0
        }
    };
    // Shortest augmenting path with potentials (1-indexed, column 0 is a sentinel).
    let inf = i64::MAX / 4;
    let mut u = vec![0i64; n + 1];
    let mut v = vec![0i64; m + 1];
    let mut p = vec![0usize; m + 1];
    let mut way = vec![0usize; m + 1];
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0;
        let mut minv = vec![inf; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = inf;
            let mut j1 = 0;
            for j in 1..=m {
                if !used[j] {
                    let cur = c(i0 - 1, j - 1) - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut out = vec![usize::MAX; n];
    for j in 1..=m {
        if p[j] != 0 {
            out[p[j] - 1] = j - 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_minimizes_total_distance() {
        // Workers at 0 and 10, jobs at 9 and 1: crossing over would cost 18.
        let workers = [0.0f32, 10.0];
        let jobs = [9.0f32, 1.0];
        let a = assign(2, 2, |i, j| (workers[i] - jobs[j]).abs() as i32);
        assert_eq!(a, vec![1, 0]);
    }

    #[test]
    fn extra_workers_get_no_job() {
        let workers = [0.0f32, 5.0, 100.0];
        let a = assign(3, 1, |i, _| (workers[i] - 4.0).abs() as i32);
        assert_eq!(a.iter().filter(|&&j| j == 0).count(), 1);
        assert_eq!(a[1], 0);
        assert!(a[0] >= 1 && a[2] >= 1);
    }
}
