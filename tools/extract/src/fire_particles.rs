//! Read the Fire prefab's managed Playground emitters and cut their textures.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use game::data::Curve;
use scene::fire_particles::{EmitterDef, FireParticlesDef, FireVariant};
use serde_json::{Map, Value};

use crate::{
    sprites::{Image, Sheet, Sprite},
    yaml::{field, guid_of},
};

fn component<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node["components"]
        .as_array()?
        .iter()
        .find(|component| component["type"] == name)
        .map(|component| &component["fields"])
}

fn active_sprite(node: &Value) -> bool {
    node["active"].as_bool() == Some(true)
        && (component(node, "SpriteRenderer").is_some_and(|renderer| {
            renderer["m_Enabled"].as_i64() == Some(1) && renderer["m_Sprite"]["asset"].is_string()
        }) || node["children"]
            .as_array()
            .is_some_and(|children| children.iter().any(active_sprite)))
}

fn number(v: &Value, key: &str) -> Result<f32, String> {
    v[key]
        .as_f64()
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("invalid fire particle {key}"))
}

fn vector(v: &Value) -> Result<[f32; 3], String> {
    Ok([number(v, "x")?, number(v, "y")?, number(v, "z")?])
}

fn curve(v: &Value) -> Result<Curve, String> {
    let keys = v["m_Curve"]
        .as_array()
        .ok_or("missing fire particle curve")?
        .iter()
        .map(|key| {
            let slope = |name| match key[name].as_str() {
                Some("Infinity") => Ok(f32::INFINITY),
                Some("-Infinity") => Ok(f32::NEG_INFINITY),
                _ => number(key, name),
            };
            Ok([
                number(key, "time")?,
                number(key, "value")?,
                slope("inSlope")?,
                slope("outSlope")?,
            ])
        })
        .collect::<Result<_, String>>()?;
    Ok(Curve { keys })
}

type Tiles = HashMap<(PathBuf, u32, u32), Vec<Vec<String>>>;

fn tiles(
    path: &Path,
    columns: u32,
    rows: u32,
    sheet: &mut Sheet,
    cache: &mut Tiles,
) -> Result<Vec<Vec<String>>, String> {
    let key = (path.to_path_buf(), columns, rows);
    if let Some(names) = cache.get(&key) {
        return Ok(names.clone());
    }
    let texture = Image::load_png(path)?;
    let w = texture.w / columns;
    let h = texture.h / rows;
    if w != h || !(1..=32).contains(&w) || (w * columns, h * rows) != (texture.w, texture.h) {
        return Err(format!(
            "unsupported fire particle atlas {}",
            path.display()
        ));
    }
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("unnamed fire atlas")?;
    let mut names = Vec::new();
    for row in 0..rows {
        let mut names_in_row = Vec::new();
        // Unity counts tile rows from the bottom; the export PNG starts at the top.
        let y = (rows - 1 - row) * h;
        for col in 0..columns {
            let name = format!("__fire_{stem}_{}", row * columns + col);
            let mut pixels = Vec::with_capacity((w * h * 4) as usize);
            for y in y..y + h {
                let start = ((y * texture.w + col * w) * 4) as usize;
                pixels.extend_from_slice(&texture.px[start..start + (w * 4) as usize]);
            }
            sheet.sprites.push(Sprite {
                name: name.clone(),
                img: Image { w, h, px: pixels },
                pivot: (w as f32 * 0.5, h as f32 * 0.5),
                ppu: 32.0,
            });
            names_in_row.push(name);
        }
        names.push(names_in_row);
    }
    cache.insert(key, names.clone());
    Ok(names)
}

fn gradient_keys(
    gradient: &Value,
    count: &str,
    time: &str,
    channel: &str,
) -> Result<Vec<(f32, f32)>, String> {
    (0..number(gradient, count)? as usize)
        .map(|i| {
            let t = number(gradient, &format!("{time}{i}"))? / 65535.0;
            let v = number(&gradient[format!("key{i}")], channel)?;
            Ok((t, v))
        })
        .collect()
}

fn emitter(
    node: &Value,
    p: &Value,
    offset: [f32; 3],
    assets: &Path,
    guids: &HashMap<String, PathBuf>,
    sheet: &mut Sheet,
    cache: &mut Tiles,
) -> Result<EmitterDef, String> {
    for (key, expected) in [
        ("source", 1),
        ("loop", 1),
        ("emissionRate", 1),
        ("clearParticlesOnEmissionStop", 0),
        ("applyLifetimeVelocity", 1),
        ("applyLifetimeSize", 0),
        ("prewarm", 0),
    ] {
        if p[key].as_i64() != Some(expected) {
            return Err(format!("unsupported looping Playground {key}"));
        }
    }
    let uv = &component(node, "ParticleSystem").ok_or("missing fire ParticleSystem")?["UVModule"];
    let renderer = component(node, "ParticleSystemRenderer").ok_or("missing fire billboard")?;
    if uv["enabled"].as_i64() != Some(1)
        || uv["animationType"].as_i64() != Some(1)
        || uv["cycles"].as_i64() != Some(1)
        || uv["rowIndex"].as_i64() != Some(0)
        || renderer["m_RenderMode"].as_i64() != Some(0)
        || renderer["m_SortingOrder"].as_i64() != Some(0)
        || renderer["m_SortingLayer"].as_i64() != Some(0)
    {
        return Err("unsupported fire texture sheet or render order".into());
    }
    let material = renderer["m_Materials"][0]["asset"]
        .as_str()
        .ok_or("missing fire material")?;
    let material = std::fs::read_to_string(assets.join(material)).map_err(|e| e.to_string())?;
    let texture = field(&material, "m_Texture")
        .and_then(guid_of)
        .and_then(|id| guids.get(id))
        .ok_or("missing fire texture")?;
    let dimensions = |name| {
        uv[name]
            .as_u64()
            .filter(|n| (1..=16).contains(n))
            .map(|n| n as u32)
            .ok_or_else(|| format!("invalid fire particle {name}"))
    };
    let rows = tiles(
        texture,
        dimensions("tilesX")?,
        dimensions("tilesY")?,
        sheet,
        cache,
    )?;
    let range = |lo, hi| Ok::<_, String>([vector(&p[lo])?, vector(&p[hi])?]);
    let scatter = if p["applySourceScatter"].as_i64() == Some(1) {
        let scale = vector(&p["scatterScale"])?;
        range("sourceScatterMin", "sourceScatterMax")?
            .map(|v| std::array::from_fn(|axis| v[axis] * scale[axis]))
    } else {
        [[0.0; 3]; 2]
    };
    let velocity = if p["applyInitialLocalVelocity"].as_i64() == Some(1) {
        range("initialLocalVelocityMin", "initialLocalVelocityMax")?
    } else if p["applyInitialVelocity"].as_i64() == Some(1) {
        range("initialVelocityMin", "initialVelocityMax")?
    } else {
        [[0.0; 3]; 2]
    };
    let lifetime = number(p, "lifetime")?;
    let min_lifetime = match p["lifetimeValueMethod"].as_i64() {
        Some(0) => lifetime,
        Some(1) => number(p, "lifetimeMin")?,
        _ => return Err("unsupported fire lifetime mode".into()),
    };
    let (rgb, alpha) = match p["colorSource"].as_i64() {
        Some(0) => (Default::default(), Vec::new()),
        Some(1) => {
            let gradient = &p["lifetimeColor"];
            let rgb = |ch| gradient_keys(gradient, "m_NumColorKeys", "ctime", ch);
            (
                [rgb("r")?, rgb("g")?, rgb("b")?],
                gradient_keys(gradient, "m_NumAlphaKeys", "atime", "a")?,
            )
        }
        _ => return Err("unsupported fire color source".into()),
    };
    let motion = |axis| curve(&p["lifetimeVelocity"][axis]);
    let count = p["particleCount"]
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .ok_or("invalid fire particle count")?;
    Ok(EmitterDef {
        offset,
        count,
        lifetime: [min_lifetime, lifetime],
        size: number(p, "sizeMin")? * number(p, "scale")?,
        scatter,
        velocity,
        gravity: vector(&p["gravity"])?,
        damping: number(p, "damping")?,
        motion: [motion("x")?, motion("y")?, motion("z")?],
        frame: curve(&uv["frameOverTime"]["maxCurve"])?,
        ambient: field(&material, "m_ShaderKeywords").is_some_and(|k| k.contains("USEAMBIENT_ON")),
        rgb,
        alpha,
        random_row: uv["randomRow"].as_i64() == Some(1),
        rows,
    })
}

pub fn extract(
    assets: &Path,
    guids: &HashMap<String, PathBuf>,
    prefabs: &Map<String, Value>,
    sheet: &mut Sheet,
) -> Result<FireParticlesDef, String> {
    let fire = prefabs
        .values()
        .find_map(|node| component(node, "Fire"))
        .ok_or("missing Fire prefab")?;
    let mut cache = Tiles::new();
    let mut variants = Vec::new();
    for reference in fire["flamePrefabs"]
        .as_array()
        .ok_or("missing Fire flame prefabs")?
    {
        let path = reference["asset"].as_str().ok_or("unlinked flame prefab")?;
        let node = prefabs.get(path).ok_or_else(|| format!("missing {path}"))?;
        let flame = component(node, "Flame").ok_or_else(|| format!("missing Flame in {path}"))?;
        let size = [number(&flame["size"], "x")?, number(&flame["size"], "y")?];
        let mut emitters = Vec::new();
        for (emitter_node, offset) in std::iter::once((node, [0.0; 3])).chain(
            node["children"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|child| {
                    let offset = child["pos"]
                        .as_array()
                        .and_then(|p| {
                            Some([
                                p.first()?.as_f64()? as f32,
                                p.get(1)?.as_f64()? as f32,
                                p.get(2)?.as_f64()? as f32,
                            ])
                        })
                        .unwrap_or([0.0; 3]);
                    (child, offset)
                }),
        ) {
            if let Some(p) = component(emitter_node, "PlaygroundParticlesC") {
                emitters.push(
                    emitter(emitter_node, p, offset, assets, guids, sheet, &mut cache)
                        .map_err(|e| format!("{path}/{}: {e}", emitter_node["name"]))?,
                );
            }
        }
        variants.push(FireVariant {
            size,
            animated_sprite: active_sprite(node),
            emitters,
        });
    }
    Ok(FireParticlesDef { variants })
}
