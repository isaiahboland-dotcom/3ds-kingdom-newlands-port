//! Day/night clock, gameplay day counter, and the daylight timer shown on the
//! bottom screen.
//!
//! orig: `Director` (clock) and `Kingdom` (daytime flag and day counter).
//! The clock is linear: every in-game hour lasts the same real time.

/// Real seconds per in-game hour. orig: `Director.secondsPerInGameHour` (scene).
pub const SECONDS_PER_HOUR: f32 = 10.0;
/// Clock time when a new run starts, on day 0. orig: `Director.startTime` (scene).
pub const START_TIME: f32 = 4.0;
/// Gameplay daytime starts after this hour. orig: `Kingdom.dayStart` (scene).
pub const DAY_START: f32 = 5.0;
/// Gameplay daytime ends at this hour. orig: `Kingdom.dayEnd` (scene).
pub const DAY_END: f32 = 21.5;
/// Hour greed waves are timed to reach the castle. orig: `Director.waveAtCastle` (scene).
pub const WAVE_AT_CASTLE: f32 = 1.0;

const DAYLIGHT_HOURS: f32 = DAY_END - DAY_START;
const NIGHT_HOURS: f32 = 24.0 - DAYLIGHT_HOURS;

/// Lighting phase. Each one blends in over its window in [`TRANSITIONS`].
/// orig: `BlenderState.PartOfDay`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartOfDay {
    Dawn,
    Day,
    Evening,
    Night,
}

/// (phase, blend start, blend end) in clock hours. orig: `Director.dawnStart` … `nightEnd` (scene).
pub const TRANSITIONS: [(PartOfDay, f32, f32); 4] = [
    (PartOfDay::Dawn, 5.0, 7.5),
    (PartOfDay::Day, 8.0, 13.0),
    (PartOfDay::Evening, 16.0, 19.0),
    (PartOfDay::Night, 21.25, 23.5),
];

/// Wraps an hour value into `[0, 24)`. orig: `Util.Time24()`.
pub fn time24(hours: f32) -> f32 {
    let t = hours % 24.0;
    if t < 0.0 {
        t + 24.0
    } else {
        t
    }
}

/// Gameplay daytime test. Both bounds are exclusive. orig: `Kingdom.Update()`.
pub fn is_daytime(time: f32) -> bool {
    time > DAY_START && time < DAY_END
}

/// The lighting phase most recently started at `time`, and how far its blend
/// has progressed (0..=1).
pub fn part_of_day(time: f32) -> (PartOfDay, f32) {
    // Before dawn starts we're still in the previous night's phase.
    let mut current = TRANSITIONS[TRANSITIONS.len() - 1];
    for tr in TRANSITIONS {
        if time >= tr.1 {
            current = tr;
        }
    }
    let (phase, start, end) = current;
    let elapsed = time24(time - start);
    (phase, (elapsed / (end - start)).min(1.0))
}

/// The in-game clock. orig: `Director._currentDay` / `_currentTime` and `Director.Update()`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    /// Day index, starting at 0. This isn't the day number shown to the player; see [`DayCounter`].
    pub day: u32,
    /// Hours into the current day, `[0, 24)`.
    pub time: f32,
    /// orig: `Director.clockSpeedModifier`.
    pub speed: f32,
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock {
    pub fn new() -> Self {
        Self {
            day: 0,
            time: START_TIME,
            speed: 1.0,
        }
    }

    /// Advances by `dt` real seconds. Returns true if the clock rolled over midnight.
    pub fn advance(&mut self, dt: f32) -> bool {
        self.time += dt * self.speed / SECONDS_PER_HOUR;
        if !self.time.is_finite() {
            self.time = 0.0;
            return false;
        }
        let days = (self.time / 24.0).max(0.0) as u32;
        if days > 0 {
            self.day = self.day.saturating_add(days);
            self.time %= 24.0;
        }
        days > 0
    }

    /// Hours since day 0, 00:00. Director events are ordered by this.
    /// orig: `DirectorEvent.date`.
    pub fn date(&self) -> f32 {
        self.day as f32 * 24.0 + self.time
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayEvent {
    /// Daytime began. `show_counter` is false on the very first morning.
    /// orig: `Kingdom.OnDayStart` + `InterfaceOverlay.ShowDayCounter`.
    Started { day: u32, show_counter: bool },
    /// Night began; the kingdom becomes unsafe. orig: `Kingdom.OnDayEnd` / `BecomeUnsafe()`.
    Ended,
}

/// Tracks the day number shown to the player, which changes at dawn, not midnight.
/// orig: `Kingdom.isDaytime` / `Kingdom.day`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DayCounter {
    pub is_daytime: bool,
    pub day: u32,
}

impl DayCounter {
    /// orig: `Kingdom.OnLevelLoaded()`.
    pub fn new(clock: &Clock) -> Self {
        let is_daytime = is_daytime(clock.time);
        let day = if is_daytime { clock.day + 1 } else { clock.day };
        Self {
            is_daytime,
            day: day.max(1),
        }
    }

    /// orig: `Kingdom.Update()`.
    pub fn update(&mut self, clock: &Clock) -> Option<DayEvent> {
        if is_daytime(clock.time) {
            if !self.is_daytime {
                self.is_daytime = true;
                self.day = clock.day + 1;
                return Some(DayEvent::Started {
                    day: self.day,
                    show_counter: clock.day > 0,
                });
            }
        } else if self.is_daytime {
            self.is_daytime = false;
            return Some(DayEvent::Ended);
        }
        None
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Celestial {
    Sun,
    Moon,
}

/// What the bottom-screen daylight bar shows. Not in the original, which
/// shows the time only through lighting and the sun/moon in the sky.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Daylight {
    pub phase: Celestial,
    /// Fraction of the current day or night still left, `1.0` down to `0.0`.
    pub remaining: f32,
}

pub fn daylight(time: f32) -> Daylight {
    if is_daytime(time) {
        Daylight {
            phase: Celestial::Sun,
            remaining: (DAY_END - time) / DAYLIGHT_HOURS,
        }
    } else {
        Daylight {
            phase: Celestial::Moon,
            remaining: time24(DAY_START - time) / NIGHT_HOURS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn full_day_is_240_real_seconds() {
        let mut clock = Clock::new();
        clock.time = 0.0;
        let mut frames = 0;
        while !clock.advance(DT) {
            frames += 1;
        }
        // 240 s at 60 fps, give or take float accumulation.
        assert!((14_390..=14_410).contains(&frames), "{frames}");
        assert_eq!(clock.day, 1);
    }

    #[test]
    fn daylight_and_night_lengths() {
        assert!(approx(DAYLIGHT_HOURS * SECONDS_PER_HOUR, 165.0));
        assert!(approx(NIGHT_HOURS * SECONDS_PER_HOUR, 75.0));
    }

    #[test]
    fn daytime_bounds_are_exclusive() {
        assert!(!is_daytime(5.0));
        assert!(is_daytime(5.01));
        assert!(is_daytime(21.49));
        assert!(!is_daytime(21.5));
    }

    #[test]
    fn day_counter_first_morning_is_day_one_without_banner() {
        let mut clock = Clock::new();
        let mut counter = DayCounter::new(&clock);
        assert_eq!(counter.day, 1);
        assert!(!counter.is_daytime);

        let mut events = std::vec::Vec::new();
        // Run two full days from 04:00 on day 0.
        for _ in 0..(2 * 240 * 60) {
            clock.advance(DT);
            if let Some(e) = counter.update(&clock) {
                events.push(e);
            }
        }
        assert_eq!(
            events,
            [
                DayEvent::Started {
                    day: 1,
                    show_counter: false
                },
                DayEvent::Ended,
                DayEvent::Started {
                    day: 2,
                    show_counter: true
                },
                DayEvent::Ended,
            ]
        );
    }

    #[test]
    fn day_number_holds_through_midnight() {
        let mut clock = Clock::new();
        clock.day = 3;
        clock.time = 23.9;
        let mut counter = DayCounter::new(&clock);
        assert_eq!(counter.day, 3);
        clock.advance(2.0); // past midnight
        assert_eq!(clock.day, 4);
        assert_eq!(counter.update(&clock), None);
        assert_eq!(counter.day, 3);
    }

    #[test]
    fn daylight_bar() {
        let d = daylight(5.01);
        assert_eq!(d.phase, Celestial::Sun);
        assert!(approx(d.remaining, 1.0));
        assert!(approx(daylight(13.25).remaining, 0.5));
        let n = daylight(21.5);
        assert_eq!(n.phase, Celestial::Moon);
        assert!(approx(n.remaining, 1.0));
        assert!(approx(daylight(1.25).remaining, 0.5));
        assert!(approx(daylight(4.999).remaining, 0.0));
    }

    #[test]
    fn part_of_day_phases() {
        assert_eq!(part_of_day(3.0).0, PartOfDay::Night);
        assert_eq!(part_of_day(6.25), (PartOfDay::Dawn, 0.5));
        assert_eq!(part_of_day(14.0), (PartOfDay::Day, 1.0));
        assert_eq!(part_of_day(17.5), (PartOfDay::Evening, 0.5));
        assert_eq!(part_of_day(22.0).0, PartOfDay::Night);
    }
}
