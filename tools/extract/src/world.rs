//! Exported scene/prefab/data JSON → `game::data::WorldData` (`world.bin`).
//!
//! This is where Unity components are mapped onto the game's own data
//! model. It grows as more systems are ported.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use game::animation::Pose;
use game::data::{
    BlockDef, BuildingDef, CampDef, DayDef, DirectorDef, FixedTransform, ForestItemDef, Kind,
    Layer, LayerDef, LevelConfigDef, LightPreset, Part, PayableDef, Placed, Proto, ProtoId,
    RandomSprite, Role, ScaffoldModuleDef, Scatter, ShopDef, Side, SpriteId, ToolKind, TreeWorkDef,
    UpgradeDef, WorldData, LIGHT_COLORS, LIGHT_FLOATS, TILE_SCATTER_FLAGS,
};
use game::data::{Curve, PortalDef, WallDef, WaveDef};
use scene::authored::{PartVisual, PrefabVisual, TransformNode};
use serde_json::{Map, Value};

/// Components deriving from `Payable` (for overlap regions and half widths).
const PAYABLES: &[&str] = &[
    "Beach",
    "Cabin",
    "ChristmasTree",
    "Hermit",
    "Merchant",
    "PayableBorder",
    "PayableShield",
    "PayableShop",
    "PayableTeleporter",
    "PayableTree",
    "PayableUpgrade",
    "PayableWorkshop",
    "PayableXbox",
    "Signpost",
    "Snowman",
    "Statue",
    "Steed",
    "Wharf",
];

/// Sprite size info from the cutter: (width px, height px, ppu).
pub type SpriteSizes = HashMap<String, (u32, u32, f32)>;

pub struct Inputs<'a> {
    pub main: &'a Value,
    pub blocks: &'a Value,
    /// The menu scene (the island map).
    pub loader: &'a Value,
    pub prefabs: &'a Map<String, Value>,
    pub data: &'a Map<String, Value>,
    pub sprite_names: &'a HashMap<String, String>,
    pub sprite_sizes: &'a SpriteSizes,
    pub assets_dir: &'a Path,
}

fn comp<'v>(node: &'v Value, ty: &str) -> Option<&'v Map<String, Value>> {
    node["components"]
        .as_array()?
        .iter()
        .find(|c| c["type"] == ty)
        .and_then(|c| c["fields"].as_object())
}

/// The `id` of the first component of type `ty` (what `{"local": id}` refers to).
fn comp_id(node: &Value, ty: &str) -> Option<i64> {
    node["components"]
        .as_array()?
        .iter()
        .find(|c| c["type"] == ty)
        .and_then(|c| c["id"].as_i64())
}

fn has_any(node: &Value, types: &[&'static str]) -> Option<&'static str> {
    let comps = node["components"].as_array()?;
    for c in comps {
        if let Some(t) = c["type"].as_str() {
            if let Some(found) = types.iter().find(|x| **x == t) {
                return Some(found);
            }
        }
    }
    None
}

pub fn camera_def(main: &Value) -> Result<Option<scene::camera::CameraDef>, String> {
    let Some((_, fields)) = find_component(main, "MainCamera") else {
        return Ok(None);
    };
    use game::animation::{Key, Tangent};
    let keys = |name: &str| -> Result<Vec<Key>, String> {
        fields
            .get(name)
            .and_then(|field| field.get("m_Curve"))
            .unwrap_or(&Value::Null)
            .as_array()
            .ok_or_else(|| format!("camera {name}: no curve"))?
            .iter()
            .map(|k| {
                let f = |field: &str| {
                    k[field]
                        .as_f64()
                        .map(|v| v as f32)
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| format!("camera {name}.{field}: invalid key"))
                };
                Ok(Key {
                    time: f("time")?,
                    value: f("value")?,
                    in_tangent: Tangent::Slope(f("inSlope")?),
                    out_tangent: Tangent::Slope(f("outSlope")?),
                })
            })
            .collect()
    };
    Ok(Some(scene::camera::CameraDef {
        lead: flag(fields, "lead"),
        max_lead_speed: num(fields, "maxLeadSpeed"),
        lead_over_time: keys("leadOverTime")?,
        seek_speed_by_distance: keys("seekSpeedByDistance")?,
    }))
}

fn num(f: &Map<String, Value>, k: &str) -> f32 {
    f.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32
}

fn flag(f: &Map<String, Value>, k: &str) -> bool {
    f.get(k).and_then(Value::as_i64).unwrap_or(0) != 0
}

fn vec3(v: &Value) -> [f32; 3] {
    [0, 1, 2].map(|i| v[i].as_f64().unwrap_or(0.0) as f32)
}

fn asset_path(v: Option<&Value>) -> Option<&str> {
    v?.get("asset")?.as_str()
}

// Scaffold modules have their own draw path. Layer 29 is seen by the
// water reflection camera, whose extra pass is outside these visual parts.
fn skip_part_subtree(node: &Value) -> bool {
    comp(node, "ScaffoldModule").is_some() || node["layer"].as_i64() == Some(29)
}

fn active_part_children(node: &Value) -> impl Iterator<Item = &Value> {
    node["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|child| child["active"] == true)
}

fn local_pose(node: &Value, renderer: Option<&Map<String, Value>>) -> Pose {
    let mut pose = Pose {
        position: vec3(&node["pos"]),
        scale: [0, 1, 2].map(|i| node["scale"][i].as_f64().unwrap_or(1.0) as f32),
        rotation: [0, 1, 2, 3].map(|i| {
            node["rot"][i]
                .as_f64()
                .unwrap_or(if i == 3 { 1.0 } else { 0.0 }) as f32
        }),
        ..Default::default()
    };
    if let Some(renderer) = renderer {
        pose.color = ["r", "g", "b", "a"].map(|channel| {
            renderer
                .get("m_Color")
                .and_then(|color| color[channel].as_f64())
                .unwrap_or(1.0) as f32
        });
        pose.visible = flag(renderer, "m_Enabled");
    }
    pose
}

/// Collect visual metadata independently of the legacy sprite interning and
/// world fields. The part list follows the same active renderer traversal.
fn authored_prefab(node: &Value, legacy_parts: usize) -> Result<PrefabVisual, String> {
    fn collect(
        node: &Value,
        path: String,
        parent: Option<u16>,
        visual: &mut PrefabVisual,
    ) -> Result<(), String> {
        if skip_part_subtree(node) {
            return Ok(());
        }
        let index = u16::try_from(visual.nodes.len())
            .map_err(|_| format!("too many authored transform nodes at {path}"))?;
        let renderer = comp(node, "SpriteRenderer");
        let mut local = local_pose(node, renderer);
        if parent.is_none() {
            // Object placement and its facing own the root translation and
            // X reflection. Keep the authored rotation and scale magnitudes.
            local.position = [0.0; 3];
            local.scale[0] = local.scale[0].abs();
        }
        let animator = comp(node, "Animator")
            .filter(|animator| flag(animator, "m_Enabled"))
            .and_then(|animator| asset_path(animator.get("m_Controller")))
            .map(str::to_owned);
        if let Some(renderer) = renderer.filter(|renderer| flag(renderer, "m_Enabled")) {
            visual.parts.push(PartVisual {
                node: index,
                flip_x: flag(renderer, "m_FlipX"),
                flip_y: flag(renderer, "m_FlipY"),
            });
        }
        visual.nodes.push(TransformNode {
            path,
            parent,
            local,
            animator,
        });
        for child in active_part_children(node) {
            let path = &visual.nodes[index as usize].path;
            let name = child["name"].as_str().unwrap_or_default();
            let child_path = if path.is_empty() {
                name.to_owned()
            } else {
                format!("{path}/{name}")
            };
            collect(child, child_path, Some(index), visual)?;
        }
        Ok(())
    }

    let mut visual = PrefabVisual::default();
    collect(node, String::new(), None, &mut visual)?;
    if visual.parts.len() != legacy_parts {
        let name = node["name"].as_str().unwrap_or_default();
        return Err(format!("authored renderer count differs for {name}"));
    }
    Ok(visual)
}

/// Materials with the `Recolor` toggle on.
fn recolor_materials(assets: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(rd) = std::fs::read_dir(assets.join("Material")) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "mat") {
            if let Ok(t) = std::fs::read_to_string(&p) {
                if t.lines().any(|l| l.trim() == "- Recolor: 1") {
                    out.insert(format!(
                        "Material/{}",
                        p.file_name().unwrap().to_string_lossy()
                    ));
                }
            }
        }
    }
    out
}

struct Builder<'a> {
    inp: &'a Inputs<'a>,
    world: WorldData,
    visuals: Vec<Result<PrefabVisual, String>>,
    sprite_ids: HashMap<String, SpriteId>,
    prefab_protos: HashMap<String, ProtoId>,
    recolor: HashSet<String>,
}

impl<'a> Builder<'a> {
    fn sprite(&mut self, r: Option<&Value>) -> Option<SpriteId> {
        let name = self.inp.sprite_names.get(asset_path(r)?)?.clone();
        if let Some(&id) = self.sprite_ids.get(&name) {
            return Some(id);
        }
        let id = self.world.sprites.len() as SpriteId;
        self.world.sprites.push(name.clone());
        self.sprite_ids.insert(name, id);
        Some(id)
    }

    fn sprite_width(&self, id: Option<SpriteId>) -> f32 {
        id.and_then(|i| self.inp.sprite_sizes.get(&self.world.sprites[i as usize]))
            .map_or(0.0, |&(w, _, ppu)| w as f32 / ppu)
    }

    fn prefab(&mut self, path: &str) -> Option<ProtoId> {
        if let Some(&id) = self.prefab_protos.get(path) {
            return Some(id);
        }
        let node = self.inp.prefabs.get(path)?;
        // Reserve the id first: upgrade chains reference prefabs recursively.
        let id = self.reserve();
        self.prefab_protos.insert(path.to_owned(), id);
        self.fill(id, node);
        Some(id)
    }

    fn reserve(&mut self) -> ProtoId {
        let id = self.world.protos.len() as ProtoId;
        self.world.protos.push(Proto::default());
        self.visuals.push(Ok(PrefabVisual::default()));
        id
    }

    fn proto(&mut self, node: &Value) -> ProtoId {
        let id = self.reserve();
        self.fill(id, node);
        id
    }

    /// Scaffold modules of a WorkableBuilding, relative to the building root.
    fn scaffold(&mut self, node: &Value) -> Vec<ScaffoldModuleDef> {
        fn find<'v>(n: &'v Value, path: &mut Vec<&'v Value>) -> bool {
            path.push(n);
            if comp(n, "Scaffolding").is_some() {
                return true;
            }
            for c in n["children"].as_array().into_iter().flatten() {
                if find(c, path) {
                    return true;
                }
            }
            path.pop();
            false
        }
        let mut path = Vec::new();
        if !find(node, &mut path) {
            return Vec::new();
        }
        // Scaffolding transform relative to the building root.
        let (mut at, mut sc) = ([0.0f32; 3], (1.0f32, 1.0f32));
        for n in &path[1..] {
            let p = vec3(&n["pos"]);
            let s = vec3(&n["scale"]);
            at = [at[0] + sc.0 * p[0], at[1] + sc.1 * p[1], at[2] + p[2]];
            sc = (sc.0 * s[0], sc.1 * s[1]);
        }
        let scaffolding = path[path.len() - 1];
        let mut modules: Vec<(f32, ScaffoldModuleDef)> = Vec::new();
        fn collect<'v>(
            n: &'v Value,
            at: [f32; 3],
            sc: (f32, f32),
            out: &mut Vec<(&'v Value, [f32; 3])>,
        ) {
            for c in n["children"].as_array().into_iter().flatten() {
                let p = vec3(&c["pos"]);
                let s = vec3(&c["scale"]);
                let cat = [at[0] + sc.0 * p[0], at[1] + sc.1 * p[1], at[2] + p[2]];
                out.push((c, cat));
                collect(c, cat, (sc.0 * s[0], sc.1 * s[1]), out);
            }
        }
        let mut nodes = Vec::new();
        collect(scaffolding, at, sc, &mut nodes);
        for (n, pos) in nodes {
            let Some(m) = comp(n, "ScaffoldModule") else {
                continue;
            };
            let sprite = comp(n, "SpriteRenderer").and_then(|sr| self.sprite(sr.get("m_Sprite")));
            let fill = self.sprite(m.get("fillSprite"));
            let local = vec3(&n["pos"]);
            let order = local[0] * local[0] + local[1] * local[1] + local[2] * local[2];
            modules.push((
                order,
                ScaffoldModuleDef {
                    sprite,
                    fill,
                    x: pos[0],
                    y: pos[1],
                    z: pos[2],
                },
            ));
        }
        modules.sort_by(|a, b| a.0.total_cmp(&b.0));
        modules.into_iter().map(|m| m.1).collect()
    }

    /// Flattens the SpriteRenderers of `node`'s active subtree into parts.
    fn parts(
        &mut self,
        node: &Value,
        at: [f32; 3],
        scale: (f32, f32),
        parent: Option<u16>,
        out: &mut Vec<Part>,
        ids: &mut Vec<i64>,
    ) {
        self.parts_in(node, at, scale, parent, out, ids, false);
    }

    #[allow(clippy::too_many_arguments)]
    fn parts_in(
        &mut self,
        node: &Value,
        at: [f32; 3],
        scale: (f32, f32),
        parent: Option<u16>,
        out: &mut Vec<Part>,
        ids: &mut Vec<i64>,
        in_scaffold: bool,
    ) {
        let in_scaffold = in_scaffold || comp(node, "Scaffolding").is_some();
        if skip_part_subtree(node) {
            return;
        }
        let mut me = parent;
        if let Some(sr) = comp(node, "SpriteRenderer") {
            if flag(sr, "m_Enabled") {
                let sprite = self.sprite(sr.get("m_Sprite"));
                let random = comp(node, "RandomizeSprite")
                    .filter(|r| flag(r, "m_Enabled"))
                    .map(|r| {
                        let sprites = r
                            .get("sprites")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|s| self.sprite(Some(s)))
                            .collect();
                        RandomSprite {
                            sprites,
                            allow_flip: flag(r, "allowFlip"),
                            match_parent: flag(r, "matchParent"),
                            deactivation_probability: num(r, "deactivationProbability"),
                            shift_x: num(r, "shiftX"),
                        }
                    });
                let c = sr.get("m_Color");
                let ch =
                    |k: &str| (c.and_then(|c| c[k].as_f64()).unwrap_or(1.0) * 255.0).round() as u8;
                let material = sr
                    .get("m_Materials")
                    .and_then(Value::as_array)
                    .and_then(|m| asset_path(m.first()));
                let recolor = material.is_some_and(|p| self.recolor.contains(p));
                // Project materials are lit unless unlit/additive; built-in sprite materials are unlit.
                let lit = material.is_some_and(|p| {
                    p.starts_with("Material/") && !p.contains("Unlit") && !p.contains("Additive")
                });
                me = Some(out.len() as u16);
                ids.push(comp_id(node, "SpriteRenderer").unwrap_or(0));
                out.push(Part {
                    sprite,
                    x: at[0],
                    y: at[1],
                    z: at[2],
                    flip: (scale.0 < 0.0) ^ flag(sr, "m_FlipX"),
                    color: [ch("r"), ch("g"), ch("b"), ch("a")],
                    sort_layer: sr
                        .get("m_SortingLayer")
                        .and_then(Value::as_i64)
                        .unwrap_or(0) as i16,
                    sort_order: sr
                        .get("m_SortingOrder")
                        .and_then(Value::as_i64)
                        .unwrap_or(0) as i16,
                    random,
                    parent,
                    recolor,
                    lit,
                    scaffold: in_scaffold,
                });
            }
        }
        for child in active_part_children(node) {
            let p = vec3(&child["pos"]);
            let s = vec3(&child["scale"]);
            let cat = [at[0] + scale.0 * p[0], at[1] + scale.1 * p[1], at[2] + p[2]];
            self.parts_in(
                child,
                cat,
                (scale.0 * s[0], scale.1 * s[1]),
                me,
                out,
                ids,
                in_scaffold,
            );
        }
    }

    fn fill(&mut self, id: ProtoId, node: &Value) {
        let root_scale = vec3(&node["scale"]);
        let mut parts = Vec::new();
        let mut part_ids = Vec::new();
        self.parts(
            node,
            [0.0; 3],
            (root_scale[0].abs(), root_scale[1]),
            None,
            &mut parts,
            &mut part_ids,
        );
        self.visuals[id as usize] = authored_prefab(node, parts.len());

        let root_sprite = comp(node, "SpriteRenderer").and_then(|sr| {
            let r = sr.get("m_Sprite");
            asset_path(r)
                .and_then(|p| self.inp.sprite_names.get(p))
                .cloned()
        });
        let root_width = root_sprite
            .and_then(|n| self.inp.sprite_sizes.get(&n))
            .map(|&(w, _, ppu)| w as f32 / ppu * root_scale[0].abs());

        let payable = has_any(node, PAYABLES).and_then(|t| comp(node, t));
        let pay_region = payable.map(|p| {
            let off = p
                .get("playerPayPointOffset")
                .and_then(|o| o["x"].as_f64())
                .unwrap_or(0.0) as f32;
            (off, num(p, "playerPayDistance"))
        });
        let half_width = match (root_width, pay_region, comp(node, "GAPS")) {
            (Some(w), _, _) => w / 2.0,
            (None, Some((_, d)), _) => d,
            (None, None, Some(g)) => {
                g.get("region")
                    .and_then(|r| r["width"].as_f64())
                    .unwrap_or(0.0) as f32
                    / 2.0
            }
            _ => 0.0,
        };

        let kind = if let Some(t) = comp(node, "Tile") {
            let width =
                self.sprite_width(parts.first().and_then(|p| p.sprite)) * root_scale[0].abs();
            let mut scatter = 0;
            for (bit, name) in TILE_SCATTER_FLAGS.iter().enumerate() {
                if !name.is_empty() && flag(t, name) {
                    scatter |= 1 << bit;
                }
            }
            Kind::Tile {
                width,
                no_ground: flag(t, "noGround"),
                scatter,
            }
        } else if let Some(p) = comp(node, "PrefabPlaceholder") {
            let target = asset_path(p.get("prefab"))
                .map(str::to_owned)
                .and_then(|path| self.prefab(&path));
            match target {
                Some(proto) if flag(p, "doNotSwap") => Kind::Dormant { proto },
                Some(proto) => Kind::Placeholder {
                    proto,
                    spawn_in_game_layer: flag(p, "spawnInGameLayer"),
                },
                None => Kind::Scenery,
            }
        } else if comp(node, "HintText").is_some() {
            Kind::Hint
        } else {
            Kind::Scenery
        };

        let fixed = comp(node, "FixedTransform").map(|f| FixedTransform {
            fix_z: flag(f, "fixZ").then(|| num(f, "z")),
            fix_y: flag(f, "fixY").then(|| num(f, "y")),
            random_offset_range: num(f, "randomOffsetRange"),
            symmetric: flag(f, "symmetric"),
            flipped: flag(f, "flipped"),
        });
        let pos = vec3(&node["pos"]);
        let scatter = comp(node, "ScatteredObject").map(|s| Scatter {
            layer: Layer::from_content_index(s.get("layer").and_then(Value::as_i64).unwrap_or(2))
                .unwrap_or(Layer::Game),
            adjacent: flag(s, "adjacent"),
            spacing: num(s, "spacing"),
            irregularity: num(s, "irregularity"),
            dist_from_region_edge: num(s, "distFromRegionEdge"),
            avoid: s
                .get("avoidOverlapWith")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect(),
            y: pos[1],
            z: pos[2],
        });

        let tree = comp(node, "RandomizeTree")
            .filter(|t| flag(t, "m_Enabled"))
            .map(|t| {
                let c = &t["backColor"];
                let ch = |k: &str| (c[k].as_f64().unwrap_or(1.0) * 255.0).round() as u8;
                game::data::TreeDepth {
                    z_front: num(t, "zFront"),
                    z_back: num(t, "zBack"),
                    back_color: [ch("r"), ch("g"), ch("b"), ch("a")],
                    random_offset_range: num(t, "randomOffsetRange"),
                }
            });
        let payable_def = payable.map(|p| {
            let pair = |k: &str| {
                (
                    p[k]["x"].as_f64().unwrap_or(0.0) as f32,
                    p[k]["y"].as_f64().unwrap_or(0.0) as f32,
                )
            };
            PayableDef {
                price: p.get("price").and_then(Value::as_i64).unwrap_or(1) as u8,
                price_increase: p.get("priceIncrease").and_then(Value::as_i64).unwrap_or(0) as u8,
                coin_offset: pair("floatingCoinPosition"),
                coin_spacing: num(p, "floatingCoinSpacing"),
                pay_point_offset: pair("playerPayPointOffset").0,
                pay_distance: num(p, "playerPayDistance"),
            }
        });
        let upgrade = match comp(node, "PayableUpgrade") {
            Some(u) => {
                let next = asset_path(u.get("nextPrefab"))
                    .map(str::to_owned)
                    .and_then(|path| self.prefab(&path));
                Some(UpgradeDef {
                    next,
                    only_in_buildable_region: flag(u, "onlyInBuildableRegion"),
                    requires_stone: flag(u, "requiresStoneTech"),
                    cooldown: num(u, "cooldown"),
                })
            }
            None => None,
        };
        let building = comp(node, "WorkableBuilding").map(|w| BuildingDef {
            build_points: w.get("buildPoints").and_then(Value::as_i64).unwrap_or(0) as i32,
            auto_build_rate: num(w, "autoBuildRate"),
            scaffold: self.scaffold(node),
        });
        let forest_item = comp(node, "ForestItem")
            .filter(|f| flag(f, "m_Enabled"))
            .map(|f| ForestItemDef {
                controls_forest_size: flag(f, "controlsForestSize"),
                size_padding: num(f, "sizePadding"),
                width: root_width.unwrap_or(0.0),
            });
        let shop = comp(node, "PayableShop").and_then(|sh| {
            let item = self.inp.prefabs.get(asset_path(sh.get("itemPrefab"))?)?;
            let tool = item["tag"].as_str().and_then(ToolKind::from_tag)?;
            let place = &sh["itemPlacement"];
            Some(ShopDef {
                tool,
                item_placement: (
                    place["x"].as_f64().unwrap_or(0.0) as f32,
                    place["y"].as_f64().unwrap_or(0.0) as f32,
                ),
                item_spacing: num(sh, "itemSpacing"),
                max_items: sh.get("maxItems").and_then(Value::as_i64).unwrap_or(1) as u8,
            })
        });
        let tree_work = comp(node, "WorkableTree").map(|t| {
            let mark = t
                .get("chopMark")
                .and_then(|r| r.get("local"))
                .and_then(Value::as_i64);
            TreeWorkDef {
                strength: t.get("strength").and_then(Value::as_i64).unwrap_or(3) as i32,
                coin_yield: (
                    t["coinYield"]["min"].as_i64().unwrap_or(0) as i32,
                    t["coinYield"]["max"].as_i64().unwrap_or(0) as i32,
                ),
                chop_mark: mark
                    .and_then(|m| part_ids.iter().position(|&i| i == m))
                    .map(|i| i as u16),
            }
        });
        let camp = comp(node, "BeggarCamp").map(|c| CampDef {
            max_beggars: c.get("maxBeggars").and_then(Value::as_i64).unwrap_or(2) as u8,
            spawn_interval: num(c, "spawnInterval"),
            hunger_interval: num(c, "hungerInterval"),
        });
        let character = comp(node, "Character")
            .and_then(|_| node["tag"].as_str())
            .and_then(Role::from_tag);
        let wall = comp(node, "Wall").is_some();
        let damageable = comp(node, "Damageable");
        let hp = damageable
            .and_then(|d| d.get("initialHitPoints"))
            .and_then(Value::as_i64)
            .unwrap_or(1) as i32;
        let wall_def = match comp(node, "Wall") {
            Some(w) => {
                let wreck = asset_path(w.get("wreckPrefab"))
                    .map(str::to_owned)
                    .and_then(|path| self.prefab(&path));
                Some(WallDef {
                    hit_points: hp,
                    repair_rate: w.get("repairRate").and_then(Value::as_i64).unwrap_or(1) as i32,
                    repair_rate_above_max: w
                        .get("repairRateAboveMax")
                        .and_then(Value::as_i64)
                        .unwrap_or(1) as i32,
                    bonus_hit_points_multiplier: num(w, "bonusHitPointsMultiplier"),
                    minimum_intact: num(w, "minimumIntactHitPoints"),
                    wreck,
                })
            }
            None => None,
        };
        let damaged_by = damageable
            .and_then(|d| d.get("damagedBy"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let portal = match comp(node, "Portal") {
            Some(p) => {
                // The root trigger polygon's bounds (+ collider offset).
                let hit_box = node["components"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|c| c["type"] == "PolygonCollider2D")
                    .map_or((-1.0, 1.0, 0.0, 3.0), |c| {
                        let f = &c["fields"];
                        let off = (
                            f["m_Offset"]["x"].as_f64().unwrap_or(0.0) as f32,
                            f["m_Offset"]["y"].as_f64().unwrap_or(0.0) as f32,
                        );
                        let mut b = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
                        for path in f["m_Points"]["m_Paths"].as_array().into_iter().flatten() {
                            for pt in path.as_array().into_iter().flatten() {
                                let x = pt["x"].as_f64().unwrap_or(0.0) as f32 + off.0;
                                let y = pt["y"].as_f64().unwrap_or(0.0) as f32 + off.1;
                                b = (b.0.min(x), b.1.max(x), b.2.min(y), b.3.max(y));
                            }
                        }
                        b
                    });
                let rift = asset_path(p.get("teleporterRiftPrefab"))
                    .map(str::to_owned)
                    .and_then(|path| self.prefab(&path));
                Some(PortalDef {
                    cliff: p.get("type").and_then(Value::as_i64) == Some(1),
                    hit_points: hp,
                    by_arrows: damaged_by & 2 != 0,
                    by_knights: damaged_by & 16 != 0,
                    hit_box,
                    defense_range: num(p, "defenseTriggerRange"),
                    defense_delay: (
                        p["defenseSpawnDelay"]["min"].as_f64().unwrap_or(2.0) as f32,
                        p["defenseSpawnDelay"]["max"].as_f64().unwrap_or(16.0) as f32,
                    ),
                    defense_cooldown: num(p, "defenseSpawnCooldown"),
                    rebuild_days: num(p, "rebuildDays"),
                    rift,
                })
            }
            None => None,
        };
        let castle_level =
            comp(node, "Castle").map(|c| c.get("level").and_then(Value::as_i64).unwrap_or(0) as u8);
        let statue = comp(node, "Statue").map(|st| {
            // The charge indicators are SpriteFX components; their objects' renderers are the parts.
            let charge_parts = st
                .get("charges")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| r.get("local").and_then(Value::as_i64))
                .filter_map(|fx| {
                    fn find(n: &Value, id: i64) -> Option<&Value> {
                        if n["components"]
                            .as_array()
                            .is_some_and(|cs| cs.iter().any(|c| c["id"].as_i64() == Some(id)))
                        {
                            return Some(n);
                        }
                        n["children"].as_array()?.iter().find_map(|c| find(c, id))
                    }
                    find(node, fx)
                })
                .filter_map(|n| comp_id(n, "SpriteRenderer"))
                .filter_map(|sr| part_ids.iter().position(|&i| i == sr))
                .map(|i| i as u16)
                .collect();
            game::data::StatueDef {
                deity: match st.get("deity").and_then(Value::as_i64).unwrap_or(0) {
                    1 => game::data::Deity::Worker,
                    2 => game::data::Deity::Knight,
                    3 => game::data::Deity::Farmer,
                    4 => game::data::Deity::Time,
                    _ => game::data::Deity::Archer,
                },
                charge_days: num(st, "chargeDurationInDays"),
                charge_multiplier: num(st, "chargeDurationMultiplier"),
                max_charge: num(st, "MaxCharge"),
                overgrown: self.sprite(st.get("overgrownSprite")),
                clean: self.sprite(st.get("cleanSprite")),
                charge_parts,
            }
        });
        let ballista = comp(node, "Ballista").map(|b| {
            let ball = node["children"].as_array().and_then(|cs| {
                cs.iter()
                    .find(|c| comp(c, "Ballista").is_some() || c["name"] == "Ballista")
            });
            let base = ball.map_or([0.0; 3], |n| vec3(&n["pos"]));
            let bolt = ball
                .and_then(|n| n["children"].as_array())
                .and_then(|cs| cs.iter().find(|c| c["name"] == "Bolt Position"))
                .map_or([0.0; 3], |n| vec3(&n["pos"]));
            let body_part = b
                .get("bodySprite")
                .and_then(|r| r.get("local"))
                .and_then(Value::as_i64)
                .and_then(|sr| part_ids.iter().position(|&i| i == sr))
                .map(|i| i as u16);
            let bolt_def = asset_path(b.get("boltPrefab"))
                .and_then(|p| self.inp.prefabs.get(p))
                .and_then(|n| comp(n, "Bolt"));
            let frames = b
                .get("reloadFrames")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|s| self.sprite(Some(s)))
                .collect();
            game::data::BallistaDef {
                bolt_pos: (base[0] + bolt[0], base[1] + bolt[1]),
                range: num(b, "shootRange"),
                min_range: num(b, "minRange"),
                angle: (
                    b["angleRange"]["min"].as_f64().unwrap_or(-15.0) as f32,
                    b["angleRange"]["max"].as_f64().unwrap_or(15.0) as f32,
                ),
                reload_work: b.get("reloadWork").and_then(Value::as_i64).unwrap_or(20) as i32,
                damage: bolt_def
                    .and_then(|f| f.get("damage"))
                    .and_then(Value::as_i64)
                    .unwrap_or(12) as i32,
                max_hits: bolt_def
                    .and_then(|f| f.get("maxHits"))
                    .and_then(Value::as_i64)
                    .unwrap_or(8) as u8,
                body_part,
                frames,
                work_pos: (-0.5, 2.0625),
            }
        });
        let shield_shop = comp(node, "PayableShield").map(|s| {
            let shield_pos = s
                .get("shieldPosition")
                .and_then(|r| r.get("local"))
                .and_then(Value::as_i64)
                .and_then(|id| child_by_transform(node, id))
                .map_or((0.0, 1.7), |c| {
                    let p = vec3(&c["pos"]);
                    (p[0], p[1])
                });
            // The Banner component's SpriteRenderer is the banner part.
            let banner_part = s
                .get("banner")
                .and_then(|r| r.get("local"))
                .and_then(Value::as_i64)
                .and_then(|bid| {
                    fn find(n: &Value, bid: i64) -> Option<&Value> {
                        if n["components"]
                            .as_array()
                            .is_some_and(|cs| cs.iter().any(|c| c["id"].as_i64() == Some(bid)))
                        {
                            return Some(n);
                        }
                        n["children"].as_array()?.iter().find_map(|c| find(c, bid))
                    }
                    find(node, bid)
                })
                .and_then(|n| comp_id(n, "SpriteRenderer"))
                .and_then(|sr| part_ids.iter().position(|&i| i == sr))
                .map(|i| i as u16);
            game::data::ShieldShopDef {
                shield_pos,
                banner_part,
                banner: self.sprite(s.get("bannerSprite")),
                torn: self.sprite(s.get("bannerSpriteTorn")),
            }
        });
        let gaps = comp(node, "GAPS").map(|g| {
            let r = &g["region"];
            game::data::Gaps {
                x: r["x"].as_f64().unwrap_or(0.0) as f32,
                width: r["width"].as_f64().unwrap_or(0.0) as f32,
                layers: g["layer"]["m_Bits"].as_u64().unwrap_or(0) as u32,
            }
        });
        let farmhouse = comp(node, "Farmhouse").map(|f| game::data::FarmhouseDef {
            update_interval: num(f, "updateInterval"),
            max_farmlands: f.get("maxFarmlands").and_then(Value::as_i64).unwrap_or(0) as u8,
            statue_extra_farmlands: f
                .get("farmerStatueExtraFarmlands")
                .and_then(Value::as_i64)
                .unwrap_or(0) as u8,
            left_seed_offset: num(f, "leftSeedOffset"),
            right_seed_offset: num(f, "rightSeedOffset"),
            level: f.get("level").and_then(Value::as_i64).unwrap_or(0) as u8,
        });
        let part_index = |sr: i64| part_ids.iter().position(|&i| i == sr).map(|i| i as u16);
        let wharf = comp(node, "Wharf").map(|w| {
            // orig: GetComponentsInChildren<BoatPart>: depth-first order.
            fn boat_parts<'v>(n: &'v Value, out: &mut Vec<&'v Value>) {
                for c in n["children"].as_array().into_iter().flatten() {
                    if comp(c, "BoatPart").is_some() {
                        out.push(c);
                    }
                    boat_parts(c, out);
                }
            }
            let mut nodes = Vec::new();
            boat_parts(node, &mut nodes);
            let assets = self.inp.assets_dir;
            let parts = nodes
                .into_iter()
                .filter_map(|c| {
                    let i = comp_id(c, "SpriteRenderer").and_then(part_index)?;
                    let sprite =
                        comp(c, "SpriteRenderer").and_then(|sr| asset_path(sr.get("m_Sprite")));
                    let (cx, cy) = sprite.map_or((0.0, 0.0), |p| sprite_center(assets, p));
                    Some((i, cx, cy))
                })
                .collect();
            let skeleton = w
                .get("skeletonBase")
                .and_then(|r| r["local"].as_i64())
                .and_then(|id| child_by_id(node, id).or_else(|| child_by_transform(node, id)));
            let boat = asset_path(w.get("boatPrefab"))
                .map(str::to_owned)
                .and_then(|p| self.prefab(&p));
            game::data::WharfDef {
                work_per_part: w.get("workPerPart").and_then(Value::as_i64).unwrap_or(4) as u8,
                parts,
                skeleton: skeleton
                    .and_then(|n| comp_id(n, "SpriteRenderer"))
                    .and_then(part_index),
                skeleton_at: skeleton.map_or((0.0, 0.0), |n| {
                    let p = vec3(&n["pos"]);
                    (p[0], p[1])
                }),
                boat,
            }
        });
        let boat = comp(node, "Boat").map(|b| {
            let pushes = node["children"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["name"] == "Push Positions"))
                .and_then(|n| n["children"].as_array())
                .into_iter()
                .flatten()
                .filter(|c| comp(c, "BoatPushPosition").is_some())
                .map(|c| vec3(&c["pos"])[0])
                .collect();
            fn named<'v>(n: &'v Value, name: &str) -> Option<&'v Value> {
                n["children"].as_array()?.iter().find_map(|c| {
                    if c["name"] == name {
                        Some(c)
                    } else {
                        named(c, name)
                    }
                })
            }
            game::data::BoatDef {
                water_height: num(b, "waterHeight"),
                push_speed: num(b, "pushSpeed"),
                forest_speed_multiplier: num(b, "forestSpeedMultiplier"),
                sail_speed: num(b, "sailSpeed"),
                max_workers: b.get("maxWorkers").and_then(Value::as_i64).unwrap_or(3) as u8,
                max_knights: b.get("maxKnights").and_then(Value::as_i64).unwrap_or(3) as u8,
                push_positions: pushes,
                sail: named(node, "Sail")
                    .and_then(|n| comp_id(n, "SpriteRenderer"))
                    .and_then(part_index),
            }
        });
        self.world.protos[id as usize] = Proto {
            fires: fires_under(node),
            name: node["name"].as_str().unwrap_or_default().to_owned(),
            tag: node["tag"].as_str().unwrap_or("Untagged").to_owned(),
            kind,
            parts,
            fixed,
            scatter,
            tree,
            half_width,
            pay_region,
            root_z: pos[2],
            payable: payable_def,
            upgrade,
            building,
            forest_item,
            castle_level,
            shop,
            tree_work,
            camp,
            character,
            wall,
            wall_def,
            portal,
            gaps,
            farmhouse,
            grass: comp(node, "Grass").is_some(),
            clear_grass: comp(node, "ClearSpaceOnStart").is_some_and(|c| flag(c, "clearGrass")),
            guard_slots: guard_slots(node),
            wall_level: comp(node, "Wall")
                .map(|c| c.get("level").and_then(Value::as_i64).unwrap_or(0) as u8),
            tower_level: comp(node, "Tower")
                .map(|c| c.get("level").and_then(Value::as_i64).unwrap_or(0) as u8),
            shield_spots: comp(node, "Castle")
                .and_then(|c| c.get("shieldShopPlaceholders"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| r.get("local").and_then(Value::as_i64))
                .filter_map(|id| child_by_id(node, id))
                .map(|c| vec3(&c["pos"])[0])
                .collect(),
            keep_time: comp(node, "Castle")
                .and(child_with(node, "WorkableBuilding"))
                .and_then(|k| comp(k, "WorkableBuilding"))
                .map_or(0.0, |w| {
                    let rate = num(w, "autoBuildRate");
                    if rate > 0.0 {
                        num(w, "buildPoints") / rate
                    } else {
                        0.0
                    }
                }),
            shield_shop,
            tower_knight: comp(node, "TowerKnight").is_some(),
            banker: comp(node, "Castle")
                .and_then(|c| asset_path(c.get("bankerPrefab")))
                .map(str::to_owned)
                .and_then(|p| self.prefab(&p)),
            statue,
            mason_shrine: comp(node, "MasonShrine").is_some(),
            steed: None,
            cabin: comp(node, "Cabin")
                .and_then(|c| asset_path(c.get("hermitPrefab")))
                .map(str::to_owned)
                .and_then(|p| self.prefab(&p)),
            hermit: comp(node, "Hermit")
                .and_then(|_| node["tag"].as_str())
                .and_then(game::data::HermitKind::from_tag),
            passenger_upgrades: comp(node, "PayableUpgrade")
                .and_then(|u| u.get("passengerUpgrades"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| {
                    let kind = game::data::HermitKind::from_tag(r["tag"].as_str()?)?;
                    let path = asset_path(r.get("prefab"))?.to_owned();
                    Some((kind, self.prefab(&path)?))
                })
                .collect(),
            baker: comp(node, "Baker").map(|b| {
                (
                    b.get("maxBeggars").and_then(Value::as_i64).unwrap_or(3) as u8,
                    num(b, "callBeggarInterval"),
                )
            }),
            ballista,
            merchant_spawner: comp(node, "MerchantSpawner")
                .and_then(|m| asset_path(m.get("merchantPrefab")))
                .map(str::to_owned)
                .and_then(|p| self.prefab(&p)),
            workshop: node["children"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| comp(c, "PayableWorkshop").is_some())
                .map(|c| {
                    let x = vec3(&c["pos"])[0];
                    (self.proto(c), x)
                })
                .collect(),
            workshop_side: comp(node, "PayableWorkshop").and_then(|w| {
                let side = w.get("side").and_then(Value::as_i64).unwrap_or(1) as i8;
                let path = asset_path(w.get("catapultPrefab"))?.to_owned();
                Some((side, self.prefab(&path)?))
            }),
            catapult: comp(node, "Catapult").map(|c| {
                let boulder = asset_path(c.get("boulderPrefab"))
                    .and_then(|p| self.inp.prefabs.get(p))
                    .and_then(|n| comp(n, "Boulder"));
                // Spoon: Flippable/Arm/Spoon.
                let mut spoon = (0.19, 1.03);
                fn find_pos(n: &Value, name: &str, at: (f32, f32)) -> Option<(f32, f32)> {
                    for ch in n["children"].as_array()? {
                        let p = vec3(&ch["pos"]);
                        let here = (at.0 + p[0], at.1 + p[1]);
                        if ch["name"] == name {
                            return Some(here);
                        }
                        if let Some(r) = find_pos(ch, name, here) {
                            return Some(r);
                        }
                    }
                    None
                }
                if let Some(s) = find_pos(node, "Spoon", (0.0, 0.0)) {
                    spoon = s;
                }
                game::data::CatapultDef {
                    crank_rate: num(c, "crankRate"),
                    force: (
                        c["projectileForce"]["min"].as_f64().unwrap_or(7.0) as f32,
                        c["projectileForce"]["max"].as_f64().unwrap_or(8.0) as f32,
                    ),
                    scan: (
                        c["scanRange"]["min"].as_f64().unwrap_or(7.0) as f32,
                        c["scanRange"]["max"].as_f64().unwrap_or(8.0) as f32,
                    ),
                    hit_points: hp,
                    boulder_damage: boulder
                        .and_then(|b| b.get("hitDamage"))
                        .and_then(Value::as_i64)
                        .unwrap_or(15) as i32,
                    stuck_probability: boulder.map_or(0.5, |b| num(b, "StuckProbability")),
                    spoon,
                }
            }),
            teleporter: comp(node, "PayableTeleporter").map(|t| num(t, "cooldown")),
            dog_spawn: comp(node, "DogSpawn").map(|d| {
                let trapped = node["children"]
                    .as_array()
                    .and_then(|cs| cs.iter().find(|c| c["name"] == "Trapped Dog"));
                game::data::DogSpawnDef {
                    colors: d
                        .get("colorOptions")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|c| {
                            [c["r"].as_f64(), c["g"].as_f64(), c["b"].as_f64()]
                                .map(|v| v.unwrap_or(1.0) as f32)
                        })
                        .collect(),
                    trapped_part: trapped
                        .and_then(|t| comp_id(t, "SpriteRenderer"))
                        .and_then(|sr| part_ids.iter().position(|&i| i == sr))
                        .map(|i| i as u16),
                    trapped_x: trapped.map_or(0.4, |t| vec3(&t["pos"])[0]),
                }
            }),
            rift: comp(node, "TeleporterRift").is_some(),
            chest: comp(node, "Chest").map(|c| {
                (
                    c.get("coins").and_then(Value::as_i64).unwrap_or(0) as u8,
                    (
                        c["spillRange"]["min"].as_f64().unwrap_or(-3.0) as f32,
                        c["spillRange"]["max"].as_f64().unwrap_or(3.0) as f32,
                    ),
                )
            }),
            banker_def: comp(node, "Banker").map(|b| game::data::BankerDef {
                walk: num(b, "walkSpeed"),
                run: num(b, "runSpeed"),
                wander_range: num(b, "wanderRange"),
                coin_scan_range: num(b, "coinScanRange"),
                payout_portion: num(b, "payoutPortion"),
                daily_interest: num(b, "dailyInterest"),
                max_interest: b.get("maxInterest").and_then(Value::as_i64).unwrap_or(8) as i32,
                player_max_coins: b
                    .get("playerMaxCoins")
                    .and_then(Value::as_i64)
                    .unwrap_or(30) as u32,
                payout_interval: num(b, "payoutInterval"),
                max_coins: comp(node, "Wallet")
                    .and_then(|w| w.get("maxCoins"))
                    .and_then(Value::as_i64)
                    .unwrap_or(10) as u32,
            }),
            merchant: comp(node, "Merchant").map(|m| {
                (
                    m.get("numCoins").and_then(Value::as_i64).unwrap_or(8) as u8,
                    num(m, "supplyTimeInHours"),
                )
            }),
            steed_spawn: comp(node, "SteedSpawn").map(|s| {
                let picks = s
                    .get("steeds")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|r| asset_path(Some(r)).map(str::to_owned))
                    .filter_map(|p| self.steed(&p))
                    .collect();
                (
                    s.get("amount").and_then(Value::as_i64).unwrap_or(1) as u8,
                    picks,
                )
            }),
            border: comp(node, "PayableBorder").map(|b| game::data::BorderDef {
                distance_from_wall: num(b, "distanceFromWall"),
                min_campfire_distance: num(b, "minCampfireDistance"),
                knight_range: num(b, "knightRange"),
            }),
            wharf,
            boat,
            beach: comp(node, "Beach").is_some(),
            signpost: comp(node, "Signpost")
                .and_then(|s| s.get("map"))
                .and_then(|m| m["local"].as_i64())
                .and_then(|sfx| {
                    // The map's SpriteFX sits beside its renderer.
                    node["children"].as_array()?.iter().find_map(|c| {
                        (comp_id(c, "SpriteFX") == Some(sfx))
                            .then(|| comp_id(c, "SpriteRenderer"))
                            .flatten()
                    })
                })
                .and_then(part_index),
        };
    }

    /// orig: the main scene's `CoinBag` and `BagCoin.prefab`.
    /// orig: `Holder.firePrefab` (`Fire.prefab`), its flames and glow, and
    /// `Player.torchBrightnessThreshold`.
    fn fire_style(&mut self) {
        let inp = self.inp;
        let keys = |f: &Map<String, Value>, k: &str| -> Vec<(f32, f32)> {
            f.get(k)
                .and_then(|c| c["m_Curve"].as_array())
                .into_iter()
                .flatten()
                .map(|k| {
                    (
                        k["time"].as_f64().unwrap_or(0.0) as f32,
                        k["value"].as_f64().unwrap_or(0.0) as f32,
                    )
                })
                .collect()
        };
        let mut style = game::data::FireStyleDef {
            glow_scale: 1.0,
            torch_threshold: 0.5,
            ..Default::default()
        };
        if let Some((node, f)) = inp
            .prefabs
            .values()
            .find_map(|n| comp(n, "Fire").map(|f| (n, f)))
        {
            style.strength = keys(f, "sizeToStrengthCurve");
            style.intensity = keys(f, "intensityCurve");
            style.range = keys(f, "rangeCurve");
            style.ambient = keys(f, "ambientIntensity");
            for c in node["children"].as_array().into_iter().flatten() {
                if let Some(l) = comp(c, "Class108") {
                    let col = &l["m_Color"];
                    style.light_color =
                        ["r", "g", "b"].map(|k| col[k].as_f64().unwrap_or(1.0) as f32);
                }
                if let Some(sr) = comp(c, "SpriteRenderer") {
                    style.glow = self.sprite(sr.get("m_Sprite"));
                    let col = &sr["m_Color"];
                    style.glow_color =
                        ["r", "g", "b", "a"].map(|k| col[k].as_f64().unwrap_or(1.0) as f32);
                    style.glow_scale = vec3(&c["scale"])[0];
                }
            }
            for flame in f
                .get("flamePrefabs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(fnode) = asset_path(Some(flame)).and_then(|p| inp.prefabs.get(p)) else {
                    continue;
                };
                fn first_sprite(n: &Value) -> Option<&Value> {
                    comp(n, "SpriteRenderer")
                        .and_then(|sr| sr.get("m_Sprite"))
                        .filter(|s| s.get("asset").is_some())
                        .or_else(|| n["children"].as_array()?.iter().find_map(first_sprite))
                }
                let size = comp(fnode, "Flame").map_or((0.25, 0.25), |fl| {
                    (
                        fl["size"]["x"].as_f64().unwrap_or(0.25) as f32,
                        fl["size"]["y"].as_f64().unwrap_or(0.25) as f32,
                    )
                });
                let sprite = first_sprite(fnode).and_then(|s| self.sprite(Some(s)));
                style.flames.push((size, sprite));
            }
        } else {
            eprintln!("warning: no Fire prefab");
        }
        if let Some(p) = inp.prefabs.values().find_map(|n| comp(n, "Player")) {
            style.torch_threshold = num(p, "torchBrightnessThreshold");
        }
        self.world.fire = style;
    }

    /// orig: `Level/Water` with its `Water` script and `Water Material`.
    fn water(&mut self) {
        let inp = self.inp;
        let w = &mut self.world.water;
        if let Some((node, f)) = find_component(inp.main, "Water") {
            let pos = vec3(&node["pos"]);
            let scale = vec3(&node["scale"]);
            let half = scale[1].abs() / 2.0;
            (w.top, w.bottom, w.z) = (pos[1] + half, pos[1] - half, pos[2]);
            w.reflection_scale = num(f, "reflectionScale");
            w.max_displacement = (
                f["maxDisplacement"]["x"].as_f64().unwrap_or(16.0) as f32,
                f["maxDisplacement"]["y"].as_f64().unwrap_or(16.0) as f32,
            );
        } else {
            eprintln!("warning: no Water in the main scene");
        }
        // The material's maps: red and green channels of each.
        let load = |name: &str| -> Option<(u16, u16, Vec<[u8; 2]>)> {
            let img = crate::sprites::Image::load_png(&inp.assets_dir.join("Texture2D").join(name))
                .ok()?;
            let (iw, ih) = (img.w as usize, img.h as usize);
            // PNG rows run top-down; Unity's v runs bottom-up.
            let texels = (0..ih)
                .rev()
                .flat_map(|y| (0..iw).map(move |x| (y * iw + x) * 4))
                .map(|i| [img.px[i], img.px[i + 1]])
                .collect();
            Some((iw as u16, ih as u16, texels))
        };
        match (load("water_coarse.png"), load("water_fine.png")) {
            (Some((cw, ch, coarse)), Some((fw, fh, fine))) if (cw, ch) == (fw, fh) => {
                w.size = (cw, ch);
                w.coarse = coarse;
                w.fine = fine;
            }
            _ => eprintln!("warning: water displacement maps missing"),
        }
        if let Some(f) = std::fs::read_to_string(inp.assets_dir.join("Material/Water Material.mat"))
            .ok()
            .and_then(|t| {
                let i = t.find("_Displacement1Tex:")?;
                let j = t[i..].find("m_Scale: {x: ")? + i + "m_Scale: {x: ".len();
                t[j..].split(',').next()?.trim().parse::<f32>().ok()
            })
        {
            w.repeat.0 = f;
        }
    }

    fn bag(&mut self) {
        let inp = self.inp;
        let Some((node, cb)) = find_component(inp.main, "CoinBag") else {
            return;
        };
        let path = |n: &Value| -> Vec<(f32, f32)> {
            let Some(c) = comp(n, "PolygonCollider2D") else {
                return Vec::new();
            };
            let off = vec3(&n["pos"]);
            c["m_Points"]["m_Paths"][0]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| {
                    (
                        p["x"].as_f64().unwrap_or(0.0) as f32 + off[0],
                        p["y"].as_f64().unwrap_or(0.0) as f32 + off[1],
                    )
                })
                .collect()
        };
        let child = |name: &str| {
            node["children"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["name"] == name))
        };
        let sr = |b: &mut Self, name: &str| {
            child(name)
                .and_then(|c| comp(c, "SpriteRenderer"))
                .and_then(|r| r.get("m_Sprite").cloned())
                .and_then(|s| b.sprite(Some(&s)))
        };
        let mut def = game::data::BagDef {
            back: sr(self, "Back"),
            front: sr(self, "Front"),
            closed: sr(self, "Closed"),
            wall: child("Collider").map(path).unwrap_or_default(),
            spill: child("Ground").map(path).unwrap_or_default(),
            spawn_interval: num(cb, "coinSpawnInterval"),
            ..Default::default()
        };
        let coin = asset_path(cb.get("bagCoinPrefab")).and_then(|p| inp.prefabs.get(p));
        if let Some(coin) = coin {
            def.coin = comp(coin, "SpriteRenderer")
                .and_then(|r| r.get("m_Sprite").cloned())
                .and_then(|s| self.sprite(Some(&s)));
            if let Some(bc) = comp(coin, "BagCoin") {
                def.coin_colors = bc["colors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| {
                        [c["r"].as_f64(), c["g"].as_f64(), c["b"].as_f64()]
                            .map(|v| v.unwrap_or(1.0) as f32)
                    })
                    .collect();
            }
            if let Some(rb) = comp(coin, "Rigidbody2D") {
                def.gravity_scale = num(rb, "m_GravityScale");
                def.linear_drag = num(rb, "m_LinearDrag");
                def.angular_drag = num(rb, "m_AngularDrag");
            }
            if let Some(col) = child_with(coin, "PolygonCollider2D") {
                def.coin_shape = path(col);
                let mat = comp(col, "PolygonCollider2D")
                    .and_then(|c| asset_path(c.get("m_Material")))
                    .and_then(|p| std::fs::read_to_string(inp.assets_dir.join(p)).ok())
                    .unwrap_or_default();
                let field = |k: &str| {
                    mat.lines()
                        .find_map(|l| l.trim().strip_prefix(k))
                        .and_then(|v| v.trim().parse::<f32>().ok())
                };
                def.friction = field("friction:").unwrap_or(0.1);
                def.bounciness = field("bounciness:").unwrap_or(0.5);
            }
        }
        self.world.bag = def;
    }

    /// orig: the loader scene's `Map`.
    fn map(&mut self) {
        let inp = self.inp;
        let Some((node, _)) = find_component(inp.loader, "Map") else {
            return;
        };
        let spr = |b: &mut Self, n: &Value| {
            n["components"]
                .as_array()
                .into_iter()
                .flatten()
                .find_map(|c| c["fields"].get("m_Sprite").cloned())
                .and_then(|s| b.sprite(Some(&s)))
        };
        let pos = |n: &Value| {
            let p = vec3(&n["pos"]);
            (p[0], p[1])
        };
        let flipped = |n: &Value| vec3(&n["scale"])[0] < 0.0;
        let named = |n: &'a Value, name: &str| -> Option<&'a Value> {
            n["children"].as_array()?.iter().find(|c| c["name"] == name)
        };
        let Some(back) = named(node, "Background") else {
            return;
        };
        let Some(group) = named(back, "Group") else {
            return;
        };
        let mut def = game::data::MapDef {
            back: spr(self, back),
            ..Default::default()
        };
        if let Some(d) = named(group, "Detail") {
            for c in d["children"].as_array().into_iter().flatten() {
                if let Some(s) = spr(self, c) {
                    def.details.push((s, pos(c)));
                }
            }
        }
        let mut lands: Vec<(i64, game::data::MapLandDef)> = Vec::new();
        for l in group["children"].as_array().into_iter().flatten() {
            let Some(ml) = comp(l, "MapLand") else {
                continue;
            };
            let mut land = game::data::MapLandDef {
                sprite: spr(self, l),
                at: pos(l),
                ..Default::default()
            };
            for c in l["children"].as_array().into_iter().flatten() {
                let name = c["name"].as_str().unwrap_or("");
                if let Some(u) = comp(c, "MapUnlock") {
                    let icon = named(c, "Icon");
                    land.unlocks.push(game::data::MapUnlockDef {
                        unlock: u.get("unlockID").and_then(Value::as_i64).unwrap_or(0) as u8,
                        at: pos(c),
                        icon: icon.and_then(|i| spr(self, i)),
                        icon_at: icon.map_or((0.0, 0.0), pos),
                        map_at: named(c, "Map").map_or((0.0, 10.0), pos),
                    });
                } else if name == "MapBoat" {
                    land.boat = (pos(c), flipped(c));
                } else if name == "Pointer" {
                    land.pointer = (pos(c), flipped(c));
                } else if name.starts_with("Cloud") {
                    if let Some(s) = spr(self, c) {
                        land.clouds.push((s, pos(c)));
                    }
                }
            }
            lands.push((ml.get("land").and_then(Value::as_i64).unwrap_or(0), land));
        }
        lands.sort_by_key(|l| l.0);
        def.lands = lands.into_iter().map(|l| l.1).collect();
        let frames = |b: &mut Self, base: &str| -> Vec<SpriteId> {
            (0..8)
                .map_while(|i| {
                    let name = format!("{base}_{i}");
                    inp.sprite_sizes
                        .contains_key(&name)
                        .then(|| b.named_sprite(&name))
                })
                .collect()
        };
        def.boat = frames(self, "map_boat");
        def.wreck = frames(self, "map_boat_wreck");
        def.pointer = frames(self, "map_pointer");
        def.light_patch = Some(self.named_sprite("map_lightpatch"));
        def.map_icon = Some(self.named_sprite("map_icon_map"));
        def.button = Some(self.named_sprite("map_button_normal"));
        // The loader's RectTransforms aren't exported; the map's is 300×200.
        def.back_size = (300.0, 200.0);
        let border = |name: &str| {
            let text = std::fs::read_to_string(inp.assets_dir.join(format!("Sprite/{name}.asset")))
                .unwrap_or_default();
            let line = text
                .lines()
                .find(|l| l.trim_start().starts_with("m_Border:"))
                .unwrap_or("");
            let v = |k: &str| {
                line.split(k)
                    .nth(1)
                    .and_then(|r| r.split([',', '}']).next())
                    .and_then(|n| n.trim().parse::<f32>().ok())
                    .unwrap_or(0.0)
            };
            (v("x:"), v("y:"), v("z:"), v("w:"))
        };
        def.back_border = border("map_back");
        def.button_border = border("map_button_normal");
        self.world.map = def;
    }

    /// A sprite by its exported name.
    fn named_sprite(&mut self, name: &str) -> SpriteId {
        if let Some(&id) = self.sprite_ids.get(name) {
            return id;
        }
        let id = self.world.sprites.len() as SpriteId;
        self.world.sprites.push(name.to_owned());
        self.sprite_ids.insert(name.to_owned(), id);
        id
    }

    /// A steed prefab, registered once in `WorldData::steeds`.
    fn steed(&mut self, path: &str) -> Option<u16> {
        let proto = self.prefab(path)?;
        if let Some(i) = self.world.steeds.iter().position(|s| s.proto == proto) {
            return Some(i as u16);
        }
        let node = self.inp.prefabs.get(path)?;
        let f = comp(node, "Steed")?;
        let color = |k: &str| {
            [f[k]["r"].as_f64(), f[k]["g"].as_f64(), f[k]["b"].as_f64()]
                .map(|c| c.unwrap_or(1.0) as f32)
        };
        let clips = comp(node, "Animator")
            .and_then(|a| asset_path(a.get("m_Controller")))
            .and_then(|p| Path::new(p).file_stem())
            .map(|s| {
                s.to_string_lossy()
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .to_owned()
            })
            .unwrap_or_else(|| "player_horse".to_owned());
        let rider = node["children"]
            .as_array()
            .and_then(|cs| cs.iter().find(|c| c["name"] == "King"))
            .map_or((0.0, 0.0), |k| {
                let p = vec3(&k["pos"]);
                (p[0], p[1])
            });
        let hb = &f["hitBox"];
        let def = game::data::SteedDef {
            name: node["name"].as_str().unwrap_or_default().to_owned(),
            proto,
            clips,
            walk_speed: num(f, "walkSpeed"),
            run_speed: num(f, "runSpeed"),
            forest_speed_multiplier: num(f, "forestSpeedMultiplier"),
            walk_stamina_rate: num(f, "walkStaminaRate"),
            run_stamina_rate: num(f, "runStaminaRate"),
            stand_stamina_rate: num(f, "standStaminaRate"),
            eat_delay: num(f, "eatDelay"),
            eat_duration: num(f, "eatDuration"),
            rearing_cooldown: num(f, "rearingCooldown"),
            rearing_duration: num(f, "rearingDuration"),
            well_fed_duration: num(f, "wellFedDuration"),
            tired_duration: num(f, "tiredDuration"),
            reserve_stamina: num(f, "reserveStamina"),
            reserve_probability: num(f, "reserveProbability"),
            attacks_while_running: flag(f, "attacksWhileRunning"),
            hit_box: (
                hb["x"].as_f64().unwrap_or(0.0) as f32,
                hb["y"].as_f64().unwrap_or(0.0) as f32,
                hb["width"].as_f64().unwrap_or(0.0) as f32,
                hb["height"].as_f64().unwrap_or(0.0) as f32,
            ),
            attack_damage: f.get("attackDamage").and_then(Value::as_i64).unwrap_or(0) as i32,
            attack_stamina_cost: num(f, "attackStaminaCost"),
            attracts_deer: flag(f, "attractsDeer"),
            poop_coins: f
                .get("poopCoinsWhenEating")
                .and_then(Value::as_i64)
                .unwrap_or(0) as u8,
            inspire_citizens: flag(f, "inspireCitizens"),
            inspired_duration: num(f, "inspiredDuration"),
            inspired_range: num(f, "inspiredRange"),
            ability_cooldown: num(f, "abilityCooldown"),
            recolor_to_coat: flag(f, "recolorToCoatOfArms"),
            disable_switching: flag(f, "disableSteedSwitching"),
            does_not_need_grass: flag(f, "doesNotNeedGrass"),
            wander_range: num(f, "wanderRange"),
            coat: color("coatColor"),
            mane: color("maneColor"),
            rider,
            torch: node["children"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["name"] == "King"))
                .and_then(|k| {
                    let at = vec3(&k["pos"]);
                    fires_under(k).into_iter().next().map(|mut f| {
                        f.at = (f.at.0 + at[0], f.at.1 + at[1], f.at.2 + at[2]);
                        f
                    })
                }),
            passenger: node["children"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["name"] == "Passenger"))
                .map_or((-0.28125, 0.5625), |k| {
                    let p = vec3(&k["pos"]);
                    (p[0], p[1])
                }),
        };
        let i = self.world.steeds.len() as u16;
        self.world.steeds.push(def);
        self.world.protos[proto as usize].steed = Some(i);
        Some(i)
    }

    /// orig: `Holder.farmlandPrefab`.
    fn farmland(&mut self, path: &str) -> Option<game::data::FarmlandDef> {
        let proto = self.prefab(path)?;
        let node = self.inp.prefabs.get(path)?;
        let f = comp(node, "Farmland")?;
        let sprites = |b: &mut Self, k: &str| -> Vec<SpriteId> {
            f.get(k)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|s| b.sprite(Some(s)))
                .collect()
        };
        let frames = sprites(self, "frames");
        let back_frames = sprites(self, "backgroundFrames");
        // Parts are flattened depth-first: the root renderer, then children in order.
        let back_id = f
            .get("backgroundRenderer")
            .and_then(|r| r.get("local"))
            .and_then(Value::as_i64);
        let mut ids = Vec::new();
        let mut parts = Vec::new();
        self.parts(node, [0.0; 3], (1.0, 1.0), None, &mut parts, &mut ids);
        let back_part = back_id
            .and_then(|b| ids.iter().position(|&i| i == b))
            .unwrap_or(0) as u16;
        let width = comp(node, "GAPS")
            .and_then(|g| g["region"]["width"].as_f64())
            .unwrap_or(2.0) as f32;
        Some(game::data::FarmlandDef {
            proto,
            farm_points: f.get("farmPoints").and_then(Value::as_i64).unwrap_or(10) as i32,
            coin_yield: f.get("coinYield").and_then(Value::as_i64).unwrap_or(0) as i32,
            decay_interval: num(f, "decayInterval"),
            coin_drop_force: (
                f["coinDropForce"]["x"].as_f64().unwrap_or(0.0) as f32,
                f["coinDropForce"]["y"].as_f64().unwrap_or(0.0) as f32,
            ),
            frames,
            back_frames,
            decayed: (
                self.sprite(f.get("decayedFrame")),
                self.sprite(f.get("decayedBackgroundFrame")),
            ),
            frozen: (
                self.sprite(f.get("frozenFrame")),
                self.sprite(f.get("frozenBackgroundFrame")),
            ),
            stages: curve(&f["stages"]),
            spacing: num(f, "spacing"),
            front_part: 0,
            back_part,
            width,
        })
    }

    fn block(&mut self, node: &Value) -> Option<BlockDef> {
        let lb = comp(node, "LevelBlock")?;
        let mut items = Vec::new();
        let mut tiles: Vec<(f32, f32)> = Vec::new();
        for layer_node in node["children"].as_array().into_iter().flatten() {
            let Some(layer) = layer_node["name"].as_str().and_then(Layer::from_name) else {
                continue;
            };
            let layer_active = layer_node["active"] == true;
            for item in layer_node["children"].as_array().into_iter().flatten() {
                let pos = vec3(&item["pos"]);
                let proto = self.proto(item);
                // Width counts every tile, active or not. orig: GetComponentsInChildren<Tile>(true).
                if let Kind::Tile { width, .. } = self.world.protos[proto as usize].kind {
                    tiles.push((pos[0], pos[0] + width));
                }
                if layer_active && item["active"] == true {
                    items.push(Placed {
                        proto,
                        layer,
                        x: pos[0],
                        y: pos[1],
                        z: pos[2],
                        scale_x: vec3(&item["scale"])[0].signum(),
                    });
                }
            }
        }
        tiles.sort_by(|a, b| a.0.total_cmp(&b.0));
        let left = tiles.first().map_or(0.0, |t| t.0);
        let right = tiles.last().map_or(0.0, |t| t.1);
        let group = |k: &str| lb.get(k).and_then(Value::as_i64).unwrap_or(0) as u8;
        Some(BlockDef {
            name: node["name"].as_str().unwrap_or_default().to_owned(),
            needs_unlock: flag(lb, "needsUnlock"),
            unlock_id: lb.get("unlockID").and_then(Value::as_i64).unwrap_or(0) as i32,
            min_distance: num(lb, "minDistance"),
            max_distance: num(lb, "maxDistance"),
            maximize_mutual_distance: flag(lb, "maximizeMutualDistance"),
            side: match lb.get("side").and_then(Value::as_i64) {
                Some(1) => Side::Left,
                Some(2) => Side::Right,
                Some(3) => Side::Distribute,
                _ => Side::Any,
            },
            absolute_center: flag(lb, "absoluteCenter"),
            groups: [group("groupOne"), group("groupTwo"), group("groupThree")],
            left,
            width: (right - left).floor() as i32,
            items,
        })
    }
}

/// The child GameObject with this file id.
fn child_by_id(node: &Value, id: i64) -> Option<&Value> {
    node["children"].as_array()?.iter().find_map(|c| {
        if c["id"].as_i64() == Some(id) {
            Some(c)
        } else {
            child_by_id(c, id)
        }
    })
}

/// The child whose Transform has this file id.
fn child_by_transform(node: &Value, id: i64) -> Option<&Value> {
    node["children"].as_array()?.iter().find_map(|c| {
        if comp_id(c, "Transform") == Some(id) {
            Some(c)
        } else {
            child_by_transform(c, id)
        }
    })
}

/// The first descendant with a component of type `ty`.
fn child_with<'v>(node: &'v Value, ty: &str) -> Option<&'v Value> {
    node["children"].as_array()?.iter().find_map(|c| {
        if comp(c, ty).is_some() {
            Some(c)
        } else {
            child_with(c, ty)
        }
    })
}

/// The middle of a sprite's rect relative to its pivot, in units (stands in
/// for the centroid of a `BoatPart`'s outline).
fn sprite_center(assets: &Path, path: &str) -> (f32, f32) {
    let Ok(text) = std::fs::read_to_string(assets.join(path)) else {
        return (0.0, 0.0);
    };
    let field = |key: &str, after: &str| -> f32 {
        text.find(after)
            .and_then(|i| {
                let rest = &text[i..];
                let j = rest.find(key)? + key.len();
                let v = rest[j..].trim_start();
                let end = v.find([',', '}', '\n']).unwrap_or(v.len());
                v[..end].trim().parse().ok()
            })
            .unwrap_or(0.0)
    };
    let (w, h) = (field("width:", "m_Rect:"), field("height:", "m_Rect:"));
    let (px, py) = (field("x:", "m_Pivot:"), field("y:", "m_Pivot:"));
    let ppu = match field("m_PixelsToUnits:", "m_PixelsToUnits:") {
        v if v > 0.0 => v,
        _ => 32.0,
    };
    ((0.5 - px) * w / ppu, (0.5 - py) * h / ppu)
}

/// `FirePlaceholder`s under `node`, positioned relative to it.
fn fires_under(node: &Value) -> Vec<game::data::FireDef> {
    fn walk(n: &Value, at: [f32; 3], out: &mut Vec<game::data::FireDef>) {
        for c in n["children"].as_array().into_iter().flatten() {
            let p = vec3(&c["pos"]);
            let here = [at[0] + p[0], at[1] + p[1], at[2] + p[2]];
            if let Some(f) = comp(c, "FirePlaceholder") {
                if flag(f, "m_Enabled") || f.get("m_Enabled").is_none() {
                    out.push(game::data::FireDef {
                        at: (here[0], here[1], here[2]),
                        size: (
                            f["size"]["x"].as_f64().unwrap_or(0.25) as f32,
                            f["size"]["y"].as_f64().unwrap_or(0.25) as f32,
                        ),
                        night_only: flag(f, "burnsAtNightOnly"),
                        auto_start: flag(f, "autoStart"),
                        real_light: flag(f, "forceRealLight"),
                    });
                }
            }
            walk(c, here, out);
        }
    }
    let mut out = Vec::new();
    walk(node, [0.0; 3], &mut out);
    out
}

/// `GuardSlot` positions under `node`, relative to it.
fn guard_slots(node: &Value) -> Vec<(f32, f32)> {
    fn walk(n: &Value, at: (f32, f32), out: &mut Vec<(f32, f32)>) {
        for c in n["children"].as_array().into_iter().flatten() {
            if c["active"] != true {
                continue;
            }
            let p = vec3(&c["pos"]);
            let here = (at.0 + p[0], at.1 + p[1]);
            if comp(c, "GuardSlot").is_some() {
                out.push(here);
            }
            walk(c, here, out);
        }
    }
    let mut out = Vec::new();
    walk(node, (0.0, 0.0), &mut out);
    out
}

fn find_component<'v>(roots: &'v Value, ty: &str) -> Option<(&'v Value, &'v Map<String, Value>)> {
    fn walk<'v>(n: &'v Value, ty: &str) -> Option<(&'v Value, &'v Map<String, Value>)> {
        if let Some(c) = comp(n, ty) {
            return Some((n, c));
        }
        n["children"].as_array()?.iter().find_map(|c| walk(c, ty))
    }
    roots["roots"].as_array()?.iter().find_map(|r| walk(r, ty))
}

/// orig: `AnimationCurve` keys.
fn curve(v: &Value) -> Curve {
    let f = |k: &Value| match k {
        Value::String(s) if s == "Infinity" => f32::INFINITY,
        Value::String(s) if s == "-Infinity" => f32::NEG_INFINITY,
        other => other.as_f64().unwrap_or(0.0) as f32,
    };
    Curve {
        keys: v["m_Curve"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|k| {
                [
                    f(&k["time"]),
                    f(&k["value"]),
                    f(&k["inSlope"]),
                    f(&k["outSlope"]),
                ]
            })
            .collect(),
    }
}

fn level_config(name: &str, f: &Map<String, Value>, waves: &HashMap<&str, u16>) -> LevelConfigDef {
    let wave = |k: &str| asset_path(f.get(k)).and_then(|p| waves.get(p).copied());
    // orig: Director/EnemyManager.StoreWaveReferences: seasonal waves only when overridden.
    let seasonal = |base: &str| {
        let summer = wave(base);
        let autumn = if flag(f, "overrideAutumnWaves") {
            wave(&format!("{base}Autumn"))
        } else {
            summer
        };
        let winter = if flag(f, "overrideWinterWaves") {
            wave(&format!("{base}Winter"))
        } else {
            summer
        };
        [summer, autumn, winter]
    };
    let timeframe = &f["waveMultiplierTimeframe"];
    let range = |k: &str| {
        let r = &f[k];
        (
            r["min"].as_i64().unwrap_or(0) as i32,
            r["max"].as_i64().unwrap_or(0) as i32,
        )
    };
    let mut group_counts = vec![(0, 0); 64];
    for g in f
        .get("groupCounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let idx = g["group"].as_i64().unwrap_or(0) as usize;
        if idx < group_counts.len() {
            group_counts[idx] = (
                g["count"]["min"].as_i64().unwrap_or(0) as i32,
                g["count"]["max"].as_i64().unwrap_or(0) as i32,
            );
        }
    }
    LevelConfigDef {
        name: name.to_owned(),
        autumn_start: range("autumnStart"),
        winter_start: range("winterStart"),
        days_before_game_over: f
            .get("daysBeforeGameOver")
            .and_then(Value::as_i64)
            .unwrap_or(-1) as i32,
        starting_coins: f.get("startingCoins").and_then(Value::as_i64).unwrap_or(0) as i32,
        min_level_width: f.get("minLevelWidth").and_then(Value::as_i64).unwrap_or(0) as i32,
        group_counts,
        waves: [
            seasonal("regularWave"),
            seasonal("bossWave"),
            seasonal("recoveryWave"),
        ],
        defence_waves: seasonal("portalDefenceWave"),
        retaliation_waves: seasonal("retaliationWave"),
        wave_timed_multiplier: f
            .get("waveTimedMultiplier")
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32,
        wave_multiplier_timeframe: (
            timeframe["min"].as_i64().unwrap_or(0) as i32,
            timeframe["max"].as_i64().unwrap_or(-1) as i32,
        ),
        season_change_days: f
            .get("seasonChangeDays")
            .and_then(Value::as_f64)
            .unwrap_or(3.0) as f32,
        income_multiplier: f
            .get("incomeMultiplier")
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32,
        free_boat_parts: f.get("freeBoatParts").and_then(Value::as_i64).unwrap_or(0) as u8,
        wave_boat_multiplier: f
            .get("waveBoatMultiplier")
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32,
        force_all_unlocks: flag(f, "forceAllUnlocks"),
    }
}

/// Legacy world data, audio clip source paths, and visual metadata by prototype ID.
pub fn build(inp: &Inputs) -> Result<(WorldData, Vec<String>, Vec<PrefabVisual>), String> {
    let mut b = Builder {
        inp,
        world: WorldData::default(),
        visuals: Vec::new(),
        sprite_ids: HashMap::new(),
        prefab_protos: HashMap::new(),
        recolor: recolor_materials(inp.assets_dir),
    };

    // Layers from the main scene's Level object.
    let (level_node, level) = find_component(inp.main, "Level").ok_or("main scene has no Level")?;
    b.world.layers = Layer::ALL
        .iter()
        .map(|l| {
            let node = level_node["children"].as_array().and_then(|cs| {
                cs.iter()
                    .find(|c| c["name"].as_str().and_then(Layer::from_name) == Some(*l))
            });
            let ratio = node
                .and_then(|n| comp(n, "Parallax2D"))
                .and_then(|p| p["ratio"]["x"].as_f64())
                .unwrap_or(1.0) as f32;
            let z = node.map_or(0.0, |n| vec3(&n["pos"])[2]);
            LayerDef { ratio, z }
        })
        .collect();

    // Scatterables, matched to the Tile flag named "scatter" + prefab name without spaces.
    for r in level
        .get("scatterableObjects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(path) = asset_path(Some(r)).map(str::to_owned) else {
            continue;
        };
        let Some(proto) = b.prefab(&path) else {
            continue;
        };
        let flag_name = format!(
            "scatter{}",
            b.world.protos[proto as usize].name.replace(' ', "")
        );
        match TILE_SCATTER_FLAGS.iter().position(|f| *f == flag_name) {
            Some(bit) => b.world.scatterables.push((proto, bit as u8)),
            None => eprintln!("warning: scatterable {path} has no Tile flag {flag_name}"),
        }
    }

    // Blocks: LevelBlocks/Blocks/<block>.
    let container = inp.blocks["roots"]
        .as_array()
        .and_then(|r| r.first())
        .and_then(|r| r["children"].as_array())
        .and_then(|c| c.first())
        .ok_or("blocks scene layout unexpected")?;
    for node in container["children"].as_array().into_iter().flatten() {
        if let Some(block) = b.block(node) {
            b.world.blocks.push(block);
        }
    }

    // Waves. orig: Resources/waves/*.asset.
    let waves = assets_of(inp.data, "Wave");
    let wave_index: HashMap<&str, u16> = waves
        .iter()
        .enumerate()
        .map(|(i, (p, _))| (*p, i as u16))
        .collect();
    b.world.waves = waves
        .iter()
        .map(|(p, f)| WaveDef {
            name: Path::new(p)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            trolls: curve(&f["numTrolls"]),
            tough_trolls: curve(&f["numToughTrolls"]),
            squids: curve(&f["numSquids"]),
            bosses: curve(&f["numBosses"]),
            killer_bosses: curve(&f["numKillerBosses"]),
            both_sides: flag(f, "spawnOnBothSides"),
            boss_wave: flag(f, "bossWave"),
        })
        .collect();

    // Island configs in Land order. orig: Holder.levelConfigs.
    let (_, holder) = find_component(inp.main, "Holder").ok_or("main scene has no Holder")?;
    for r in holder
        .get("levelConfigs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let path = asset_path(Some(r)).ok_or("level config ref")?;
        let asset = inp
            .data
            .get(path)
            .ok_or_else(|| format!("missing {path}"))?;
        let name = asset["name"].as_str().unwrap_or(path);
        let fields = asset["fields"].as_object().ok_or("level config fields")?;
        b.world.levels.push(level_config(name, fields, &wave_index));
    }

    lighting_and_days(&mut b.world, inp)?;

    let grass_path = asset_path(holder.get("grassPrefab")).map(str::to_owned);
    if let Some(node) = grass_path.as_deref().and_then(|p| inp.prefabs.get(p)) {
        let g = comp(node, "Grass").ok_or("grass prefab")?;
        let thicket = asset_path(g.get("thicketPrefab")).map(str::to_owned);
        let (_, world_comp) = find_component(inp.main, "World").ok_or("main scene has no World")?;
        let sprites = holder
            .get("grassSprites")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|s| b.sprite(Some(s)).unwrap_or(0))
            .collect();
        b.world.grass = Some(game::data::GrassDef {
            sprites,
            spacing: num(g, "spacing"),
            update_interval: num(g, "updateInterval"),
            z: comp(node, "FixedTransform").map_or(-0.01, |f| num(f, "z")),
            thicket: thicket.and_then(|p| b.prefab(&p)),
            thicket_spacing: num(world_comp, "thicketSpacing"),
        });
    }
    let list = |b: &mut Builder, k: &str| -> Vec<game::data::ProtoId> {
        holder
            .get(k)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| asset_path(Some(r)).map(str::to_owned))
            .filter_map(|p| b.prefab(&p))
            .collect()
    };
    b.world.wall_prefabs = list(&mut b, "wallPrefabs");
    b.bag();
    b.map();
    b.water();
    b.fire_style();
    // orig: UnlockManager.unlockableOnLand, serialized as little-endian int32 hex.
    if let Some((_, u)) = find_component(inp.main, "UnlockManager") {
        let hex = u
            .get("unlockableOnLand")
            .and_then(Value::as_str)
            .unwrap_or("");
        b.world.unlockable_on_land = hex
            .as_bytes()
            .chunks(8)
            .filter_map(|c| {
                let s = std::str::from_utf8(c).ok()?;
                let v = u32::from_str_radix(s, 16).ok()?;
                Some(v.swap_bytes() as u8)
            })
            .collect();
    }
    b.world.tower_prefabs = list(&mut b, "towerPrefabs");
    let (_, kingdom) = find_component(inp.main, "Kingdom").ok_or("main scene has no Kingdom")?;
    let border = asset_path(kingdom.get("borderPrefab")).map(str::to_owned);
    b.world.border = border.and_then(|p| b.prefab(&p));
    b.world.shield_shop = b.prefab("Resources/prefabs/buildings and interactive/ShopShield.prefab");
    b.world.bolt_sprite = inp
        .prefabs
        .get("Resources/prefabs/objects/Bolt.prefab")
        .and_then(|n| {
            // The sprite may sit on the root or a child.
            comp(n, "SpriteRenderer")
                .or_else(|| child_with(n, "SpriteRenderer").and_then(|c| comp(c, "SpriteRenderer")))
        })
        .and_then(|sr| sr.get("m_Sprite").cloned())
        .and_then(|s| b.sprite(Some(&s)));
    b.world.boulder_sprite = inp
        .prefabs
        .get("Resources/prefabs/objects/Boulder.prefab")
        .and_then(|n| comp(n, "SpriteRenderer"))
        .and_then(|sr| sr.get("m_Sprite").cloned())
        .and_then(|s| b.sprite(Some(&s)));
    b.world.rift_proto =
        b.prefab("Resources/prefabs/buildings and interactive/TeleporterRift.prefab");
    // orig: Player.standardSteedPrefab.
    let player = inp
        .prefabs
        .get("Resources/prefabs/characters/Player.prefab")
        .ok_or("no Player prefab")?;
    let standard = comp(player, "Player")
        .and_then(|p| asset_path(p.get("standardSteedPrefab")))
        .map(str::to_owned)
        .ok_or("Player.standardSteedPrefab")?;
    b.world.default_steed = b.steed(&standard).ok_or("standard steed")?;
    let farmland = asset_path(holder.get("farmlandPrefab")).map(str::to_owned);
    b.world.farmland = farmland.and_then(|p| b.farmland(&p));

    // Tool sprites. orig: Resources/prefabs/objects/Tool*.prefab.
    b.world.tool_sprites = ["ToolBow", "ToolHammer", "ToolScythe", "ToolShield", "Bread"]
        .iter()
        .map(|name| {
            let path = format!("Resources/prefabs/objects/{name}.prefab");
            let node = inp.prefabs.get(&path)?;
            let sr = comp(node, "SpriteRenderer")?;
            b.sprite(sr.get("m_Sprite"))
        })
        .collect();

    // Sun and moon. orig: CelestialBody children of Level/Heaven.
    let heaven_z = level_node["children"]
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["name"] == "Heaven"))
        .map_or(0.0, |h| vec3(&h["pos"])[2]);
    let heaven = level_node["children"]
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["name"] == "Heaven"))
        .ok_or("no Heaven")?;
    for body in heaven["children"].as_array().into_iter().flatten() {
        let Some(cb) = comp(body, "CelestialBody") else {
            continue;
        };
        let sprite = comp(body, "SpriteRenderer").and_then(|sr| b.sprite(sr.get("m_Sprite")));
        let z = heaven_z + vec3(&body["pos"])[2];
        // Every SpriteRenderer child is a glow halo, each with its own
        // sprite, scale, depth and tinting colour.
        let glows: Vec<game::data::CelestialGlow> = body["children"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|g| {
                let sr = comp(g, "SpriteRenderer")?;
                let sprite = b.sprite(sr.get("m_Sprite"))?;
                let scale = vec3(&g["scale"])[0];
                let col = &sr["m_Color"];
                Some(game::data::CelestialGlow {
                    sprite,
                    z: z + vec3(&g["pos"])[2],
                    scale: if scale > 0.0 { scale } else { 1.0 },
                    color: ["r", "g", "b", "a"].map(|k| col[k].as_f64().unwrap_or(1.0) as f32),
                })
            })
            .collect();
        let pair = |k: &str| {
            (
                cb[k]["x"].as_f64().unwrap_or(0.0) as f32,
                cb[k]["y"].as_f64().unwrap_or(0.0) as f32,
            )
        };
        b.world.celestials.push(game::data::CelestialDef {
            sprite,
            glows,
            is_sun: body["name"] == "Sun",
            orbit_scale: pair("orbitScale"),
            orbit_center: pair("orbitCenter"),
            rises_at: num(cb, "risesAt"),
            sets_at: num(cb, "setsAt"),
            z,
        });
    }

    // Sounds, and the soundtrack each day queues.
    let found = crate::audio::find(inp);
    for ((_, f), day) in assets_of(inp.data, "Day").iter().zip(&mut b.world.days) {
        day.tracks = f
            .get("tracks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let sound = *found.by_prefab.get(asset_path(t.get("soundTrack"))?)?;
                Some(game::data::TrackDef {
                    anchor_time: t["anchorTime"].as_f64().unwrap_or(0.0) as f32,
                    anchor_seconds: t["trackAnchorInSeconds"].as_f64().unwrap_or(0.0) as f32,
                    sound,
                })
            })
            .collect();
    }
    b.world.audio = found.audio;
    let visuals = b.visuals.into_iter().collect::<Result<_, _>>()?;
    Ok((b.world, found.clip_paths, visuals))
}

/// Data assets of `ty`, sorted by path, as (path, fields).
fn assets_of<'v>(data: &'v Map<String, Value>, ty: &str) -> Vec<(&'v str, &'v Map<String, Value>)> {
    let mut v: Vec<_> = data
        .iter()
        .filter(|(_, a)| a["type"] == ty)
        .filter_map(|(p, a)| Some((p.as_str(), a["fields"].as_object()?)))
        .collect();
    v.sort_by_key(|(p, _)| *p);
    v
}

fn light_preset(path: &str, f: &Map<String, Value>) -> LightPreset {
    let int = |k: &str| f.get(k).and_then(Value::as_i64).unwrap_or(0);
    let mut enabled = 0u32;
    let mut colors = [[0.0f32; 4]; LIGHT_COLORS.len()];
    for (i, name) in LIGHT_COLORS.iter().enumerate() {
        let field = &f[*name];
        if field["enabled"].as_i64() == Some(1) {
            enabled |= 1 << i;
        }
        let c = &field["value"];
        colors[i] = ["r", "g", "b", "a"].map(|k| c[k].as_f64().unwrap_or(0.0) as f32);
    }
    let mut floats = [0.0f32; LIGHT_FLOATS.len()];
    for (j, name) in LIGHT_FLOATS.iter().enumerate() {
        let field = &f[*name];
        if field["enabled"].as_i64() == Some(1) {
            enabled |= 1 << (LIGHT_COLORS.len() + j);
        }
        floats[j] = field["value"].as_f64().unwrap_or(0.0) as f32;
    }
    LightPreset {
        name: Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        category: int("category") as u8,
        weather: int("weather") as u16,
        season: int("season") as u8,
        part_of_day: int("partOfDay") as u8,
        test_only: int("testOnly") != 0,
        enabled,
        colors,
        floats,
    }
}

/// orig: `BlenderState` and `Day` assets, the `Director` day lists and `Blender.initialStack`.
fn lighting_and_days(world: &mut WorldData, inp: &Inputs) -> Result<(), String> {
    let lights = assets_of(inp.data, "BlenderState");
    let light_index: HashMap<&str, u16> = lights
        .iter()
        .enumerate()
        .map(|(i, (p, _))| (*p, i as u16))
        .collect();
    world.lights = lights.iter().map(|(p, f)| light_preset(p, f)).collect();

    let days = assets_of(inp.data, "Day");
    let day_index: HashMap<&str, u16> = days
        .iter()
        .enumerate()
        .map(|(i, (p, _))| (*p, i as u16))
        .collect();
    world.days = days
        .iter()
        .map(|(p, f)| DayDef {
            name: Path::new(p)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            seasons: f.get("seasonOptions").and_then(Value::as_i64).unwrap_or(7) as u8,
            weathers: f.get("weatherOptions").and_then(Value::as_i64).unwrap_or(1) as u16,
            tracks: Vec::new(),
        })
        .collect();

    let (_, dir) = find_component(inp.main, "Director").ok_or("main scene has no Director")?;
    let day = |k: &str| -> Result<u16, String> {
        asset_path(dir.get(k))
            .and_then(|p| day_index.get(p).copied())
            .ok_or_else(|| format!("Director.{k} not found"))
    };
    let day_list = |k: &str| -> Vec<u16> {
        dir.get(k)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| asset_path(Some(r)).and_then(|p| day_index.get(p).copied()))
            .collect()
    };
    let n = &dir["numRegularDays"];
    let (_, blender) = find_component(inp.main, "Blender").ok_or("main scene has no Blender")?;
    world.director = DirectorDef {
        first_day: day("firstDay")?,
        first_autumn_day: day("firstAutumnDay")?,
        first_winter_day: day("firstWinterDay")?,
        regular_days: day_list("regularDays"),
        boss_days: day_list("bossDays"),
        recovery_days: day_list("recoveryDays"),
        num_regular_days: (
            n["min"].as_i64().unwrap_or(3) as i32,
            n["max"].as_i64().unwrap_or(4) as i32,
        ),
        initial_lights: blender
            .get("initialStack")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| asset_path(Some(r)).and_then(|p| light_index.get(p).copied()))
            .collect(),
    };
    Ok(())
}

#[cfg(test)]
mod transform_tests {
    use super::*;
    use scene::authored::Affine2;
    use serde_json::json;

    fn extract(node: &Value) -> (Proto, PrefabVisual, Vec<String>) {
        let empty = json!({});
        let maps = Map::new();
        let names = HashMap::from([
            ("sprite".into(), "panel".into()),
            ("other-sprite".into(), "trim".into()),
        ]);
        let sizes = HashMap::from([("panel".into(), (4, 4, 32.0))]);
        let inp = Inputs {
            main: &empty,
            blocks: &empty,
            loader: &empty,
            prefabs: &maps,
            data: &maps,
            sprite_names: &names,
            sprite_sizes: &sizes,
            assets_dir: Path::new("."),
        };
        let mut builder = Builder {
            inp: &inp,
            world: WorldData::default(),
            visuals: Vec::new(),
            sprite_ids: HashMap::new(),
            prefab_protos: HashMap::new(),
            recolor: HashSet::new(),
        };
        let index = builder.proto(node) as usize;
        (
            builder.world.protos.remove(index),
            builder.visuals.remove(index).unwrap(),
            builder.world.sprites,
        )
    }

    fn node(
        name: &str,
        pos: [f32; 3],
        scale: [f32; 3],
        rot: [f32; 4],
        children: Vec<Value>,
        sprite: bool,
    ) -> Value {
        json!({
            "name": name, "active": true, "pos": pos, "scale": scale, "rot": rot, "layer": 0,
            "components": if sprite { vec![json!({"id": 31, "type": "SpriteRenderer", "fields": {
                "m_Enabled": 1, "m_Sprite": {"asset": "sprite"}, "m_FlipX": 1, "m_FlipY": 1
            }})] } else { vec![] },
            "children": children,
        })
    }

    const IDENTITY_ROTATION: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn plain(name: &str, children: Vec<Value>, sprite: bool) -> Value {
        node(
            name,
            [0.0; 3],
            [1.0; 3],
            IDENTITY_ROTATION,
            children,
            sprite,
        )
    }

    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-5, "{a} != {b}");
    }

    fn composed(visual: &PrefabVisual, part: usize) -> Affine2 {
        visual
            .sampled_transform(visual.parts[part].node, |_, node| node.local)
            .unwrap()
            .0
    }

    #[test]
    fn composed_parts_keep_nonrendered_parent_rotation_scale_and_renderer_flips() {
        let c = core::f32::consts::FRAC_1_SQRT_2;
        let panel = node(
            "Panel",
            [2.0, 0.0, 0.0],
            [-1.0, 1.0, 1.0],
            IDENTITY_ROTATION,
            vec![],
            true,
        );
        let mut pivot = node(
            "Pivot",
            [1.0, 2.0, 0.0],
            [1.0, 3.0, 1.0],
            [
                0.0,
                0.0,
                (core::f32::consts::PI / 8.0).sin(),
                (core::f32::consts::PI / 8.0).cos(),
            ],
            vec![panel],
            false,
        );
        pivot["components"] = json!([{"type": "Animator", "fields": {
            "m_Enabled": 1, "m_Controller": {"asset": "AnimatorController/panel.controller"}
        }}]);
        let root = node(
            "Root",
            [7.0, 8.0, 0.0],
            [-2.0, 1.0, 1.0],
            [0.0, 0.0, c, c],
            vec![pivot],
            false,
        );
        let (legacy, visual, _) = extract(&root);
        assert_eq!(
            visual
                .nodes
                .iter()
                .map(|node| node.path.as_str())
                .collect::<Vec<_>>(),
            ["", "Pivot", "Pivot/Panel"]
        );
        assert_eq!(visual.nodes[2].parent, Some(1));
        assert_eq!(
            visual.nodes[1].animator.as_deref(),
            Some("AnimatorController/panel.controller")
        );
        assert_eq!(visual.parts[0].node, 2);
        assert!(visual.parts[0].flip_x && visual.parts[0].flip_y);
        assert_eq!((legacy.parts[0].x, legacy.parts[0].y), (6.0, 2.0));
        assert!(
            !legacy.parts[0].flip,
            "old pack keeps the folded reflection"
        );
        let transform = composed(&visual, 0);
        close(transform.translation[0], -2.0 - 2.0 * c);
        close(transform.translation[1], 2.0 + 4.0 * c);
        for (actual, expected) in transform
            .basis
            .into_iter()
            .zip([c, -3.0 * c, -2.0 * c, -6.0 * c])
        {
            close(actual, expected);
        }
    }

    #[test]
    fn visual_parts_match_legacy_indices_without_interning_sprites() {
        let mut inactive = plain("Inactive", vec![plain("Sprite", vec![], true)], false);
        inactive["active"] = false.into();
        let mut reflection = plain("Reflection", vec![plain("Sprite", vec![], true)], false);
        reflection["layer"] = 29.into();
        let mut scaffold = plain("Scaffold", vec![plain("Sprite", vec![], true)], true);
        scaffold["components"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type": "ScaffoldModule", "fields": {}}));
        let mut child = plain("Child", vec![], true);
        child["components"][0]["fields"]["m_Sprite"]["asset"] = "other-sprite".into();
        let mut disabled = plain("Disabled", vec![child], true);
        disabled["components"][0]["fields"]["m_Enabled"] = 0.into();
        let root = plain(
            "Root",
            vec![
                inactive,
                reflection,
                scaffold,
                disabled,
                plain("Normal", vec![], true),
            ],
            false,
        );
        let (legacy, visual, sprite_names) = extract(&root);
        assert_eq!(sprite_names, ["trim", "panel"]);
        assert_eq!(
            legacy
                .parts
                .iter()
                .map(|part| part.sprite)
                .collect::<Vec<_>>(),
            [Some(0), Some(1)]
        );
        assert_eq!(
            visual
                .parts
                .iter()
                .map(|part| visual.nodes[part.node as usize].path.as_str())
                .collect::<Vec<_>>(),
            ["Disabled/Child", "Normal"]
        );
        assert!(!visual.nodes[1].local.visible);
    }

    #[test]
    fn portal_surface_goo_and_catapult_rope_keep_authored_transform_magnitudes() {
        let surface = node(
            "Surface",
            [0.0; 3],
            [1.0, 1.5, 1.0],
            IDENTITY_ROTATION,
            vec![],
            true,
        );
        let goo = node(
            "Goo Spin",
            [0.0; 3],
            [-1.3, 1.1, 1.0],
            IDENTITY_ROTATION,
            vec![],
            true,
        );
        let rope = node(
            "Rope",
            [0.0; 3],
            [6.659998, 1.0, 1.0],
            [0.0, 0.0, 0.36361992, 0.9315474],
            vec![],
            true,
        );
        let (_, visual, _) = extract(&plain("Root", vec![surface, goo, rope], false));
        assert_eq!(composed(&visual, 0).basis, [1.0, 0.0, 0.0, 1.5]);
        assert_eq!(composed(&visual, 1).basis, [-1.3, 0.0, 0.0, 1.1]);
        let [a, b, c, d] = composed(&visual, 2).basis;
        close((a * a + c * c).sqrt(), 6.659998);
        close((b * b + d * d).sqrt(), 1.0);
        assert!(c > 4.0 && b < -0.6, "rope rotation was discarded");
    }
}
