//! The monarch's purse: every coin is a little rigid body piling up inside
//! the bag. Pile it too high and coins tumble over the rim and fall out of
//! the purse for real.
//!
//! orig: `CoinBag`, `BagCoin` (Box2D bodies against the bag's colliders).
//!
//! Like Box2D, coins sleep when settled and are woken only by what touches
//! them: a coin spent or spilled disturbs its neighbours, not the whole
//! purse (waking all of a 300-coin pile costs milliseconds per tick on an
//! Old 3DS). Scratch buffers are reused between ticks, so a churning pile
//! doesn't allocate.
//! Units are the bag's own (y up, origin at the bag sprites' pivot).

use alloc::vec::Vec;

use crate::data::BagDef;
use crate::rng::Rng;

/// orig: Physics2D gravity.
const GRAVITY: f32 = 15.0;
/// orig: `CoinBag.OverflowLimit`: past this, coins stop stacking on each other.
const OVERFLOW_LIMIT: usize = 300;
/// orig: Physics2D `m_VelocityThreshold`: slower impacts don't bounce.
const BOUNCE_THRESHOLD: f32 = 1.0;
/// orig: Physics2D `m_LinearSleepTolerance`, `m_AngularSleepTolerance` (deg/s), `m_TimeToSleep`.
const SLEEP_LINEAR: f32 = 0.01;
const SLEEP_ANGULAR: f32 = 2.0;
const SLEEP_TIME: f32 = 0.5;
/// How far a coin may drift and still count as staying put.
const SLEEP_DRIFT: f32 = 0.005;
/// A coin moving faster than this wakes a sleeping one it touches.
const WAKE_SPEED: f32 = 0.05;
/// orig: the default physics material's friction (the bag's colliders).
const WALL_FRICTION: f32 = 0.4;
/// orig: `m_DefaultContactOffset` (contact slop) and `m_VelocityIterations`.
const SLOP: f32 = 0.01;
/// Speculative contacts: coins this close already count as touching, and
/// may only close the gap this step (so a resting pile can come to rest).
const MARGIN: f32 = 0.02;
const ITERATIONS: usize = 8;
/// Solves per tick (1/120 s each). Longer steps (one 1/60 s, or 1/30 s
/// every other tick) leave a resting pile with enough rocking that it never
/// falls asleep: it shimmers on screen and, never sleeping, ends up costing
/// more than this (see docs/perf.md).
const SUBSTEPS: usize = 2;
/// orig: `BagCoin.Blink()`: 0.2 s to full white, then back.
const BLINK_TIME: f32 = 0.2;

/// Which pile a coin stacks in. Back and front coins pass through each other.
/// orig: the `Debris1` / `Debris2` / `CollideWithGround` layers.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pile {
    Back,
    Front,
    /// Past the overflow limit: only the bag stops it.
    Loose,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct BagCoin {
    pub x: f32,
    pub y: f32,
    /// Degrees.
    pub angle: f32,
    vx: f32,
    vy: f32,
    /// Degrees per second.
    w: f32,
    pub pile: Pile,
    /// Index into `BagDef::coin_colors`, and the pile's shade.
    pub color: u8,
    pub shade: f32,
    still: f32,
    /// Where it was when it last counted as still (x, y, angle).
    anchor: (f32, f32, f32),
    asleep: bool,
    /// Hasn't touched anything yet. orig: `BagCoin._isNew`.
    new: bool,
    /// Seconds until a blink starts, then how far into it.
    blink_in: Option<f32>,
    pub blink: f32,
}

/// What the bag did this tick.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BagEvent {
    /// A coin landed on another. orig: `coinTouchSound`.
    Clink,
    /// A coin fell out of the purse. orig: `CoinBag.CoinFell()`.
    Spilled,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bag {
    pub coins: Vec<BagCoin>,
    /// orig: `_currentCoins` (includes coins still to drop in).
    count: u32,
    /// orig: `_coinsToAdd`.
    to_add: u32,
    spawn_t: f32,
    /// orig: `BlinkSequential` every 2 s.
    blink_t: f32,
    /// Coins added so far (picks the colour). orig: `_coins.Count` at spawn.
    spawned: u32,
    /// Last step's contact impulses, to start this step's from (warm
    /// starting). Keyed by (coin, other coin or the bag, disc, disc),
    /// sorted by key. A `BTreeMap` here cost a heap node per contact per
    /// step; a sorted flat `Vec` doesn't.
    #[cfg_attr(feature = "serde", serde(skip))]
    warm: Vec<WarmImpulse>,
    /// Reused per step (see [`Bag::step`]): the contacts found and the
    /// sweep-and-prune order.
    #[cfg_attr(feature = "serde", serde(skip))]
    scratch: Scratch,
}

/// Step scratch space, kept across ticks so the heap stops churning.
#[derive(Clone, Default, Debug, PartialEq)]
struct Scratch {
    contacts: Vec<Contact>,
    order: Vec<usize>,
}

/// The bag itself, as the "other coin" of a contact key.
const BAG_KEY: u16 = u16::MAX;

/// A warm-start entry: contact key and its last impulses (normal, tangent).
type WarmImpulse = ((u16, u16, u8, u8), (f32, f32));

#[derive(Clone, Debug, PartialEq)]
struct Contact {
    key: (u16, u16, u8, u8),
    a: usize,
    /// `None` for the bag itself.
    b: Option<usize>,
    /// Contact point and normal (from b toward a).
    px: f32,
    py: f32,
    nx: f32,
    ny: f32,
    depth: f32,
    bias: f32,
    friction: f32,
    /// Impulses so far this step. orig: Box2D clamps the accumulated
    /// impulse, not each pass's, so later passes can take back an overshoot.
    jn: f32,
    jt: f32,
}

/// The coin as three discs along its width (the collider is a flat hexagon).
fn discs(def: &BagDef) -> (f32, f32) {
    let (mut w, mut h) = (0.0f32, 0.0f32);
    for &(x, y) in &def.coin_shape {
        w = w.max(x.abs());
        h = h.max(y.abs());
    }
    if w == 0.0 {
        (w, h) = (0.114, 0.047);
    }
    (w - h, h)
}

fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
    }
    c
}

/// Closest point on the outline to (x, y).
fn closest(poly: &[(f32, f32)], x: f32, y: f32) -> (f32, f32) {
    let n = poly.len();
    let mut best = (f32::MAX, (x, y));
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l = dx * dx + dy * dy;
        let t = if l > 0.0 {
            (((x - a.0) * dx + (y - a.1) * dy) / l).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = (a.0 + dx * t, a.1 + dy * t);
        let d = (q.0 - x) * (q.0 - x) + (q.1 - y) * (q.1 - y);
        if d < best.0 {
            best = (d, q);
        }
    }
    best.1
}

impl Bag {
    /// Largest linear and angular speed of an awake coin (diagnostics).
    pub fn motion(&self) -> (f32, f32) {
        self.coins
            .iter()
            .filter(|c| !c.asleep)
            .fold((0.0f32, 0.0f32), |m, c| {
                (
                    m.0.max(libm::sqrtf(c.vx * c.vx + c.vy * c.vy)),
                    m.1.max(c.w.abs()),
                )
            })
    }

    /// (x, y, speed, spin) of the fastest coins (diagnostics).
    pub fn movers(&self) -> Vec<(f32, f32, f32, f32)> {
        let mut v: Vec<(f32, f32, f32, f32)> = self
            .coins
            .iter()
            .map(|c| (c.x, c.y, libm::sqrtf(c.vx * c.vx + c.vy * c.vy), c.w))
            .collect();
        v.sort_by(|a, b| b.2.total_cmp(&a.2));
        v.truncate(6);
        v
    }

    /// Coins still moving (the rest are asleep).
    pub fn awake(&self) -> usize {
        self.coins.iter().filter(|c| !c.asleep).count()
    }

    /// Coins in the purse as the bag shows them.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// orig: `CoinBag.ShowCoins(total)`: new coins drop in one by one,
    /// spent ones vanish (the newest first).
    fn show(&mut self, total: u32) {
        if total > self.count {
            self.to_add += total - self.count;
            self.count = total;
        } else if total < self.count {
            let mut gone = self.count - total;
            self.count = total;
            let from_queue = gone.min(self.to_add);
            self.to_add -= from_queue;
            gone -= from_queue;
            // Only the neighbours of a taken coin fall into its hole; the
            // rest of the pile keeps sleeping (Box2D wakes touching islands,
            // not the world).
            let mut holes: Vec<(f32, f32)> = Vec::new();
            for _ in 0..gone {
                if let Some(c) = self.coins.pop() {
                    holes.push((c.x, c.y));
                }
            }
            if !holes.is_empty() {
                self.warm.clear();
                self.wake_near(&holes);
            }
        }
    }

    /// Wakes coins that could rest on the holes just made at `holes`
    /// (a couple of coin widths around each).
    fn wake_near(&mut self, holes: &[(f32, f32)]) {
        const REACH: f32 = 0.3;
        for c in &mut self.coins {
            if holes
                .iter()
                .any(|&(x, y)| (c.x - x).abs() < REACH && (c.y - y).abs() < REACH)
            {
                c.asleep = false;
            }
        }
    }

    /// orig: `CoinBag.SpawnCoin()`.
    fn spawn(&mut self, def: &BagDef, rng: &mut Rng) {
        let n = self.coins.len();
        let pile = if n >= OVERFLOW_LIMIT {
            Pile::Loose
        } else if n < 8 || n.is_multiple_of(2) {
            Pile::Back
        } else {
            Pile::Front
        };
        let colors = def.coin_colors.len().max(1) as u32;
        self.coins.push(BagCoin {
            x: rng.range(-0.2, 0.2),
            y: 1.0,
            angle: rng.range_i32(-30, 30) as f32,
            vx: 0.0,
            vy: 0.0,
            w: 0.0,
            pile,
            color: (self.spawned % colors) as u8,
            shade: match pile {
                Pile::Loose => 0.8,
                Pile::Back => 0.9,
                Pile::Front => 1.0,
            },
            still: 0.0,
            anchor: (f32::MAX, f32::MAX, 0.0),
            asleep: false,
            new: true,
            blink_in: None,
            blink: 0.0,
        });
        self.spawned += 1;
    }

    /// Follows the purse (`coins`), drops new coins in, and steps the pile.
    pub fn update(
        &mut self,
        dt: f32,
        def: &BagDef,
        coins: u32,
        rng: &mut Rng,
        mut events: impl FnMut(BagEvent),
    ) {
        // Without the bag's data (tests) there's nothing to pile into.
        if def.wall.is_empty() {
            self.count = coins;
            return;
        }
        self.show(coins);
        // orig: AddCoinsOneByOne every coinSpawnInterval.
        self.spawn_t -= dt;
        if self.to_add > 0 && self.spawn_t <= 0.0 {
            self.to_add -= 1;
            self.spawn(def, rng);
            self.spawn_t = def.spawn_interval.max(0.01);
        }
        // orig: BlinkSequential: every 2 s, a run of blinks left to right while the purse is thin.
        self.blink_t += dt;
        if self.blink_t >= 2.0 {
            self.blink_t = 0.0;
            if self.count <= 8 {
                let mut order: Vec<usize> = (0..self.coins.len()).collect();
                order.sort_by(|&a, &b| self.coins[a].x.total_cmp(&self.coins[b].x));
                for (k, i) in order.into_iter().enumerate() {
                    self.coins[i].blink_in = Some(k as f32 * 0.25);
                    self.coins[i].new = false;
                }
            }
        }
        for c in &mut self.coins {
            if let Some(t) = &mut c.blink_in {
                *t -= dt;
                if *t <= 0.0 {
                    c.blink = -*t;
                    if c.blink >= BLINK_TIME {
                        c.blink_in = None;
                        c.blink = 0.0;
                        // orig: BlinkInternal: the rotation is folded into ±90°.
                        c.angle =
                            c.angle + 90.0 - 180.0 * libm::floorf((c.angle + 90.0) / 180.0) - 90.0;
                    }
                }
            }
        }
        let h = dt / SUBSTEPS as f32;
        for _ in 0..SUBSTEPS {
            crate::prof::mark("bag_int");
            self.step(h, def, &mut events);
        }
    }

    fn step(&mut self, dt: f32, def: &BagDef, events: &mut impl FnMut(BagEvent)) {
        // A settled pile stays put until something lands on it or is spent.
        if self.coins.iter().all(|c| c.asleep) {
            return;
        }
        let (half, r) = discs(def);
        let g = GRAVITY * def.gravity_scale;
        let ld = 1.0 / (1.0 + dt * def.linear_drag);
        let ad = 1.0 / (1.0 + dt * def.angular_drag);
        for c in self.coins.iter_mut().filter(|c| !c.asleep) {
            c.vy -= g * dt;
            c.vx *= ld;
            c.vy *= ld;
            c.w *= ad;
        }
        // Inertia of the flat coin about its middle (unit mass).
        let (w2, h2) = (2.0 * (half + r), 2.0 * r);
        let inertia = (w2 * w2 + h2 * h2) / 12.0;
        let centers = |c: &BagCoin| {
            let a = c.angle.to_radians();
            let (dx, dy) = (half * libm::cosf(a), half * libm::sinf(a));
            [(c.x - dx, c.y - dy), (c.x, c.y), (c.x + dx, c.y + dy)]
        };
        let e = def.bounciness;
        let mut contacts = core::mem::take(&mut self.scratch.contacts);
        contacts.clear();
        let n = self.coins.len();
        // Sweep along x: only coins within reach of each other can touch.
        let mut order = core::mem::take(&mut self.scratch.order);
        order.clear();
        order.extend(0..n);
        order.sort_unstable_by(|&a, &b| self.coins[a].x.total_cmp(&self.coins[b].x));
        crate::prof::mark("bag_sort");
        let reach = 2.0 * (half + r);
        for ii in 0..n {
            let i = order[ii];
            let ci = centers(&self.coins[i]);
            let ai = &self.coins[i];
            // The bag.
            if !ai.asleep {
                for (di, &(x, y)) in ci.iter().enumerate() {
                    let q = closest(&def.wall, x, y);
                    let (dx, dy) = (x - q.0, y - q.1);
                    let d = libm::sqrtf(dx * dx + dy * dy);
                    let into = inside(&def.wall, x, y);
                    if into || d < r + MARGIN {
                        let (nx, ny) = if d > 1e-6 {
                            if into {
                                (-dx / d, -dy / d)
                            } else {
                                (dx / d, dy / d)
                            }
                        } else {
                            (0.0, 1.0)
                        };
                        let depth = if into { d + r } else { r - d };
                        contacts.push(Contact {
                            key: (i as u16, BAG_KEY, di as u8, 0),
                            a: i,
                            b: None,
                            px: q.0,
                            py: q.1,
                            nx,
                            ny,
                            depth,
                            bias: 0.0,
                            jn: 0.0,
                            jt: 0.0,
                            friction: libm::sqrtf(def.friction * WALL_FRICTION),
                        });
                    }
                }
            }
            if ai.pile == Pile::Loose {
                continue;
            }
            // Other coins in the same pile.
            for &j in &order[ii + 1..] {
                let bj = &self.coins[j];
                if bj.x - ai.x > reach {
                    break;
                }
                if bj.pile != ai.pile || (ai.asleep && bj.asleep) {
                    continue;
                }
                if (ai.y - bj.y).abs() > reach {
                    continue;
                }
                let cj = centers(bj);
                for (d1, &(x1, y1)) in ci.iter().enumerate() {
                    for (d2i, &(x2, y2)) in cj.iter().enumerate() {
                        let (dx, dy) = (x1 - x2, y1 - y2);
                        let d2 = dx * dx + dy * dy;
                        if d2 < (2.0 * r + MARGIN) * (2.0 * r + MARGIN) {
                            let d = libm::sqrtf(d2);
                            let (nx, ny) = if d > 1e-6 {
                                (dx / d, dy / d)
                            } else {
                                (0.0, 1.0)
                            };
                            // Keyed with the lower coin first, so it matches next step.
                            let (a, b, da, db, sx, sy, s) = if i < j {
                                (i, j, d1, d2i, x2, y2, 1.0)
                            } else {
                                (j, i, d2i, d1, x1, y1, -1.0)
                            };
                            let (nx, ny) = (nx * s, ny * s);
                            contacts.push(Contact {
                                key: (a as u16, b as u16, da as u8, db as u8),
                                a,
                                b: Some(b),
                                px: sx + nx * r,
                                py: sy + ny * r,
                                nx,
                                ny,
                                depth: 2.0 * r - d,
                                bias: 0.0,
                                jn: 0.0,
                                jt: 0.0,
                                friction: def.friction,
                            });
                        }
                    }
                }
            }
        }
        // Waking: an awake coin pressing on a sleeping one.
        crate::prof::mark("bag_sweep");
        for k in &contacts {
            if let Some(b) = k.b {
                let (sa, sb) = (self.coins[k.a].asleep, self.coins[b].asleep);
                if sa != sb {
                    let mover = if sa { b } else { k.a };
                    let c = &self.coins[mover];
                    // Judged by how it left the last solve: this step's
                    // gravity alone (0.075 at 1/120 s) would read as moving,
                    // so a coin merely resting on a sleeper woke it every
                    // substep and the pile flip-flopped between asleep and
                    // awake.
                    let (vx, vy) = (c.vx / ld, c.vy / ld + g * dt);
                    if vx * vx + vy * vy > WAKE_SPEED * WAKE_SPEED {
                        self.coins[if sa { k.a } else { b }].asleep = false;
                    }
                }
            }
        }
        let vel_at = |c: &BagCoin, px: f32, py: f32| {
            let w = c.w.to_radians();
            (c.vx - w * (py - c.y), c.vy + w * (px - c.x))
        };
        // Restitution, from the approach speed before solving.
        for k in &mut contacts {
            let (va, vb) = {
                let a = &self.coins[k.a];
                let v = vel_at(a, k.px, k.py);
                let u =
                    k.b.map_or((0.0, 0.0), |b| vel_at(&self.coins[b], k.px, k.py));
                (v, u)
            };
            let vn = (va.0 - vb.0) * k.nx + (va.1 - vb.1) * k.ny;
            if k.depth < 0.0 {
                // Not touching yet: it may approach by the gap this step.
                k.bias = k.depth / dt;
            } else if vn < -BOUNCE_THRESHOLD {
                k.bias = -e * vn;
            }
            let a = &mut self.coins[k.a];
            if a.new && vn < -0.1 && k.depth >= 0.0 {
                a.new = false;
                // orig: OnCollisionEnter2D: a new coin blinks half a second after landing.
                a.blink_in = Some(0.5);
                if k.b.is_some() {
                    events(BagEvent::Clink);
                }
            }
        }
        // Warm start: each contact begins with last step's impulse.
        let mut warm = core::mem::take(&mut self.warm);
        for k in &mut contacts {
            // Only touching contacts: a gap mustn't be pushed open further.
            let Some((jn, jt)) = warm
                .binary_search_by(|&(key, _)| key.cmp(&k.key))
                .ok()
                .map(|i| warm[i].1)
                .filter(|_| k.depth >= 0.0)
            else {
                continue;
            };
            k.jn = jn;
            k.jt = jt;
            let (tx, ty) = (-k.ny, k.nx);
            let (jx, jy) = (k.nx * jn + tx * jt, k.ny * jn + ty * jt);
            for (idx, sign) in [(Some(k.a), 1.0), (k.b, -1.0)] {
                let Some(idx) = idx else { continue };
                let c = &mut self.coins[idx];
                if c.asleep {
                    continue;
                }
                let (rx, ry) = (k.px - c.x, k.py - c.y);
                c.vx += sign * jx;
                c.vy += sign * jy;
                c.w += sign * (rx * jy - ry * jx) / inertia * 57.29578;
            }
        }
        for _ in 0..ITERATIONS {
            crate::prof::mark("bag_solve");
            for k in &mut contacts {
                let im_b =
                    k.b.map_or(0.0, |b| if self.coins[b].asleep { 0.0 } else { 1.0 });
                let ii_b = if im_b > 0.0 { 1.0 / inertia } else { 0.0 };
                let im_a = if self.coins[k.a].asleep { 0.0 } else { 1.0 };
                let ii_a = if im_a > 0.0 { 1.0 / inertia } else { 0.0 };
                if im_a + im_b == 0.0 {
                    continue;
                }
                let a = &self.coins[k.a];
                let (rax, ray) = (k.px - a.x, k.py - a.y);
                let va = vel_at(a, k.px, k.py);
                let (rbx, rby, vb) = match k.b {
                    Some(b) => {
                        let c = &self.coins[b];
                        (k.px - c.x, k.py - c.y, vel_at(c, k.px, k.py))
                    }
                    None => (0.0, 0.0, (0.0, 0.0)),
                };
                let (rvx, rvy) = (va.0 - vb.0, va.1 - vb.1);
                let vn = rvx * k.nx + rvy * k.ny;
                let ra_n = rax * k.ny - ray * k.nx;
                let rb_n = rbx * k.ny - rby * k.nx;
                let kn = im_a + im_b + ra_n * ra_n * ii_a + rb_n * rb_n * ii_b;
                let total = (k.jn + (k.bias - vn) / kn).max(0.0);
                let jn = total - k.jn;
                k.jn = total;
                // Friction along the tangent, bounded by the normal impulse.
                let (tx, ty) = (-k.ny, k.nx);
                let vt = rvx * tx + rvy * ty;
                let ra_t = rax * ty - ray * tx;
                let rb_t = rbx * ty - rby * tx;
                let kt = im_a + im_b + ra_t * ra_t * ii_a + rb_t * rb_t * ii_b;
                let limit = k.friction * k.jn;
                let total = (k.jt - vt / kt).clamp(-limit, limit);
                let jt = total - k.jt;
                k.jt = total;
                let (jx, jy) = (k.nx * jn + tx * jt, k.ny * jn + ty * jt);
                let apply = |c: &mut BagCoin, s: f32, im: f32, ii: f32, rx: f32, ry: f32| {
                    c.vx += s * jx * im;
                    c.vy += s * jy * im;
                    c.w += s * (rx * jy - ry * jx) * ii * 57.29578;
                };
                apply(&mut self.coins[k.a], 1.0, im_a, ii_a, rax, ray);
                if let Some(b) = k.b {
                    apply(&mut self.coins[b], -1.0, im_b, ii_b, rbx, rby);
                }
            }
        }
        // Warm starting: keep this step's impulses, sorted by key for next
        // step's binary searches. Same buffer, so nothing is reallocated.
        warm.clear();
        warm.extend(contacts.iter().map(|k| (k.key, (k.jn, k.jt))));
        warm.sort_unstable_by_key(|&(key, _)| key);
        self.warm = warm;
        for c in self.coins.iter_mut().filter(|c| !c.asleep) {
            c.x += c.vx * dt;
            c.y += c.vy * dt;
            c.angle += c.w * dt;
        }
        // Push apart what still overlaps. orig: Box2D position iterations.
        for k in &contacts {
            // The bag doesn't give: coins pressed into it go straight back out.
            let k_push = if k.b.is_none() { 0.8 } else { 0.2 };
            let push = ((k.depth - SLOP).max(0.0) * k_push).min(0.2);
            if push <= 0.0 {
                continue;
            }
            let wa = if self.coins[k.a].asleep { 0.0 } else { 1.0 };
            let wb =
                k.b.map_or(0.0, |b| if self.coins[b].asleep { 0.0 } else { 1.0 });
            let total = wa + wb;
            if total == 0.0 {
                continue;
            }
            let a = &mut self.coins[k.a];
            a.x += k.nx * push * wa / total;
            a.y += k.ny * push * wa / total;
            if let Some(b) = k.b {
                let b = &mut self.coins[b];
                b.x -= k.nx * push * wb / total;
                b.y -= k.ny * push * wb / total;
            }
        }
        // Sleep, and coins that went over the rim.
        let mut i = 0;
        while i < self.coins.len() {
            let c = &mut self.coins[i];
            if !c.asleep {
                // Box2D's test is on speed; a coin wedged in the pile can keep a
                // little speed while going nowhere, so staying put counts too.
                let slow = c.vx * c.vx + c.vy * c.vy < SLEEP_LINEAR * SLEEP_LINEAR
                    && c.w.abs() < SLEEP_ANGULAR;
                let (ax, ay, aa) = c.anchor;
                let put = (c.x - ax).abs() < SLEEP_DRIFT
                    && (c.y - ay).abs() < SLEEP_DRIFT
                    && (c.angle - aa).abs() < SLEEP_ANGULAR * SLEEP_TIME;
                if !put {
                    c.anchor = (c.x, c.y, c.angle);
                }
                let still = slow || put;
                c.still = if still { c.still + dt } else { 0.0 };
            }
            if inside(&def.spill, c.x, c.y) || c.y < -5.0 {
                let (x, y) = (c.x, c.y);
                self.coins.remove(i);
                self.warm.clear();
                self.count = self.count.saturating_sub(1);
                events(BagEvent::Spilled);
                // The gap it left, not the whole purse.
                self.wake_near(&[(x, y)]);
                continue;
            }
            i += 1;
        }
        // orig: Box2D sleeps bodies still for `m_TimeToSleep`. (It sleeps whole
        // islands of touching bodies; here each coin sleeps alone, and is
        // woken by a moving coin landing on it or by the coins under it
        // being taken. A full purse settles completely this way.)
        for c in &mut self.coins {
            if !c.asleep && c.still >= SLEEP_TIME {
                c.sleep();
            }
        }
        // Hand the scratch buffers back for the next step.
        self.scratch.contacts = contacts;
        self.scratch.order = order;
    }
}

impl BagCoin {
    fn sleep(&mut self) {
        self.asleep = true;
        self.vx = 0.0;
        self.vy = 0.0;
        self.w = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A small cup: a U-shaped wall, with a spill zone outside and below it.
    fn cup() -> BagDef {
        BagDef {
            wall: vec![
                (0.3, 0.2),
                (0.4, 0.2),
                (0.4, -0.6),
                (-0.4, -0.6),
                (-0.4, 0.2),
                (-0.3, 0.2),
                (-0.3, -0.5),
                (0.3, -0.5),
            ],
            spill: vec![(-3.0, -3.0), (3.0, -3.0), (3.0, -0.7), (-3.0, -0.7)],
            coin_shape: vec![(-0.114, 0.0), (0.0, 0.047), (0.114, 0.0), (0.0, -0.047)],
            coin_colors: vec![[1.0, 0.8, 0.3]],
            spawn_interval: 0.15,
            gravity_scale: 0.6,
            linear_drag: 0.7,
            angular_drag: 0.05,
            friction: 0.1,
            bounciness: 0.5,
            ..Default::default()
        }
    }

    fn run(bag: &mut Bag, def: &BagDef, coins: &mut u32, seconds: f32) -> u32 {
        let mut rng = Rng::new(1);
        let mut spilled = 0;
        for _ in 0..(seconds * 60.0) as u32 {
            bag.update(1.0 / 60.0, def, *coins, &mut rng, |e| {
                if e == BagEvent::Spilled {
                    spilled += 1;
                }
            });
            *coins -= spilled.min(*coins);
            spilled = 0;
        }
        bag.count()
    }

    #[test]
    fn coins_pile_up_in_the_bag_and_spill_when_heaped() {
        let def = cup();
        let mut bag = Bag::default();
        let mut coins = 6;
        assert_eq!(run(&mut bag, &def, &mut coins, 8.0), 6);
        assert!(bag
            .coins
            .iter()
            .all(|c| c.y > -0.6 && c.y < 0.2 && c.x.abs() < 0.3));
        // Spending takes coins out.
        coins = 2;
        assert_eq!(run(&mut bag, &def, &mut coins, 1.0), 2);
        assert_eq!(bag.coins.len(), 2);
        // Heaped far past the brim, some tumble out.
        coins = 120;
        run(&mut bag, &def, &mut coins, 40.0);
        assert!(coins < 120, "nothing spilled");
        assert_eq!(bag.count(), coins);
    }

    /// A settled purse must fall asleep and stay put: a pile that keeps
    /// rocking shimmers on the touch screen and never stops costing solver
    /// time (it did with a 1/30 s step).
    #[test]
    fn a_settled_pile_sleeps_and_holds_still() {
        let def = cup();
        let mut bag = Bag::default();
        let mut coins = 12;
        run(&mut bag, &def, &mut coins, 15.0);
        assert_eq!(bag.coins.len(), 12);
        assert_eq!(bag.awake(), 0, "the pile never settled");
        let before: Vec<(f32, f32, f32)> = bag.coins.iter().map(|c| (c.x, c.y, c.angle)).collect();
        run(&mut bag, &def, &mut coins, 2.0);
        let after: Vec<(f32, f32, f32)> = bag.coins.iter().map(|c| (c.x, c.y, c.angle)).collect();
        assert_eq!(before, after, "a sleeping pile moved");
    }

    /// A full purse, heaped past the brim and spilled down to what fits,
    /// must fall asleep too. It didn't: the wake test read this substep's
    /// gravity as motion, so any awake coin resting on a sleeping one woke
    /// it, and big piles flip-flopped forever (hundreds of µs per tick on
    /// the bench host, tens of ms on an Old 3DS).
    #[test]
    fn a_full_purse_sleeps() {
        let def = cup();
        let mut bag = Bag::default();
        let mut coins = 60;
        run(&mut bag, &def, &mut coins, 30.0);
        assert!(bag.coins.len() > 20, "the cup barely holds anything");
        run(&mut bag, &def, &mut coins, 10.0);
        assert_eq!(bag.awake(), 0, "a full purse never settled");
    }
}
