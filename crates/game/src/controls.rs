//! Platform-independent input helpers shared by every backend.

/// Two presses of the same direction within this window start a gallop.
/// orig: Rewired input behavior "Default", `_buttonDoublePressSpeed` (loader scene).
pub const DOUBLE_PRESS_WINDOW: f32 = 0.6;

/// Detects double presses of left/right. orig: Rewired
/// `GetButtonDoublePressDown(0)` / `GetNegativeButtonDoublePressDown(0)`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct DoubleTap {
    /// Seconds since the last press of [left, right].
    since_press: [f32; 2],
    prev_dir: i8,
}

impl Default for DoubleTap {
    fn default() -> Self {
        Self {
            since_press: [f32::INFINITY; 2],
            prev_dir: 0,
        }
    }
}

impl DoubleTap {
    /// `dir` is -1, 0 or 1. Returns true on the press that completes a double press.
    pub fn update(&mut self, dt: f32, dir: i8) -> bool {
        for t in &mut self.since_press {
            *t += dt;
        }
        let pressed = dir != 0 && dir != self.prev_dir;
        self.prev_dir = dir;
        if !pressed {
            return false;
        }
        let slot = &mut self.since_press[usize::from(dir > 0)];
        if *slot <= DOUBLE_PRESS_WINDOW {
            *slot = f32::INFINITY;
            true
        } else {
            *slot = 0.0;
            false
        }
    }
}

/// Everything the simulation needs from the player's controls for one tick.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default)]
pub struct Controls {
    /// -1 left, 0 none, 1 right.
    pub direction: i8,
    /// Double-tapped a direction, or pressed the sprint button.
    pub start_sprint: bool,
    /// Released the sprint button.
    pub stop_sprint: bool,
    /// Pay/drop button (A) held.
    pub pay: bool,
    /// Pay/drop button went down this tick.
    pub pay_down: bool,
    /// Coin bag tapped on the touch screen.
    pub drop_coin: bool,
}

/// Raw button state from a backend, before edge detection.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default)]
pub struct RawInput {
    /// -1 left, 0 none, 1 right.
    pub direction: i8,
    /// Sprint button (R on 3DS) held.
    pub sprint: bool,
    /// Pay button (A) held.
    pub pay: bool,
    /// Coin bag tapped this frame.
    pub bag_tap: bool,
}

/// Turns raw button state into [`Controls`]: edges, and double-tap to gallop.
/// orig: `Player.ReceiveInput()`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct ControlMapper {
    double_tap: DoubleTap,
    prev: RawInput,
}

impl ControlMapper {
    pub fn map(&mut self, dt: f32, raw: RawInput) -> Controls {
        let double = self.double_tap.update(dt, raw.direction);
        let c = Controls {
            direction: raw.direction,
            start_sprint: double || (raw.sprint && !self.prev.sprint),
            stop_sprint: !raw.sprint && self.prev.sprint,
            pay: raw.pay,
            pay_down: raw.pay && !self.prev.pay,
            drop_coin: raw.bag_tap,
        };
        self.prev = raw;
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(tap: &mut DoubleTap, seq: &[(i8, u32)]) -> std::vec::Vec<bool> {
        let mut out = std::vec::Vec::new();
        for &(dir, frames) in seq {
            for _ in 0..frames {
                out.push(tap.update(1.0 / 60.0, dir));
            }
        }
        out
    }

    #[test]
    fn quick_double_tap_triggers_once() {
        let mut tap = DoubleTap::default();
        let out = feed(&mut tap, &[(1, 5), (0, 5), (1, 30)]);
        assert_eq!(out.iter().filter(|&&b| b).count(), 1);
        assert!(out[10]);
    }

    #[test]
    fn slow_taps_do_not_trigger() {
        let mut tap = DoubleTap::default();
        let out = feed(&mut tap, &[(1, 5), (0, 40), (1, 5)]);
        assert!(!out.iter().any(|&b| b));
    }

    #[test]
    fn opposite_directions_do_not_combine() {
        let mut tap = DoubleTap::default();
        let out = feed(&mut tap, &[(1, 5), (0, 5), (-1, 5)]);
        assert!(!out.iter().any(|&b| b));
    }
}
