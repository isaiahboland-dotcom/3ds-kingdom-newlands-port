//! Deer roam the forests and rabbits the thickets; archers hunt them for
//! the coins they drop.
//!
//! orig: `PopulationController` (on `Forest` and `Thicket`), `Deer`,
//! `Bunny`, their `Damageable`s.

use alloc::vec::Vec;

use crate::coins::{Coins, Policy, Who};
use crate::rng::Rng;
use crate::units::Mover;

/// orig: Forest prefab `PopulationController` (Deer).
const DEER_DENSITY: f32 = 0.027;
const DEER_WINTER_DENSITY: f32 = 0.0;
const DEER_UPDATE: f32 = 10.0;
const DEER_MIN_REGION: f32 = 20.0;
/// orig: Thicket prefab `PopulationController` (Bunny); its GAPS is 10 wide.
const BUNNY_DENSITY: f32 = 0.3;
const BUNNY_WINTER_DENSITY: f32 = 0.3;
const BUNNY_UPDATE: f32 = 25.0;
const THICKET_WIDTH: f32 = 10.0;

/// orig: Deer prefab.
const DEER_WALK: f32 = 0.5;
const DEER_RUN: f32 = 2.75;
const DEER_ALERT_RANGE: f32 = 5.0;
const DEER_THREAT_RANGE: f32 = 3.0;
const DEER_CHASE_DISTANCE: f32 = 2.5;
const DEER_COINS: u32 = 3;
const DEER_HP: i32 = 3;
const DEER_Z: f32 = 0.95;
/// orig: Bunny prefab.
const BUNNY_WALK: f32 = 0.6;
const BUNNY_HP: i32 = 1;
const BUNNY_Z: f32 = 1.25;
/// How long a dead animal lies there before it's gone.
const CORPSE_TIME: f32 = 1.0;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Species {
    Deer,
    Bunny,
}

/// orig: `Deer.State` (bunnies only wander).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnimalState {
    Wander {
        t: f32,
    },
    Look {
        t: f32,
    },
    Eat {
        t: f32,
    },
    WalkAway {
        t: f32,
    },
    /// Running to a habitat spot (`t` unused) or away for `t` seconds.
    RunAway {
        t: f32,
        to_habitat: bool,
    },
    FollowPlayer {
        t: f32,
    },
    /// Bunny: walking to a spot, then waiting `t`.
    Hop {
        walking: bool,
        t: f32,
    },
    Dead {
        t: f32,
    },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Animal {
    pub id: u32,
    pub species: Species,
    pub x: f32,
    pub z: f32,
    pub mover: Mover,
    pub hp: i32,
    pub state: AnimalState,
    /// orig: `_habitat` once set.
    habitat: Option<(f32, f32)>,
    /// Where its spawner was: the forest it came from, or its thicket.
    home_x: f32,
    thicket: Option<u32>,
    threat: Option<f32>,
    check: f32,
    pub anim_t: f32,
}

impl Animal {
    pub fn is_dead(&self) -> bool {
        matches!(self.state, AnimalState::Dead { .. })
    }

    /// Hit box for arrows, relative to the feet. orig: the prefabs' physical colliders.
    pub fn hit_box(&self) -> (f32, f32, f32, f32) {
        match self.species {
            Species::Deer => (self.x + 0.1 - 0.5, 0.0, self.x + 0.1 + 0.5, 1.0),
            Species::Bunny => (self.x - 0.22, 0.0, self.x + 0.22, 0.2),
        }
    }
}

/// What animals react to this tick.
pub struct WildCtx<'a> {
    pub forests: &'a [(f32, f32)],
    pub thickets: &'a [(u32, f32)],
    pub winterness: f32,
    pub monarch_x: f32,
    pub monarch_moving: bool,
    pub monarch_running: bool,
    /// orig: `Steed.attractsDeer` (the stag).
    pub attracts_deer: bool,
    /// Citizens that alarm deer: (x, is archer). orig: the Citizens layer.
    pub citizens: &'a [(f32, bool)],
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WildEvent {
    Killed {
        species: Species,
        x: f32,
    },
    /// A deer took a liking to the stag. orig: `Deer.FollowPlayer()` hearts.
    Hearts(f32),
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wildlife {
    pub animals: Vec<Animal>,
    /// Per forest (same order as the forest list) and per thicket.
    forest_timers: Vec<f32>,
    thicket_timers: Vec<(u32, f32)>,
    next_id: u32,
}

impl Wildlife {
    fn spawn(
        &mut self,
        species: Species,
        x: f32,
        home_x: f32,
        thicket: Option<u32>,
        rng: &mut Rng,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        self.animals.push(Animal {
            id,
            species,
            x,
            // orig: FixedTransform randomOffsetRange 0.01.
            z: match species {
                Species::Deer => DEER_Z,
                Species::Bunny => BUNNY_Z,
            } + rng.value() * 0.01,
            mover: Mover::new(),
            hp: match species {
                Species::Deer => DEER_HP,
                Species::Bunny => BUNNY_HP,
            },
            state: match species {
                Species::Deer => AnimalState::Wander { t: 0.0 },
                Species::Bunny => AnimalState::Hop {
                    walking: false,
                    t: 0.0,
                },
            },
            habitat: None,
            home_x,
            thicket,
            threat: None,
            check: 0.0,
            anim_t: 0.0,
        });
    }

    /// An arrow (or anything) hit an animal. orig: `Damageable.ReceiveDamage()`,
    /// `Deer.HandleOnReceiveDamage()` (run from the hitter), `HandleOnDeath()` (coins).
    pub fn hurt(
        &mut self,
        id: u32,
        damage: i32,
        from_x: f32,
        coins: &mut Coins,
        rng: &mut Rng,
        events: &mut impl FnMut(WildEvent),
    ) {
        let Some(a) = self.animals.iter_mut().find(|a| a.id == id) else {
            return;
        };
        if a.is_dead() {
            return;
        }
        a.hp -= damage;
        if a.hp > 0 {
            if a.species == Species::Deer {
                a.threat = Some(from_x);
                a.state = run_away(a, rng);
            }
            return;
        }
        a.state = AnimalState::Dead { t: 0.0 };
        a.mover.stop();
        events(WildEvent::Killed {
            species: a.species,
            x: a.x,
        });
        let (n, spread, up, oy) = match a.species {
            Species::Deer => (DEER_COINS, 0.7, 0.7, 0.4),
            Species::Bunny => (1, 0.5, 0.5, 0.1),
        };
        for _ in 0..n {
            coins.drop(
                Who::Wildlife,
                (a.x, oy),
                (rng.range(-spread, spread), up),
                Policy::Nobody,
                None,
                None,
            );
        }
    }

    pub fn update(
        &mut self,
        dt: f32,
        ctx: &WildCtx,
        level_bounds: (f32, f32),
        rng: &mut Rng,
        mut events: impl FnMut(WildEvent),
    ) {
        self.spawners(dt, ctx, rng);
        for a in &mut self.animals {
            step(a, dt, ctx, rng, &mut events);
            if level_bounds.1 > level_bounds.0 {
                a.x = a.x.clamp(level_bounds.0 + 0.5, level_bounds.1 - 0.5);
            }
        }
        self.animals
            .retain(|a| !matches!(a.state, AnimalState::Dead { t } if t > CORPSE_TIME));
    }

    /// orig: `PopulationController.Update()`: one more creature per interval
    /// while below `ceil(width × density)`.
    fn spawners(&mut self, dt: f32, ctx: &WildCtx, rng: &mut Rng) {
        let winter = ctx.winterness >= 0.5;
        if self.forest_timers.len() != ctx.forests.len() {
            self.forest_timers = ctx
                .forests
                .iter()
                .map(|_| DEER_UPDATE + rng.value() - 0.5)
                .collect();
        }
        for (i, &(l, r)) in ctx.forests.iter().enumerate() {
            self.forest_timers[i] -= dt;
            if self.forest_timers[i] > 0.0 {
                continue;
            }
            self.forest_timers[i] = DEER_UPDATE + rng.value() - 0.5;
            let width = r - l;
            if width <= DEER_MIN_REGION {
                continue;
            }
            let density = if winter {
                DEER_WINTER_DENSITY
            } else {
                DEER_DENSITY
            };
            let want = libm::ceilf(width * density) as usize;
            let have = self
                .animals
                .iter()
                .filter(|a| {
                    a.species == Species::Deer && !a.is_dead() && a.home_x >= l && a.home_x <= r
                })
                .count();
            if want > have {
                let x = rng.range(l, r);
                self.spawn(Species::Deer, x, x, None, rng);
            }
        }
        self.thicket_timers
            .retain(|t| ctx.thickets.iter().any(|k| k.0 == t.0));
        for &(id, x) in ctx.thickets {
            let ti = match self.thicket_timers.iter().position(|t| t.0 == id) {
                Some(i) => i,
                None => {
                    self.thicket_timers
                        .push((id, BUNNY_UPDATE + rng.value() - 0.5));
                    self.thicket_timers.len() - 1
                }
            };
            self.thicket_timers[ti].1 -= dt;
            if self.thicket_timers[ti].1 > 0.0 {
                continue;
            }
            self.thicket_timers[ti].1 = BUNNY_UPDATE + rng.value() - 0.5;
            let density = if winter {
                BUNNY_WINTER_DENSITY
            } else {
                BUNNY_DENSITY
            };
            let want = libm::ceilf(THICKET_WIDTH * density) as usize;
            let have = self
                .animals
                .iter()
                .filter(|a| a.thicket == Some(id) && !a.is_dead())
                .count();
            if want > have {
                // orig: randomPositionSpawn is off: at the thicket.
                self.spawn(Species::Bunny, x, x, Some(id), rng);
            }
        }
    }
}

/// orig: `Deer.SetHabitatIfEmpty()`: the nearest forest, else ±10.
fn deer_habitat(a: &mut Animal, forests: &[(f32, f32)]) -> (f32, f32) {
    *a.habitat.get_or_insert_with(|| {
        let x = a.x;
        let mut best = (x - 10.0, x + 10.0);
        let mut d = f32::MAX;
        for &(l, r) in forests {
            let dd = (x - r).max(l - x);
            if dd < d {
                d = dd;
                best = (l, r);
            }
        }
        best
    })
}

/// The closest thing a deer notices within `range`; `archers_only` for the
/// threat scanner. orig: `Scanner.GetClosest()` + `Deer.IsThreat()`.
fn deer_threat(a: &Animal, ctx: &WildCtx, range: f32, archers_only: bool) -> Option<f32> {
    let mut best: Option<(f32, bool)> = None;
    let mut consider = |x: f32, threat: bool| {
        let d = (x - a.x).abs();
        if d <= range && best.is_none_or(|b| d < (b.0 - a.x).abs()) {
            best = Some((x, threat));
        }
    };
    consider(ctx.monarch_x, ctx.monarch_moving && !ctx.attracts_deer);
    for &(x, archer) in ctx.citizens {
        if !archers_only || archer {
            consider(x, true);
        }
    }
    best.filter(|b| b.1).map(|b| b.0)
}

/// orig: `Deer.RunAway()` setup.
fn run_away(a: &mut Animal, rng: &mut Rng) -> AnimalState {
    let (l, r) = a.habitat.unwrap_or((a.x - 10.0, a.x + 10.0));
    let habitat_distance = (a.x - r).max(l - a.x);
    match a.threat {
        Some(tx) if !(habitat_distance > 5.0 && rng.value() < 0.2) => {
            let mut dir: i8 = if a.x - tx >= 0.0 { 1 } else { -1 };
            if (tx - a.x).abs() < DEER_CHASE_DISTANCE {
                dir = -dir;
            }
            a.mover.set_speed(DEER_RUN, dir);
            AnimalState::RunAway {
                t: -rng.range(3.0, 4.0),
                to_habitat: false,
            }
        }
        _ => {
            a.mover.set_goal(rng.range(l, r), DEER_RUN);
            AnimalState::RunAway {
                t: 0.0,
                to_habitat: true,
            }
        }
    }
}

fn step(a: &mut Animal, dt: f32, ctx: &WildCtx, rng: &mut Rng, events: &mut impl FnMut(WildEvent)) {
    let prev = core::mem::discriminant(&a.state);
    match a.species {
        Species::Bunny => bunny(a, dt, rng),
        Species::Deer => deer(a, dt, ctx, rng, events),
    }
    if let AnimalState::Dead { t } = &mut a.state {
        *t += dt;
    }
    a.mover.update(&mut a.x, dt);
    if core::mem::discriminant(&a.state) == prev {
        a.anim_t += dt;
    } else {
        a.anim_t = 0.0;
    }
}

/// orig: `Bunny.Wander()`: walk somewhere within 5 of where it appeared,
/// wait 2 s, repeat.
fn bunny(a: &mut Animal, dt: f32, rng: &mut Rng) {
    let AnimalState::Hop { walking, t } = a.state else {
        return;
    };
    let (l, r) = *a.habitat.get_or_insert((a.x - 5.0, a.x + 5.0));
    a.state = if walking {
        if a.mover.moving_to_goal {
            AnimalState::Hop { walking, t }
        } else {
            AnimalState::Hop {
                walking: false,
                t: 2.0,
            }
        }
    } else if t <= 0.0 {
        a.mover.set_goal(rng.range(l, r), BUNNY_WALK);
        AnimalState::Hop {
            walking: true,
            t: 0.0,
        }
    } else {
        AnimalState::Hop { walking, t: t - dt }
    };
}

/// orig: `Deer` FSM: Wander → Look → (FollowPlayer | Eat | WalkAway | Wander),
/// RunAway interrupting the calm states, each state falling to its next.
fn deer(a: &mut Animal, dt: f32, ctx: &WildCtx, rng: &mut Rng, events: &mut impl FnMut(WildEvent)) {
    if a.is_dead() {
        return;
    }
    a.check -= dt;
    let check = a.check <= 0.0;
    if check {
        a.check = 1.0;
    }
    let calm = matches!(
        a.state,
        AnimalState::Wander { .. }
            | AnimalState::Look { .. }
            | AnimalState::Eat { .. }
            | AnimalState::WalkAway { .. }
    );
    if check && calm {
        if let Some(tx) = deer_threat(a, ctx, DEER_THREAT_RANGE, true) {
            a.threat = Some(tx);
            deer_habitat(a, ctx.forests);
            a.state = run_away(a, rng);
            return;
        }
    }
    let wander = |a: &mut Animal, rng: &mut Rng| {
        let mut dir = a.mover.facing;
        if rng.value() < 0.3 {
            dir = -dir;
        }
        a.mover.set_speed(DEER_WALK, dir);
        AnimalState::Wander {
            t: -rng.range(3.0, 6.0),
        }
    };
    let look = |a: &mut Animal| {
        a.mover.stop();
        AnimalState::Look { t: -3.2 }
    };
    a.state = match a.state {
        AnimalState::Wander { t: 0.0 } => wander(a, rng),
        AnimalState::Wander { t } => {
            let t = t + dt;
            if t >= 0.0 {
                look(a)
            } else {
                AnimalState::Wander { t }
            }
        }
        AnimalState::Look { t } => {
            let t = t + dt;
            if t < 0.0 {
                AnimalState::Look { t }
            } else if let Some(tx) = deer_threat(a, ctx, DEER_THREAT_RANGE, true) {
                a.threat = Some(tx);
                deer_habitat(a, ctx.forests);
                run_away(a, rng)
            } else if ctx.attracts_deer && !ctx.monarch_running && (ctx.monarch_x - a.x).abs() < 4.0
            {
                events(WildEvent::Hearts(a.x));
                a.mover.set_goal(ctx.monarch_x, DEER_WALK);
                AnimalState::FollowPlayer { t: -1.5 }
            } else if rng.value() < 0.5 {
                a.mover.stop();
                AnimalState::Eat {
                    t: -rng.range(3.0, 4.0),
                }
            } else if let Some(tx) = deer_threat(a, ctx, DEER_ALERT_RANGE, false) {
                a.threat = Some(tx);
                a.mover.set_speed(DEER_WALK, if tx < a.x { 1 } else { -1 });
                AnimalState::WalkAway {
                    t: -rng.range(5.0, 8.0),
                }
            } else {
                wander(a, rng)
            }
        }
        AnimalState::Eat { t } => {
            let t = t + dt;
            if t >= 0.0 {
                look(a)
            } else {
                AnimalState::Eat { t }
            }
        }
        AnimalState::WalkAway { t } => {
            let t = t + dt;
            if t >= 0.0 {
                look(a)
            } else {
                AnimalState::WalkAway { t }
            }
        }
        AnimalState::RunAway { t, to_habitat } => {
            let t = t + dt;
            let done = if to_habitat {
                !a.mover.moving_to_goal
            } else {
                t >= 0.0
            };
            if done {
                // orig: nextState WalkAway, away from the remembered threat.
                match a.threat {
                    Some(tx) => a.mover.set_speed(DEER_WALK, if tx < a.x { 1 } else { -1 }),
                    None => a.mover.set_speed(DEER_WALK, 0),
                }
                AnimalState::WalkAway {
                    t: -rng.range(5.0, 8.0),
                }
            } else {
                AnimalState::RunAway { t, to_habitat }
            }
        }
        AnimalState::FollowPlayer { t } => {
            let t = t + dt;
            if t >= 0.0 {
                let d = (ctx.monarch_x - a.x).abs();
                a.mover.set_goal(
                    ctx.monarch_x + rng.range(-3.0, 3.0),
                    if d < 3.0 { DEER_WALK } else { DEER_RUN },
                );
                AnimalState::FollowPlayer {
                    t: -rng.range(2.0, 4.0),
                }
            } else {
                AnimalState::FollowPlayer { t }
            }
        }
        s => s,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(forests: &'a [(f32, f32)], citizens: &'a [(f32, bool)]) -> WildCtx<'a> {
        WildCtx {
            forests,
            thickets: &[],
            winterness: 0.0,
            monarch_x: -100.0,
            monarch_moving: false,
            monarch_running: false,
            attracts_deer: false,
            citizens,
        }
    }

    #[test]
    fn forests_fill_with_deer_and_deer_flee_archers() {
        let forests = [(0.0, 100.0)];
        let mut w = Wildlife::default();
        let mut rng = Rng::new(2);
        let c = ctx(&forests, &[]);
        for _ in 0..(60 * 60 * 5) {
            w.update(1.0 / 60.0, &c, (-200.0, 200.0), &mut rng, |_| {});
        }
        // ceil(100 × 0.027) = 3.
        assert_eq!(w.animals.len(), 3);
        let x = w.animals[0].x;
        let archers = [(x + 1.0, true)];
        let c = ctx(&forests, &archers);
        for _ in 0..90 {
            w.update(1.0 / 60.0, &c, (-200.0, 200.0), &mut rng, |_| {});
        }
        assert!(matches!(
            w.animals[0].state,
            AnimalState::RunAway { .. } | AnimalState::WalkAway { .. }
        ));
    }

    #[test]
    fn killing_a_deer_drops_three_coins() {
        let mut w = Wildlife::default();
        let mut rng = Rng::new(2);
        w.spawn(Species::Deer, 5.0, 5.0, None, &mut rng);
        let id = w.animals[0].id;
        let mut coins = Coins::default();
        let mut killed = 0;
        for _ in 0..3 {
            w.hurt(id, 1, 0.0, &mut coins, &mut rng, &mut |e| {
                if matches!(e, WildEvent::Killed { .. }) {
                    killed += 1
                }
            });
        }
        assert_eq!(killed, 1);
        assert_eq!(coins.list.len(), 3);
        assert!(coins.list.iter().all(|c| c.dropper == Who::Wildlife));
    }
}
