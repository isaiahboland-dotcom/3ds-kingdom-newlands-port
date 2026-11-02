//! Teleporters: grown out of a fallen portal's rift. The first trip lets
//! the monarch fly an exit point anywhere and leave a new rift there;
//! building a teleporter on that rift links the two.
//!
//! orig: `PayableTeleporter`, `TeleporterExit`, `TeleporterRift`.

use alloc::vec::Vec;

/// orig: TeleporterExit prefab `moveSpeed`, `acceleration`, `timeout`.
const EXIT_SPEED: f32 = 15.0;
const EXIT_ACCEL: f32 = 20.0;
const EXIT_TIMEOUT: f32 = 15.0;
/// orig: `TeleporterRift.destroyDelay` (rifts left by an exit).
const RIFT_LIFETIME: f32 = 30.0;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Teleporter {
    pub object: u32,
    pub linked: Option<u32>,
    pub cooldown: f32,
}

/// The exit point the monarch steers. orig: `TeleporterExit`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Exit {
    pub x: f32,
    vx: f32,
    pub t: f32,
    entrance: u32,
}

/// A rift left by an exit: (rift object, its entrance, seconds left).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingRift {
    rift: u32,
    entrance: u32,
    t: f32,
}

/// What a teleporter step asks the game to do.
pub enum TeleportAction {
    /// Put the monarch here.
    MoveMonarch(f32),
    /// Leave a rift here, linked to this entrance once it's built up.
    LeaveRift { x: f32, entrance: u32 },
    /// This rift went unused; remove it.
    RemoveRift(u32),
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Teleports {
    pub list: Vec<Teleporter>,
    pub exit: Option<Exit>,
    rifts: Vec<PendingRift>,
}

impl Teleports {
    pub fn register(&mut self, object: u32) {
        if !self.list.iter().any(|t| t.object == object) {
            self.list.push(Teleporter {
                object,
                linked: None,
                cooldown: 0.0,
            });
        }
    }

    /// orig: `PayableTeleporter.CanPay()`.
    pub fn can_pay(&self, object: u32) -> bool {
        self.exit.is_none()
            && self
                .list
                .iter()
                .any(|t| t.object == object && t.cooldown <= 0.0)
    }

    /// orig: `PayableTeleporter.Pay()`: to the linked one (returned), or fly an exit.
    pub fn pay(&mut self, object: u32, x: f32, cooldown: f32) -> Option<u32> {
        let t = self.list.iter_mut().find(|t| t.object == object)?;
        t.cooldown = cooldown;
        match t.linked {
            Some(other) => {
                let o = self.list.iter_mut().find(|t| t.object == other)?;
                o.cooldown = cooldown;
                Some(other)
            }
            None => {
                self.exit = Some(Exit {
                    x,
                    vx: 0.0,
                    t: EXIT_TIMEOUT,
                    entrance: object,
                });
                None
            }
        }
    }

    /// A rift left by the exit was built up into a teleporter: link them.
    /// orig: `TeleporterRift.HandleOnUpgraded()` → `PayableTeleporter.Link()`.
    pub fn rift_upgraded(&mut self, rift: u32, teleporter: u32, cooldown: f32) {
        let Some(i) = self.rifts.iter().position(|r| r.rift == rift) else {
            return;
        };
        let entrance = self.rifts.remove(i).entrance;
        self.register(teleporter);
        for t in &mut self.list {
            if t.object == entrance {
                t.linked = Some(teleporter);
                t.cooldown = cooldown;
            } else if t.object == teleporter {
                t.linked = Some(entrance);
            }
        }
    }

    pub fn rift_left(&mut self, rift: u32, entrance: u32) {
        self.rifts.push(PendingRift {
            rift,
            entrance,
            t: RIFT_LIFETIME,
        });
    }

    /// `direction`/`go` are the controls while the exit is flying.
    /// orig: `TeleporterExit.ReceiveInput()`, `DoTeleport()`, `AbortTeleport()`.
    pub fn update(
        &mut self,
        dt: f32,
        direction: i8,
        go: bool,
        mut act: impl FnMut(TeleportAction),
    ) {
        for t in &mut self.list {
            t.cooldown -= dt;
        }
        self.rifts.retain_mut(|r| {
            r.t -= dt;
            if r.t <= 0.0 {
                act(TeleportAction::RemoveRift(r.rift));
                false
            } else {
                true
            }
        });
        let Some(e) = &mut self.exit else { return };
        if direction != 0 {
            let target = EXIT_SPEED * direction as f32;
            e.vx = if target > 0.0 {
                target.min(e.vx + EXIT_ACCEL * dt)
            } else {
                target.max(e.vx - EXIT_ACCEL * dt)
            };
        } else {
            e.vx *= 0.8;
        }
        e.x += e.vx * dt;
        e.t -= dt;
        if go {
            act(TeleportAction::MoveMonarch(e.x));
            act(TeleportAction::LeaveRift {
                x: e.x,
                entrance: e.entrance,
            });
            self.exit = None;
        } else if e.t <= 0.0 {
            self.exit = None;
        }
    }
}
