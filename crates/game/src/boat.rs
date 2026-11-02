//! The way off the island: the wharf where workers piece the boat together
//! from parts the monarch pays for, the push along the shore to the beach,
//! and sailing away.
//!
//! orig: `Wharf`, `WorkableWharf`, `BoatPart`, `Boat`, `BoatPushPosition`,
//! `Beach`.

use alloc::vec::Vec;

use crate::data::WorldData;
use crate::level::{self, Level};
use crate::rng::Rng;
use crate::units::Unit;

/// orig: `Boat.State` (stored on the boat's level object as `mode`).
pub mod mode {
    pub const PUSHING: u8 = 0;
    pub const WAITING: u8 = 1;
    pub const SAILING: u8 = 2;
}

/// orig: `Boat.DeckLeft` / `DeckRight` (x, and the deck height).
pub const DECK_LEFT: f32 = -1.25;
pub const DECK_RIGHT: f32 = 1.5;
pub const DECK_Y: f32 = 23.0 / 32.0;
/// orig: `Boat.SailAway()`: the monarch's place on deck.
pub const MONARCH_DECK: f32 = 1.40625;
/// orig: `BoatPushPosition.GetWorkOffset()`.
pub const PUSH_OFFSET: f32 = -5.0 / 32.0;
/// orig: `Boat.Update()`: arrived within this of the beach.
const ARRIVE_DISTANCE: f32 = 0.125;
/// orig: `Boat.Awake()`: `DOMoveY(waterHeight, 1.5)` with `Ease.OutBack`.
const FLOAT_TIME: f32 = 1.5;
/// orig: `BoatPart.Appear()`: the built part shows after the loose one flew in.
const PLACE_TIME: f32 = 0.5;
/// How high a stacked part drops from. orig: `Wharf.StackPart()`.
const DROP_HEIGHT: f32 = 2.0;

/// A part stacked at the wharf, waiting for the workers. orig: the
/// `Droppable` spawned by `Wharf.StackPart()`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoosePart {
    /// Index into `WharfDef::parts`.
    pub index: u16,
    pub x: f32,
    /// Ground-relative height of its middle.
    pub y: f32,
    vy: f32,
    /// Degrees.
    pub angle: f32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Wharf {
    pub object: u32,
    /// Toward the beach. orig: `transform.localScale.x`.
    pub facing: i8,
    /// orig: `_availableParts` (indices into `WharfDef::parts`).
    available: Vec<u16>,
    /// orig: `_stackedParts` with their loose pieces.
    pub stacked: Vec<LoosePart>,
    /// Loose parts flying into place: (part, seconds in).
    pub placing: Vec<(LoosePart, f32)>,
    /// orig: `_currentWork`.
    work: u8,
    /// orig: `_boatLaunched`.
    pub launched: bool,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct Boat {
    pub object: u32,
    /// Toward the beach. orig: `Boat.side`.
    pub dir: i8,
    pub speed: f32,
    /// Seconds since launch (it bobs down onto the water).
    pub t: f32,
    /// Seconds since it set sail (the sail's open animation).
    pub sail_t: f32,
    start_y: f32,
    /// orig: `numArchers`, `numWorkers`, `numKnights`.
    pub archers: u8,
    pub workers: u8,
    pub knights: u8,
    /// Who stands on deck, drawn with the boat (x is the deck offset).
    pub deck: Vec<Unit>,
    /// The dog jumped on (its color). orig: `Dog.JumpOnBoat()`.
    pub dog: Option<[f32; 3]>,
}

/// The wharf, its boat and the beach.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct Harbor {
    pub wharf: Option<Wharf>,
    pub boat: Option<Boat>,
    pub beach: Option<(u32, f32)>,
}

/// Something that happened at the harbor.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HarborEvent {
    /// A part was dropped at the wharf. orig: `partDropSound`.
    PartStacked,
    /// The workers fitted a part. orig: `Wharf.PlacePart()`.
    PartPlaced,
    /// orig: `Wharf.LaunchBoat()` (`boatLaunchSound`).
    Launched,
    /// orig: `Beach.BoatArrived()`.
    Arrived,
    /// orig: `Boat.SailAway()` (`sailSound`).
    SetSail,
}

/// orig: DOTween `Ease.OutBack`.
fn out_back(t: f32) -> f32 {
    let s = 1.70158;
    let t = t - 1.0;
    t * t * ((s + 1.0) * t + s) + 1.0
}

impl Harbor {
    /// Finds the beach (and a wharf, if the island starts with one).
    pub fn new(world: &WorldData, level: &mut Level, free_parts: u8, rng: &mut Rng) -> Self {
        let beach = level
            .objects
            .iter()
            .find(|o| world.protos[o.proto as usize].beach)
            .map(|o| (o.id, o.x));
        let mut h = Harbor {
            wharf: None,
            boat: None,
            beach,
        };
        let wharf = level
            .objects
            .iter()
            .find(|o| world.protos[o.proto as usize].wharf.is_some())
            .map(|o| o.id);
        if let Some(id) = wharf {
            h.wharf_built(world, level, id, free_parts, rng);
        }
        h
    }

    /// The wreck was rebuilt into a wharf. orig: `Wharf.Awake()` / `Start()`:
    /// face the beach, hide every part, stack the island's free parts.
    pub fn wharf_built(
        &mut self,
        world: &WorldData,
        level: &mut Level,
        id: u32,
        free_parts: u8,
        rng: &mut Rng,
    ) {
        let Some(wi) = level.index_of(id) else {
            return;
        };
        let o = &mut level.objects[wi];
        let Some(def) = world.protos[o.proto as usize].wharf.as_ref() else {
            return;
        };
        let facing = match self.beach {
            Some((_, bx)) if bx < o.x => -1,
            _ => 1,
        };
        o.flip = facing < 0;
        // orig: BoatPart.SetState(Available): every part hidden.
        for &(p, _, _) in &def.parts {
            if let Some(part) = o.parts.get_mut(p as usize) {
                part.visible = false;
            }
        }
        self.wharf = Some(Wharf {
            object: o.id,
            facing,
            available: (0..def.parts.len() as u16).collect(),
            stacked: Vec::new(),
            placing: Vec::new(),
            work: 0,
            launched: false,
        });
        for _ in 0..free_parts {
            self.stack_part(world, level, rng);
        }
    }

    /// orig: `Wharf.CanPay()`.
    pub fn wharf_can_pay(&self) -> bool {
        self.wharf
            .as_ref()
            .is_some_and(|w| !w.launched && (!w.available.is_empty() || w.stacked.is_empty()))
    }

    /// orig: `Beach.CanPay()`: the boat is waiting at the beach.
    pub fn beach_can_pay(&self, level: &Level) -> bool {
        self.boat_mode(level) == Some(mode::WAITING)
    }

    /// orig: `Wharf.Pay()`.
    pub fn pay_wharf(
        &mut self,
        world: &WorldData,
        level: &mut Level,
        rng: &mut Rng,
    ) -> Option<HarborEvent> {
        let w = self.wharf.as_ref()?;
        if !w.available.is_empty() {
            self.stack_part(world, level, rng);
            Some(HarborEvent::PartStacked)
        } else if w.stacked.is_empty() {
            self.launch(world, level);
            Some(HarborEvent::Launched)
        } else {
            None
        }
    }

    /// orig: `Wharf.StackPart()`: the next part drops onto the pile.
    fn stack_part(&mut self, world: &WorldData, level: &mut Level, rng: &mut Rng) {
        let Some(w) = &mut self.wharf else { return };
        let Some(index) = w.available.pop() else {
            return;
        };
        let Some(oi) = level.index_of(w.object) else {
            return;
        };
        let o = &mut level.objects[oi];
        let Some(def) = world.protos[o.proto as usize].wharf.as_ref() else {
            return;
        };
        let (_, cx, _) = def.parts[index as usize];
        w.work = 0;
        w.stacked.push(LoosePart {
            index,
            x: o.x + cx * w.facing as f32 + rng.range(-0.2, 0.2),
            y: DROP_HEIGHT,
            vy: 0.0,
            angle: rng.range_i32(-45, 45) as f32,
        });
        // orig: Kingdom.AddWork(_workable).
        o.reload = w.stacked.len() as i32;
    }

    /// orig: `Wharf.OnWorkFinish()`: every `workPerPart` strokes fit a part.
    pub fn work(&mut self, world: &WorldData, level: &mut Level) -> Option<HarborEvent> {
        let w = self.wharf.as_mut()?;
        let oi = level.index_of(w.object)?;
        let o = &mut level.objects[oi];
        let def = world.protos[o.proto as usize].wharf.as_ref()?;
        if w.stacked.is_empty() {
            return None;
        }
        w.work += 1;
        if w.work < def.work_per_part {
            return None;
        }
        w.work = 0;
        // orig: Wharf.PlacePart(): the oldest stacked part.
        let p = w.stacked.remove(0);
        w.placing.push((p, 0.0));
        o.reload = w.stacked.len() as i32;
        Some(HarborEvent::PartPlaced)
    }

    /// A troll picked up a stacked part. orig: `Wharf.HandleOnPartPickedUp()`.
    pub fn part_taken(&mut self, level: &mut Level, i: usize) {
        let Some(w) = &mut self.wharf else { return };
        if i < w.stacked.len() {
            let p = w.stacked.remove(i);
            w.available.push(p.index);
            if let Some(oi) = level.index_of(w.object) {
                level.objects[oi].reload = w.stacked.len() as i32;
            }
        }
    }

    /// orig: `Wharf.LaunchBoat()`.
    fn launch(&mut self, world: &WorldData, level: &mut Level) {
        let Some(w) = &mut self.wharf else { return };
        let Some(oi) = level.index_of(w.object) else {
            return;
        };
        w.launched = true;
        let o = &mut level.objects[oi];
        o.mode = 1;
        let Some(def) = world.protos[o.proto as usize].wharf.as_ref() else {
            return;
        };
        for p in def.parts.iter().map(|p| p.0).chain(def.skeleton) {
            if let Some(part) = o.parts.get_mut(p as usize) {
                part.visible = false;
            }
        }
        let Some(proto) = def.boat else { return };
        let s = w.facing as f32;
        let (x, y) = (o.x + def.skeleton_at.0 * s, o.y + def.skeleton_at.1);
        let layer = o.layer;
        let p = &world.protos[proto as usize];
        let z = p.fixed.and_then(|f| f.fix_z).unwrap_or(p.root_z);
        let mut b = level::instantiate(world, proto, layer, x, y, z, s);
        b.mode = mode::PUSHING;
        let object = level.add(b);
        self.boat = Some(Boat {
            object,
            dir: w.facing,
            speed: 0.0,
            t: 0.0,
            sail_t: 0.0,
            start_y: y,
            archers: 0,
            workers: 0,
            knights: 0,
            deck: Vec::new(),
            dog: None,
        });
    }

    /// Dev: every part fitted, the boat launched and pushed to just short
    /// of the beach.
    pub fn dev_finish(&mut self, world: &WorldData, level: &mut Level, rng: &mut Rng) {
        let Some(w) = &mut self.wharf else { return };
        w.available.clear();
        w.stacked.clear();
        w.placing.clear();
        if !w.launched {
            self.launch(world, level);
        }
        let _ = rng;
        if let (Some(b), Some((_, bx))) = (&self.boat, self.beach) {
            if let Some(i) = level.index_of(b.object) {
                level.objects[i].x = bx - b.dir as f32 * 0.1;
            }
        }
    }

    /// Room for another worker or knight on deck. orig: `Boat.WorkAvailable()`,
    /// `Knight.CanCharge()`.
    pub fn room(&self, world: &WorldData, level: &Level, knight: bool) -> bool {
        let Some(b) = &self.boat else { return false };
        let Some(d) = level
            .index_of(b.object)
            .and_then(|i| world.protos[level.objects[i].proto as usize].boat.as_ref())
        else {
            return false;
        };
        if knight {
            b.knights < d.max_knights
        } else {
            b.workers < d.max_workers
        }
    }

    /// Puts a unit on deck. orig: `Worker/Knight/Archer.Embark()`.
    pub fn embark(&mut self, level: &mut Level, mut u: Unit, rng: &mut Rng) {
        let Some(b) = &mut self.boat else { return };
        match u.role {
            crate::data::Role::Knight => b.knights += 1,
            crate::data::Role::Archer => b.archers += 1,
            _ => {
                b.workers += 1;
                if let Some(i) = level.index_of(b.object) {
                    level.objects[i].reload = b.workers as i32;
                }
            }
        }
        // orig: Boat.GetRandomDeckPosition (rounded to pixels).
        let x = DECK_LEFT + (DECK_RIGHT - DECK_LEFT) * rng.value();
        u.x = libm::roundf(x * 32.0) / 32.0;
        u.mover.stop();
        u.mover.facing = b.dir;
        u.ai = crate::units::Ai::Stand;
        u.anim_t = 0.0;
        b.deck.push(u);
    }

    /// orig: `Beach.Pay()` → `Boat.SailAway()`.
    pub fn sail_away(&mut self, level: &mut Level) -> Option<HarborEvent> {
        let b = self.boat.as_mut()?;
        let i = level.index_of(b.object)?;
        level.objects[i].mode = mode::SAILING;
        Some(HarborEvent::SetSail)
    }

    /// The boat's mode, if there is one.
    pub fn boat_mode(&self, level: &Level) -> Option<u8> {
        let b = self.boat.as_ref()?;
        level.index_of(b.object).map(|i| level.objects[i].mode)
    }

    /// The boat's x and direction.
    pub fn boat_at(&self, level: &Level) -> Option<(f32, i8)> {
        let b = self.boat.as_ref()?;
        level
            .index_of(b.object)
            .map(|i| (level.objects[i].x, b.dir))
    }

    /// `pushers`: workers at its push positions; `in_forest`: its prow is in
    /// a forest. orig: `Boat.Update()`, `BoatPart.Appear()`, loose parts falling.
    pub fn update(
        &mut self,
        dt: f32,
        world: &WorldData,
        level: &mut Level,
        pushers: usize,
        in_forest: bool,
        mut events: impl FnMut(HarborEvent),
    ) {
        if let Some(w) = &mut self.wharf {
            for p in &mut w.stacked {
                if p.y > 0.0 {
                    p.vy -= crate::coins::GRAVITY * dt;
                    p.y = (p.y + p.vy * dt).max(0.0);
                }
            }
            if let Some(oi) = level.index_of(w.object) {
                let o = &mut level.objects[oi];
                let def = world.protos[o.proto as usize].wharf.as_ref();
                w.placing.retain_mut(|(p, t)| {
                    *t += dt;
                    if *t < PLACE_TIME {
                        return true;
                    }
                    if let Some(&(part, _, _)) = def.and_then(|d| d.parts.get(p.index as usize)) {
                        if let Some(ps) = o.parts.get_mut(part as usize) {
                            ps.visible = !w.launched;
                        }
                    }
                    false
                });
            }
        }
        let Some(beach_x) = self.beach.map(|b| b.1) else {
            return;
        };
        let Some(b) = &mut self.boat else { return };
        let Some(oi) = level.index_of(b.object) else {
            return;
        };
        let o = &mut level.objects[oi];
        let Some(d) = world.protos[o.proto as usize].boat.as_ref() else {
            return;
        };
        b.t += dt;
        let k = (b.t / FLOAT_TIME).min(1.0);
        o.y = b.start_y + (d.water_height - b.start_y) * out_back(k);
        let s = b.dir as f32;
        match o.mode {
            mode::PUSHING => {
                if (beach_x - o.x).abs() > ARRIVE_DISTANCE {
                    b.speed = pushers as f32 * d.push_speed;
                    if in_forest {
                        b.speed *= d.forest_speed_multiplier;
                    }
                    // Don't overshoot the beach in one tick.
                    let step = (b.speed * dt).min((beach_x - o.x).abs());
                    o.x += s * step;
                } else {
                    b.speed = 0.0;
                    o.mode = mode::WAITING;
                    events(HarborEvent::Arrived);
                }
            }
            mode::SAILING => {
                b.sail_t += dt;
                b.speed = (b.speed + dt * 0.2).min(d.sail_speed);
                o.x += s * dt * b.speed;
            }
            _ => b.speed = 0.0,
        }
        for u in &mut b.deck {
            u.anim_t += dt;
        }
    }

    /// Where a deck offset is in the world (x, ground-relative y).
    pub fn deck_pos(&self, level: &Level, dx: f32) -> Option<(f32, f32)> {
        let b = self.boat.as_ref()?;
        let o = &level.objects[level.index_of(b.object)?];
        Some((o.x + dx * b.dir as f32, o.y + DECK_Y - crate::build::WALK_Y))
    }
}
