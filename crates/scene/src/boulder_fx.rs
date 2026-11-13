//! The two authored one-shot Playground bursts used by the PC Boulder.
//!
//! Game events supply the impact position and yaw. Particle randomness and
//! clocks belong to the scene, independent of gameplay RNG and persistence.

use game::rng::Rng;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read},
    path::Path,
};

pub const FILENAME: &str = "boulder-effects.bin";
const MAGIC: [u8; 4] = *b"KNBE";
const VERSION: u16 = 1;
const MAX_BYTES: u64 = 8192;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BoulderFxDef {
    pub dust: BoulderEmitterDef,
    pub fragments: BoulderEmitterDef,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BoulderEmitterDef {
    pub count: u8,
    pub sprites: Vec<String>,
    /// Dust advances through the row, while each fragment selects a row at birth.
    pub animate: bool,
    pub size: f32,
    pub lifetime: [f32; 2],
    pub scatter_min: [f32; 3],
    pub scatter_max: [f32; 3],
    pub velocity_min: [f32; 2],
    pub velocity_max: [f32; 2],
    /// Playground subtracts this vector in each update.
    pub gravity: [f32; 2],
    pub damping: f32,
    pub rotation: [f32; 2],
    pub spin: [f32; 2],
    pub colors: Vec<BoulderGradientDef>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct BoulderGradientDef {
    pub color: [f32; 4],
    /// Fraction of the particle lifetime when its alpha begins to fade to zero.
    pub fade_start: f32,
}

impl BoulderFxDef {
    pub fn valid(&self) -> bool {
        [&self.dust, &self.fragments]
            .into_iter()
            .all(|emitter| emitter.valid())
    }
}

impl BoulderEmitterDef {
    fn valid(&self) -> bool {
        let range = |[min, max]: [f32; 2]| {
            min.is_finite() && max.is_finite() && min <= max && min >= -1000.0 && max <= 1000.0
        };
        (1..=32).contains(&self.count)
            && (1..=16).contains(&self.sprites.len())
            && self
                .sprites
                .iter()
                .all(|name| !name.is_empty() && name.len() <= 100)
            && self.size.is_finite()
            && (0.01..=2.0).contains(&self.size)
            && range(self.lifetime)
            && self.lifetime[0] > 0.0
            && self.lifetime[1] <= 5.0
            && self
                .scatter_min
                .into_iter()
                .zip(self.scatter_max)
                .all(|(min, max)| range([min, max]))
            && self
                .velocity_min
                .into_iter()
                .zip(self.velocity_max)
                .all(|(min, max)| range([min, max]))
            && self
                .gravity
                .into_iter()
                .all(|g| g.is_finite() && g.abs() <= 100.0)
            && (0.0..=10.0).contains(&self.damping)
            && range(self.rotation)
            && range(self.spin)
            && (1..=8).contains(&self.colors.len())
            && self.colors.iter().all(|gradient| {
                gradient.color.iter().all(|v| (0.0..=1.0).contains(v))
                    && (0.0..1.0).contains(&gradient.fade_start)
            })
    }
}

impl BoulderFxDef {
    pub fn encode(&self, world_id: u32) -> Result<Vec<u8>, String> {
        let mut data = Vec::from(MAGIC);
        data.extend(VERSION.to_le_bytes());
        data.extend(world_id.to_le_bytes());
        data.extend(postcard::to_allocvec(self).map_err(|e| e.to_string())?);
        Self::decode(&data, world_id)?;
        Ok(data)
    }

    fn decode(bytes: &[u8], world_id: u32) -> Result<Self, String> {
        if bytes.len() < 10
            || bytes.len() as u64 > MAX_BYTES
            || bytes[..4] != MAGIC
            || bytes[4..6] != VERSION.to_le_bytes()
            || bytes[6..10] != world_id.to_le_bytes()
        {
            return Err("invalid Boulder effect header".into());
        }
        let (definition, rest): (Self, _) =
            postcard::take_from_bytes(&bytes[10..]).map_err(|e| e.to_string())?;
        if !rest.is_empty() || !definition.valid() {
            return Err("invalid Boulder effect definition".into());
        }
        Ok(definition)
    }

    pub fn read(path: impl AsRef<Path>, world_id: u32) -> Result<Option<Self>, String> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Self::decode(&bytes, world_id).map(Some)
    }

    pub fn sprites(&self) -> impl Iterator<Item = &str> {
        [&self.dust, &self.fragments]
            .into_iter()
            .flat_map(|e| e.sprites.iter().map(String::as_str))
    }
}

const MAX_PARTICLES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoulderParticleKind {
    Dust,
    Fragments,
}

impl BoulderParticleKind {
    fn emitter(self, effect: &BoulderFxDef) -> &BoulderEmitterDef {
        match self {
            Self::Dust => &effect.dust,
            Self::Fragments => &effect.fragments,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoulderParticleSample<'a> {
    pub kind: BoulderParticleKind,
    pub sprite: &'a str,
    pub x: f32,
    pub y: f32,
    /// Painter depth relative to the boulder's game layer.
    pub z: f32,
    /// Scale of the extracted eight-pixel tile at 32 pixels per world unit.
    pub scale: f32,
    /// Screen-space rotation in radians.
    pub angle: f32,
    pub color: [f32; 4],
}

#[derive(Clone, Debug)]
struct Particle {
    kind: BoulderParticleKind,
    position: [f32; 3],
    velocity: [f32; 2],
    gravity: [f32; 2],
    damping: f32,
    lifetime: f32,
    age: f32,
    angle: f32,
    spin: f32,
    row: usize,
    gradient: BoulderGradientDef,
}

#[derive(Clone, Debug)]
pub struct BoulderParticles {
    particles: Vec<Particle>,
    rng: Rng,
}

impl Default for BoulderParticles {
    fn default() -> Self {
        Self {
            particles: Vec::new(),
            rng: Rng::new(0x424f_554c),
        }
    }
}

impl BoulderParticles {
    pub fn spawn(
        &mut self,
        def: &BoulderFxDef,
        kind: BoulderParticleKind,
        x: f32,
        y: f32,
        direction: i8,
    ) {
        let emitter = kind.emitter(def);
        if !x.is_finite()
            || !y.is_finite()
            || emitter.sprites.is_empty()
            || emitter.colors.is_empty()
        {
            return;
        }
        let count = usize::from(emitter.count).min(MAX_PARTICLES);
        let overflow = (self.particles.len() + count).saturating_sub(MAX_PARTICLES);
        self.particles.drain(..overflow);
        let yaw_x = if direction > 0 { 1.0 } else { -1.0 };
        for index in 0..count {
            // PoolSpawner gives the burst world position and identity rotation.
            // HitObject's 0/180-degree Y yaw rotates the local launch velocity.
            // Playground adds source scatter in simulation space afterward.
            let mut position = [x, y, 0.0];
            for (axis, coordinate) in position.iter_mut().enumerate() {
                *coordinate += self
                    .rng
                    .range(emitter.scatter_min[axis], emitter.scatter_max[axis]);
            }
            let velocity = [
                yaw_x
                    * self
                        .rng
                        .range(emitter.velocity_min[0], emitter.velocity_max[0]),
                self.rng
                    .range(emitter.velocity_min[1], emitter.velocity_max[1]),
            ];
            let row = if emitter.animate {
                0
            } else {
                self.rng.range_i32(0, emitter.sprites.len() as i32) as usize
            };
            // Playground increments lifetimeColorId modulo the palette on
            // each birth. The authored 12-particle burst cycles both colors.
            let color = (index + 1) % emitter.colors.len();
            self.particles.push(Particle {
                kind,
                position,
                velocity,
                gravity: emitter.gravity,
                damping: emitter.damping,
                lifetime: self.rng.range(emitter.lifetime[0], emitter.lifetime[1]),
                age: 0.0,
                angle: self
                    .rng
                    .range(emitter.rotation[0], emitter.rotation[1])
                    .to_radians(),
                spin: self
                    .rng
                    .range(emitter.spin[0], emitter.spin[1])
                    .to_radians(),
                row,
                gradient: emitter.colors[color],
            });
        }
    }

    pub fn advance(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        self.particles.retain_mut(|particle| {
            particle.age += dt;
            if particle.age >= particle.lifetime {
                return false;
            }
            // Playground applies gravity, lerps velocity toward zero, and
            // moves by the new velocity. These emitters have collision off.
            let retain = (1.0 - particle.damping * dt).clamp(0.0, 1.0);
            for axis in 0..2 {
                particle.velocity[axis] =
                    (particle.velocity[axis] - particle.gravity[axis] * dt) * retain;
                particle.position[axis] += particle.velocity[axis] * dt;
            }
            particle.angle += particle.spin * dt;
            true
        });
    }

    pub fn samples<'a>(
        &self,
        def: &'a BoulderFxDef,
    ) -> impl Iterator<Item = BoulderParticleSample<'a>> + use<'a, '_> {
        self.particles.iter().filter_map(move |particle| {
            let emitter = particle.kind.emitter(def);
            let life = (particle.age / particle.lifetime).clamp(0.0, 1.0);
            let sprite_index = if emitter.animate {
                ((life * emitter.sprites.len() as f32) as usize)
                    .min(emitter.sprites.len().saturating_sub(1))
            } else {
                particle.row
            };
            let mut color = particle.gradient.color;
            let fade = particle.gradient.fade_start;
            if life > fade {
                color[3] *= (1.0 - life) / (1.0 - fade);
            }
            let [x, y, z] = particle.position;
            Some(BoulderParticleSample {
                kind: particle.kind,
                sprite: emitter.sprites.get(sprite_index)?,
                x,
                y,
                z,
                scale: emitter.size / 0.25,
                angle: particle.angle,
                color,
            })
        })
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn fixture() -> BoulderFxDef {
        let dust = BoulderEmitterDef {
            count: 12,
            sprites: ["dust0", "dust1", "dust2", "dust3"]
                .map(String::from)
                .into(),
            animate: true,
            size: 0.25,
            lifetime: [1.0; 2],
            scatter_min: [-0.125, -0.125, -0.1],
            scatter_max: [0.125, 0.125, -0.1],
            velocity_min: [1.0, 2.0],
            velocity_max: [5.0, 4.0],
            gravity: [0.0, 8.0],
            damping: 0.5,
            rotation: [0.0; 2],
            spin: [0.0; 2],
            colors: vec![
                BoulderGradientDef {
                    color: [0.5, 0.4, 0.3, 1.0],
                    fade_start: 0.75,
                },
                BoulderGradientDef {
                    color: [0.6, 0.5, 0.4, 1.0],
                    fade_start: 0.75,
                },
            ],
        };
        let fragments = BoulderEmitterDef {
            count: 4,
            sprites: ["fragment0", "fragment1"].map(String::from).into(),
            animate: false,
            rotation: [100.0; 2],
            spin: [180.0; 2],
            ..dust.clone()
        };
        BoulderFxDef { dust, fragments }
    }

    #[test]
    fn boulder_sidecar_rejects_invalid_world_or_emitter() {
        let mut definition = fixture();
        let mut bytes = definition.encode(17).unwrap();
        assert_eq!(BoulderFxDef::decode(&bytes, 17).unwrap(), definition);
        assert!(BoulderFxDef::decode(&bytes, 19).is_err());
        bytes.push(0);
        assert!(BoulderFxDef::decode(&bytes, 17).is_err());
        definition.dust.lifetime = [0.0; 2];
        assert!(definition.encode(17).is_err());
    }

    #[test]
    fn dust_advances_atlas_and_fades_while_fragments_keep_their_row_and_spin() {
        let world = fixture();
        let mut fx = BoulderParticles::default();
        fx.spawn(&world, BoulderParticleKind::Dust, 2.0, 1.0, 1);
        fx.spawn(&world, BoulderParticleKind::Fragments, 2.0, 1.0, 1);
        let at_birth: Vec<_> = fx.samples(&world).collect();
        assert_eq!(at_birth.len(), 16);
        let dust_colors = &world.dust.colors;
        assert!(at_birth[..12]
            .iter()
            .enumerate()
            .all(|(i, s)| s.color == dust_colors[(i + 1) % 2].color));
        assert!(at_birth[..12]
            .iter()
            .all(|s| s.kind == BoulderParticleKind::Dust && s.sprite == "dust0" && s.angle == 0.0));
        assert!(at_birth[12..]
            .iter()
            .all(|s| s.kind == BoulderParticleKind::Fragments
                && matches!(s.sprite, "fragment0" | "fragment1")));
        fx.advance(0.25);
        let advanced: Vec<_> = fx.samples(&world).collect();
        assert!(advanced[..12]
            .iter()
            .all(|s| s.sprite == "dust1" && s.color[3] == 1.0));
        for (before, after) in at_birth[12..].iter().zip(&advanced[12..]) {
            assert_eq!(before.sprite, after.sprite);
            assert!((after.angle - before.angle - 45.0_f32.to_radians()).abs() < 1e-6);
        }
        fx.advance(0.625);
        assert!(fx.samples(&world).all(|s| s.color[3] == 0.5));
        fx.advance(0.125);
        assert_eq!(fx.samples(&world).count(), 0);
    }

    #[test]
    fn world_scatter_stays_put_and_yaw_mirrors_only_the_local_launch_velocity() {
        let world = fixture();
        let mut right = BoulderParticles::default();
        let mut left = BoulderParticles::default();
        right.spawn(&world, BoulderParticleKind::Fragments, 0.0, 0.0, 1);
        left.spawn(&world, BoulderParticleKind::Fragments, 0.0, 0.0, -1);
        assert_eq!(
            right.samples(&world).collect::<Vec<_>>(),
            left.samples(&world).collect::<Vec<_>>()
        );
        let before = right.particles[0].clone();
        let dt = game::state::TICK;
        right.advance(dt);
        left.advance(dt);
        let r = right.samples(&world).next().unwrap();
        let l = left.samples(&world).next().unwrap();
        assert!(r.x > before.position[0] && l.x < before.position[0]);
        assert!((r.x + l.x - 2.0 * before.position[0]).abs() < 1e-6);
        assert_eq!(r.y, l.y);
        assert_eq!((r.z, l.z), (-0.1, -0.1));
        assert!(
            (r.y - before.position[1] - (before.velocity[1] - 8.0 * dt) * (1.0 - 0.5 * dt) * dt)
                .abs()
                < 1e-6
        );
    }
}
