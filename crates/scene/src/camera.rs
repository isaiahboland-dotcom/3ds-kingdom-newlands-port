//! MainCamera.Update/SetTarget: anticipation, target seek and pixel snap.
//! This is a presentation clock, advanced once per admitted simulation tick.
use game::animation::{sample, Key, Tangent};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read},
    path::Path,
};

pub const FILENAME: &str = "camera.bin";
const MAGIC: [u8; 4] = *b"KNCA";
const VERSION: u16 = 1;
const MAX_BYTES: u64 = 8192;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraDef {
    pub lead: bool,
    pub max_lead_speed: f32,
    pub lead_over_time: Vec<Key>,
    pub seek_speed_by_distance: Vec<Key>,
}

impl CameraDef {
    pub fn encode(&self, world_id: u32) -> Result<Vec<u8>, String> {
        if !self.valid() {
            return Err("invalid camera curves".into());
        }
        let mut bytes = Vec::from(MAGIC);
        bytes.extend(VERSION.to_le_bytes());
        bytes.extend(world_id.to_le_bytes());
        bytes.extend(postcard::to_allocvec(self).map_err(|err| err.to_string())?);
        if bytes.len() as u64 > MAX_BYTES {
            return Err("camera data exceeds the size limit".into());
        }
        Ok(bytes)
    }

    pub fn read_file(path: impl AsRef<Path>, world_id: u32) -> Result<Option<Self>, String> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| err.to_string())?;
        Self::decode(&bytes, world_id).map(Some)
    }

    fn decode(bytes: &[u8], world_id: u32) -> Result<Self, String> {
        if bytes.len() < 10
            || bytes.len() as u64 > MAX_BYTES
            || bytes[..4] != MAGIC
            || bytes[4..6] != VERSION.to_le_bytes()
            || bytes[6..10] != world_id.to_le_bytes()
        {
            return Err("camera data header does not match this world".into());
        }
        let (definition, rest): (Self, _) =
            postcard::take_from_bytes(&bytes[10..]).map_err(|err| err.to_string())?;
        if !rest.is_empty() || !definition.valid() {
            return Err("invalid camera curves".into());
        }
        Ok(definition)
    }

    fn valid(&self) -> bool {
        self.max_lead_speed.is_finite()
            && self.max_lead_speed >= 0.0
            && [&self.lead_over_time, &self.seek_speed_by_distance]
                .into_iter()
                .all(|keys| {
                    !keys.is_empty()
                        && keys.windows(2).all(|pair| pair[0].time <= pair[1].time)
                        && keys.iter().all(|key| {
                            key.time.is_finite()
                                && key.time >= 0.0
                                && key.value.is_finite()
                                && [key.in_tangent, key.out_tangent]
                                    .into_iter()
                                    .all(|t| match t {
                                        Tangent::Slope(v) => v.is_finite(),
                                        Tangent::Step => true,
                                    })
                        })
                })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Monarch,
    Teleporter,
    Boat(u32),
    Crown,
    /// Unity retains the target/seek coordinates when its object is inactive.
    Inactive,
    None,
}

#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub subject: Subject,
    pub x: f32,
    pub direction: f32,
    pub lead: bool,
}

#[derive(Clone, Debug)]
pub struct Camera {
    subject: Subject,
    use_lead: bool,
    target_x: f32,
    position: f32,
    previous_direction: f32,
    lead_time: f32,
    lead_offset: f32,
    seek_offset: f32,
    pan: Option<(f32, f32)>,
}

impl Camera {
    pub fn new(target: Target) -> Self {
        Self {
            subject: target.subject,
            use_lead: target.lead,
            target_x: target.x,
            position: snap(target.x),
            previous_direction: 0.0,
            lead_time: 0.0,
            lead_offset: 0.0,
            seek_offset: 0.0,
            pan: None,
        }
    }

    pub fn position(&self) -> f32 {
        self.position
    }

    /// MainCamera.PanWithDuration; the game may explicitly take camera control.
    pub fn pan(&mut self, from: f32, to: f32, seconds: f32) {
        self.target_x = from;
        if seconds > 0.0 && seconds.is_finite() && from.is_finite() && to.is_finite() {
            self.pan = Some((to, (to - from) / seconds));
        } else {
            self.target_x = to;
            self.pan = None;
        }
    }

    pub fn step(&mut self, def: &CameraDef, target: Target, dt: f32) -> f32 {
        if dt <= 0.0 || !dt.is_finite() {
            return self.position;
        }
        if (target.subject != self.subject || target.lead != self.use_lead)
            && target.subject != Subject::Inactive
        {
            self.subject = target.subject;
            self.use_lead = target.lead;
            self.pan = None;
            self.lead_time = 0.0;
            self.lead_offset = 0.0;
            // SetTarget(null) keeps the previous target coordinate and clears seek.
            self.seek_offset = if target.subject == Subject::None {
                0.0
            } else {
                self.position - target.x
            };
        }
        if let Some((to, speed)) = self.pan {
            let before = self.target_x;
            self.target_x += dt * speed;
            if (to - before) * (to - self.target_x) <= 0.0 {
                self.target_x = to;
                self.pan = None;
            }
        } else if !matches!(target.subject, Subject::None | Subject::Inactive) {
            self.target_x = target.x;
            if def.lead && target.lead {
                if target.direction != self.previous_direction {
                    self.lead_time = 0.0;
                    self.previous_direction = target.direction;
                } else {
                    self.lead_time += dt;
                }
                let desired = sample(&def.lead_over_time, self.lead_time, 0.0) * target.direction;
                let max = dt * def.max_lead_speed.max(0.0);
                self.lead_offset += (desired - self.lead_offset).clamp(-max, max);
            }
        }
        if self.seek_offset != 0.0 {
            let speed = sample(&def.seek_speed_by_distance, self.seek_offset.abs(), 1.0).max(0.0);
            self.seek_offset -= self.seek_offset.clamp(-speed * dt, speed * dt);
            self.lead_time = 0.0;
        }
        self.position = snap(self.target_x + self.seek_offset + self.lead_offset);
        self.position
    }
}

fn snap(x: f32) -> f32 {
    // Mathf.Round uses ties-to-even, at the PC's 32 pixels per world unit.
    (x * 32.0).round_ties_even() / 32.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use game::animation::{Key, Tangent};
    fn key(time: f32, value: f32, input: f32, out: f32) -> Key {
        Key {
            time,
            value,
            in_tangent: Tangent::Slope(input),
            out_tangent: Tangent::Slope(out),
        }
    }
    pub(super) fn definition() -> CameraDef {
        CameraDef {
            lead: true,
            max_lead_speed: 5.0,
            lead_over_time: vec![
                key(0., 0., 0., 0.),
                key(3., 0., 0., 0.6183402),
                key(6., 2., -0.023091942, -0.023091942),
            ],
            seek_speed_by_distance: vec![
                key(0., 1., 0., 0.),
                key(1.2, 2.5, 0., 0.),
                key(5., 2.5, 0., 0.),
                key(12., 30., 0., 0.),
            ],
        }
    }
    fn target(x: f32, direction: f32) -> Target {
        Target {
            subject: Subject::Monarch,
            x,
            direction,
            lead: true,
        }
    }

    #[test]
    fn camera_sidecar_rejects_wrong_world_and_trailing_bytes() {
        let def = definition();
        let mut bytes = def.encode(17).unwrap();
        assert_eq!(CameraDef::decode(&bytes, 17).unwrap(), def);
        assert!(CameraDef::decode(&bytes, 18).is_err());
        bytes.push(0);
        assert!(CameraDef::decode(&bytes, 17).is_err());
    }

    #[test]
    fn authored_camera_waits_then_leads_even_when_stopped_and_reverses_at_rate_limit() {
        let def = definition();
        let mut c = Camera::new(target(0., 1.));
        // PC's lead depends on facing duration, not velocity: walk and gallop
        // translate the target without changing camera-to-target displacement.
        for i in 0..180 {
            assert_eq!(
                c.step(&def, target(i as f32 / 32., 1.), 1. / 60.),
                i as f32 / 32.
            );
        }
        for _ in 0..181 {
            c.step(&def, target(10., 1.), 1. / 60.);
        }
        assert!((c.position() - 12.).abs() < 0.04);
        let before = c.position();
        c.step(&def, target(10., -1.), 1. / 60.);
        assert!(before - c.position() > 0.0 && before - c.position() <= 3. / 32.);
        assert_eq!(c.step(&def, target(-100., -1.), 0.), c.position());
    }
    #[test]
    fn same_target_lead_change_resets_anticipation_and_seeks_from_current_view() {
        let def = definition();
        let mut c = Camera::new(target(0., 1.));
        for _ in 0..362 {
            c.step(&def, target(0., 1.), 1. / 60.);
        }
        assert_eq!(c.position(), 2.);
        let no_lead = Target {
            lead: false,
            ..target(0., 1.)
        };
        let first = c.step(&def, no_lead, 1. / 60.);
        assert!(first > 1.9 && first < 2.0);
        for _ in 0..180 {
            c.step(&def, no_lead, 1. / 60.);
        }
        assert_eq!(c.position(), 0.);
    }

    #[test]
    fn target_switch_seek_pan_and_crown_follow_use_pc_rules() {
        let def = definition();
        let mut c = Camera::new(target(0., 1.));
        let exit = Target {
            subject: Subject::Teleporter,
            x: 12.,
            direction: 1.,
            lead: false,
        };
        assert_eq!(c.step(&def, exit, 1. / 60.), 0.5); // seek curve(12)=30
        let first = c.position();
        assert!(first < exit.x);
        let inactive = Target {
            subject: Subject::Inactive,
            ..exit
        };
        let next = c.step(&def, inactive, 1. / 60.);
        assert!(
            next > first && next < 2.0,
            "despawn retains the ongoing seek"
        );
        for _ in 0..600 {
            c.step(&def, exit, 1. / 60.);
        }
        assert_eq!(c.position(), 12.);
        let crown = Target {
            subject: Subject::Crown,
            x: 13.,
            direction: 1.,
            lead: false,
        };
        assert!(c.step(&def, crown, 1. / 60.) < 13.);
        for _ in 0..120 {
            c.step(&def, crown, 1. / 60.);
        }
        assert_eq!(c.position(), 13.);
        c.pan(13., 15., 1.);
        for _ in 0..62 {
            c.step(&def, crown, 1. / 60.);
            if c.pan.is_none() {
                break;
            }
        }
        // End the test at the crossing boundary, before follow resumes.
        assert!((c.position() - 15.).abs() < 0.04);
        assert_eq!(c.step(&def, crown, 1. / 60.), 13.);
        assert_eq!(snap(0.5 / 32.), 0.);
        assert_eq!(snap(1.5 / 32.), 2. / 32.);
    }
}
