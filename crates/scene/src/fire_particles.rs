//! Fire's looping Playground emitters, separate from the save-compatible world.

use std::{
    io::{ErrorKind, Read},
    path::Path,
};

use game::{
    data::{sample_keys, Curve},
    rng::Rng,
    state::TICK,
};
use serde::{Deserialize, Serialize};

use crate::{render::Color, sheet::Sheet};

pub const FILE_NAME: &str = "fire-particles.bin";
const HEADER: [u8; 6] = *b"KNFP\x01\x00";
const MAX_BYTES: u64 = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FireParticlesDef {
    /// Fire.CreateFlames chooses the closest size, keeping prefab-list order for ties.
    pub variants: Vec<FireVariant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FireVariant {
    pub size: [f32; 2],
    /// The prefab also has an enabled SpriteRenderer on an active child.
    pub animated_sprite: bool,
    pub emitters: Vec<EmitterDef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmitterDef {
    pub offset: [f32; 3],
    pub count: u8,
    pub lifetime: [f32; 2],
    pub size: f32,
    pub scatter: [[f32; 3]; 2],
    pub velocity: [[f32; 3]; 2],
    pub gravity: [f32; 3],
    pub damping: f32,
    pub motion: [Curve; 3],
    pub frame: Curve,
    pub ambient: bool,
    pub rgb: [Vec<(f32, f32)>; 3],
    pub alpha: Vec<(f32, f32)>,
    pub random_row: bool,
    /// Named atlas tiles, one vector per row, with Unity's row 0 first.
    pub rows: Vec<Vec<String>>,
}

impl FireParticlesDef {
    pub fn encode(&self, world_id: u32) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::from(HEADER);
        bytes.extend(world_id.to_le_bytes());
        bytes.extend(postcard::to_allocvec(self).map_err(|e| e.to_string())?);
        Self::decode(&bytes, world_id)?;
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8], world_id: u32) -> Result<Self, String> {
        if bytes.len() < 10
            || bytes.len() as u64 > MAX_BYTES
            || bytes[..6] != HEADER
            || bytes[6..10] != world_id.to_le_bytes()
        {
            return Err("fire particle data belongs to a different pack or format".into());
        }
        let (def, remaining): (Self, _) =
            postcard::take_from_bytes(&bytes[10..]).map_err(|e| e.to_string())?;
        if !remaining.is_empty() {
            return Err("trailing fire particle data".into());
        }
        if !(1..=16).contains(&def.variants.len())
            || def.variants.iter().any(|variant| {
                variant.size.iter().any(|&v| !(0.0..=4.0).contains(&v))
                    || variant.emitters.len() > 4
                    || variant.emitters.iter().any(|emitter| !emitter.valid())
            })
        {
            return Err("invalid fire particle definitions".into());
        }
        Ok(def)
    }

    pub fn sprites(&self) -> impl Iterator<Item = &str> {
        self.variants
            .iter()
            .flat_map(|variant| &variant.emitters)
            .flat_map(|emitter| &emitter.rows)
            .flatten()
            .map(String::as_str)
    }

    pub fn read_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok(Some(bytes))
    }

    pub fn load(path: &Path, sheet: &Sheet, world_id: u32) -> Result<Option<Self>, String> {
        let Some(bytes) = Self::read_file(path)? else {
            return Ok(None);
        };
        let def = Self::decode(&bytes, world_id).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(missing) = def.sprites().find(|name| sheet.id(name).is_none()) {
            return Err(format!("{}: missing fire tile {missing}", path.display()));
        }
        Ok(Some(def))
    }

    pub(crate) fn nearest(&self, (x, y): (f32, f32)) -> Option<usize> {
        self.variants
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let distance = |v: &FireVariant| (x - v.size[0]).powi(2) + (y - v.size[1]).powi(2);
                distance(a).total_cmp(&distance(b))
            })
            .map(|(i, _)| i)
    }
}

impl EmitterDef {
    fn valid(&self) -> bool {
        let curve = |c: &Curve| {
            (1..=16).contains(&c.keys.len())
                && c.keys.iter().all(|k| {
                    (0.0..=1.0).contains(&k[0])
                        && k[1].abs() <= 20.0
                        && !k[2].is_nan()
                        && !k[3].is_nan()
                })
                && c.keys.windows(2).all(|w| w[0][0] < w[1][0])
        };
        (1..=64).contains(&self.count)
            && (TICK..=5.0).contains(&self.lifetime[0])
            && (self.lifetime[0]..=5.0).contains(&self.lifetime[1])
            && (0.01..=2.0).contains(&self.size)
            && (0.0..=20.0).contains(&self.damping)
            && self
                .offset
                .iter()
                .chain(self.scatter.iter().flatten())
                .chain(self.velocity.iter().flatten())
                .chain(&self.gravity)
                .all(|v| v.abs() <= 20.0)
            && self.motion.iter().all(curve)
            && curve(&self.frame)
            && self.rgb.iter().chain([&self.alpha]).all(|keys| {
                keys.len() <= 8
                    && keys
                        .iter()
                        .all(|&(t, v)| (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&v))
                    && keys.windows(2).all(|w| w[0].0 < w[1].0)
            })
            && (1..=4).contains(&self.rows.len())
            && self
                .rows
                .iter()
                .all(|row| (1..=16).contains(&row.len()) && row.iter().all(|name| !name.is_empty()))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Source {
    pub position: [f32; 3],
    pub ratio: f32,
    pub facing: f32,
}

struct Particle {
    position: [f32; 3],
    velocity: [f32; 3],
    age: f32,
    lifetime: f32,
    row: usize,
}

#[derive(Default)]
pub(crate) struct Emitters {
    variant: Option<usize>,
    emitters: Vec<Emitter>,
}

#[derive(Default)]
struct Emitter {
    particles: Vec<Particle>,
    until_birth: f32,
    ratio: f32,
}

pub(crate) struct Sample<'a> {
    pub sprite: &'a str,
    pub position: [f32; 3],
    pub ratio: f32,
    pub size: f32,
    pub tint: Color,
}

impl Emitters {
    pub fn new(def: Option<&FireParticlesDef>, size: (f32, f32)) -> Self {
        let Some((def, variant)) = def.and_then(|def| def.nearest(size).map(|i| (def, i))) else {
            return Self::default();
        };
        Self {
            variant: Some(variant),
            emitters: (0..def.variants[variant].emitters.len())
                .map(|_| Emitter::default())
                .collect(),
        }
    }

    pub fn clear(&mut self) {
        for emitter in &mut self.emitters {
            emitter.particles.clear();
            emitter.until_birth = 0.0;
        }
    }

    pub fn step(
        &mut self,
        def: &FireParticlesDef,
        source: Source,
        emitting: bool,
        wind: f32,
        rng: &mut Rng,
    ) {
        let Some(variant) = self.variant.and_then(|i| def.variants.get(i)) else {
            return;
        };
        for (emitter, profile) in self.emitters.iter_mut().zip(&variant.emitters) {
            emitter.step(profile, source, emitting, wind, rng);
        }
    }

    pub fn samples<'a>(
        &'a self,
        def: &'a FireParticlesDef,
        ambient: Color,
    ) -> impl Iterator<Item = Sample<'a>> + 'a {
        self.variant
            .and_then(|i| def.variants.get(i))
            .into_iter()
            .flat_map(move |variant| {
                self.emitters
                    .iter()
                    .zip(&variant.emitters)
                    .flat_map(move |(emitter, profile)| emitter.samples(profile, ambient))
            })
    }
}

impl Emitter {
    fn step(&mut self, def: &EmitterDef, source: Source, emitting: bool, wind: f32, rng: &mut Rng) {
        self.ratio = source.ratio;
        self.particles.retain_mut(|p| {
            p.age += TICK;
            if p.age >= p.lifetime {
                return false;
            }
            let t = p.age / p.lifetime;
            let damping = (1.0 - def.damping * TICK).max(0.0);
            // Flame.set_wind replaces the endpoint of the X velocity curve.
            let wind_adjustment = match def.motion[0].keys.as_slice() {
                [.., previous, key] => {
                    let u = ((t - previous[0]) / (key[0] - previous[0])).clamp(0.0, 1.0);
                    (wind - key[1]) * u * u * (3.0 - 2.0 * u)
                }
                _ => 0.0,
            };
            for axis in 0..3 {
                p.velocity[axis] = (p.velocity[axis] - def.gravity[axis] * TICK) * damping;
                let curve =
                    def.motion[axis].eval(t) + if axis == 0 { wind_adjustment } else { 0.0 };
                p.position[axis] += (p.velocity[axis] + curve) * TICK;
            }
            true
        });
        if !emitting {
            self.until_birth = 0.0;
            return;
        }
        self.until_birth -= TICK;
        while self.until_birth <= 0.0 {
            self.until_birth += def.lifetime[1] / f32::from(def.count);
            if self.particles.len() < usize::from(def.count) {
                self.particles.push(Particle {
                    position: std::array::from_fn(|axis| {
                        let mirror = if axis == 0 { source.facing } else { 1.0 };
                        source.position[axis]
                            + def.offset[axis] * mirror
                            + rng.range(def.scatter[0][axis], def.scatter[1][axis])
                    }),
                    velocity: std::array::from_fn(|axis| {
                        let mirror = if axis == 0 { source.facing } else { 1.0 };
                        mirror * rng.range(def.velocity[0][axis], def.velocity[1][axis])
                    }),
                    age: 0.0,
                    lifetime: rng.range(def.lifetime[0], def.lifetime[1]),
                    row: if def.random_row {
                        rng.range_i32(0, def.rows.len() as i32) as usize
                    } else {
                        0
                    },
                });
            }
        }
    }

    fn samples<'a>(
        &'a self,
        def: &'a EmitterDef,
        ambient: Color,
    ) -> impl Iterator<Item = Sample<'a>> + 'a {
        self.particles.iter().filter_map(move |p| {
            let t = p.age / p.lifetime;
            let alpha = sample_keys(&def.alpha, t);
            if alpha <= 0.0 {
                return None;
            }
            let row = &def.rows[p.row];
            let frame = (def.frame.eval(t).clamp(0.0, 1.0) * row.len() as f32) as usize;
            let [r, g, b] = def.rgb.each_ref().map(|keys| sample_keys(keys, t));
            let mut tint = Color::new(r, g, b, alpha);
            if def.ambient {
                tint = tint.times(ambient);
            }
            Some(Sample {
                sprite: &row[frame.min(row.len() - 1)],
                position: p.position,
                ratio: self.ratio,
                size: def.size,
                tint,
            })
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn fixture() -> FireParticlesDef {
        FireParticlesDef {
            variants: vec![FireVariant {
                size: [0.25, 0.25],
                animated_sprite: false,
                emitters: vec![EmitterDef {
                    offset: [0.0, 0.0, 0.1],
                    count: 4,
                    lifetime: [1.0; 2],
                    size: 0.25,
                    scatter: [[0.0; 3]; 2],
                    velocity: [[0.0, 1.0, 0.0]; 2],
                    gravity: [0.0; 3],
                    damping: 0.0,
                    motion: std::array::from_fn(|_| Curve {
                        keys: vec![[0.0; 4]],
                    }),
                    frame: Curve {
                        keys: vec![[0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0]],
                    },
                    ambient: false,
                    rgb: Default::default(),
                    alpha: vec![(0.0, 1.0), (1.0, 0.0)],
                    random_row: false,
                    rows: vec![vec!["smoke0".into(), "smoke1".into()]],
                }],
            }],
        }
    }

    #[test]
    fn stopping_emission_leaves_moving_particles_to_finish_their_own_lifetimes() {
        let def = fixture();
        let mut emitters = Emitters::new(Some(&def), (0.25, 0.25));
        let source = Source {
            position: [1.0, 2.0, -3.0],
            ratio: 1.0,
            facing: 1.0,
        };
        let mut rng = Rng::new(73);
        for _ in 0..30 {
            emitters.step(&def, source, true, 0.0, &mut rng);
        }
        let before = emitters
            .samples(&def, Color::WHITE)
            .map(|p| (p.position, p.tint.a))
            .collect::<Vec<_>>();
        assert!(before.len() >= 2);
        assert_eq!(before[0].0[2], -2.9);
        let mut next_rng = rng.clone();
        for _ in 0..12 {
            emitters.step(&def, source, false, 0.0, &mut rng);
        }
        let tail = emitters.samples(&def, Color::WHITE).collect::<Vec<_>>();
        assert_eq!(before.len(), tail.len(), "stop only suspends births");
        assert!(tail[0].position[1] > before[0].0[1]);
        assert!(tail[0].tint.a < before[0].1);
        assert_eq!(tail[0].sprite, "smoke1");
        assert_eq!(
            rng.next_u32(),
            next_rng.next_u32(),
            "the tail emits no new particles"
        );
        for _ in 0..60 {
            emitters.step(&def, source, false, 0.0, &mut rng);
        }
        assert_eq!(emitters.samples(&def, Color::WHITE).count(), 0);
    }

    #[test]
    fn sidecar_rejects_unbounded_or_damaged_emitters() {
        let mut def = fixture();
        let bytes = def.encode(17).unwrap();
        assert_eq!(FireParticlesDef::decode(&bytes, 17).unwrap(), def);
        assert!(FireParticlesDef::decode(&bytes, 18).is_err());
        def.variants[0].emitters[0].lifetime[0] = f32::NAN;
        assert!(def.encode(17).is_err());
    }
}
