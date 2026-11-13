//! Unity clips/controllers → typed curves, sprite keys and whitelisted markers.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::yaml::{field, parse_float};
use game::animation::{
    self, AnimatorState, AnimatorTransition, Condition, Controller, Event, EventKind, Key, Library,
    SpriteTrack, Tangent, TransformTrack,
};
use serde_yaml::Value as Y;

#[derive(Debug, Default)]
struct ExtractedClip {
    source_file: String,
    original_name: String,
    animation: animation::Clip,
    unsupported: Vec<String>,
}

#[derive(Default)]
struct Report {
    sprite_keys: usize,
    null_sprite_keys: usize,
    scalar_keys: usize,
    events: usize,
    duplicate_name_aliases: usize,
    unsupported: Vec<String>,
}

fn number(v: &Y) -> Option<f32> {
    v.as_f64()
        .map(|n| n as f32)
        .or_else(|| v.as_str().and_then(parse_float))
}
fn f(v: &Y) -> f32 {
    number(v).unwrap_or(0.0)
}
fn s(v: &Y) -> String {
    v.as_str().unwrap_or_default().to_owned()
}
fn seq(v: &Y) -> &[Y] {
    v.as_sequence().map(Vec::as_slice).unwrap_or_default()
}
fn boolean(v: &Y) -> bool {
    v.as_bool().unwrap_or_else(|| v.as_i64() == Some(1))
}
fn tangent(v: &Y) -> Tangent {
    match number(v) {
        Some(x) if x.is_infinite() => Tangent::Step,
        Some(x) => Tangent::Slope(x),
        None => Tangent::Slope(0.0),
    }
}
fn clean_doc(text: &str) -> String {
    text.lines()
        .filter(|l| !l.starts_with('%') && !l.starts_with("---"))
        .collect::<Vec<_>>()
        .join("\n")
}
fn curve_keys(curve: &Y, axis: Option<&str>) -> Vec<Key> {
    seq(&curve["m_Curve"])
        .iter()
        .map(|k| {
            let component = |name: &str| axis.map_or(&k[name], |axis| &k[name][axis]);
            Key {
                time: f(&k["time"]),
                value: f(component("value")),
                in_tangent: tangent(component("inSlope")),
                out_tangent: tangent(component("outSlope")),
            }
        })
        .collect()
}

fn curve_diagnostics(curve: &Y, path: &str, unsupported: &mut Vec<String>) {
    for property in ["m_PreInfinity", "m_PostInfinity"] {
        if curve[property].as_i64().is_some_and(|v| v != 2) {
            unsupported.push(format!("curve wrap {property} at {path}"));
        }
    }
    if seq(&curve["m_Curve"])
        .iter()
        .any(|k| k["weightedMode"].as_i64().is_some_and(|v| v != 0))
    {
        unsupported.push(format!("weighted curve tangents at {path}"));
    }
}

fn parse(text: &str, sprites_by_guid: &HashMap<String, String>) -> Result<ExtractedClip, String> {
    let doc: Y = serde_yaml::from_str(&clean_doc(text)).map_err(|e| e.to_string())?;
    let data = &doc["AnimationClip"];
    if !data.is_mapping() {
        return Err("expected an AnimationClip document".into());
    }
    let length = field(text, "m_StopTime")
        .and_then(parse_float)
        .unwrap_or(0.0);
    let looping = field(text, "m_LoopTime") == Some("1");
    let mut clip = ExtractedClip {
        original_name: s(&data["m_Name"]),
        animation: animation::Clip {
            length,
            looping,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut tracks: BTreeMap<String, TransformTrack> = BTreeMap::new();
    let mut sprite_tracks = BTreeMap::new();
    for item in seq(&data["m_PPtrCurves"]) {
        let path = s(&item["path"]);
        if item["attribute"].as_str() != Some("m_Sprite") {
            clip.unsupported
                .push(format!("object curve {} at {path}", s(&item["attribute"])));
            continue;
        }
        let mut keys = Vec::new();
        for key in seq(&item["curve"]) {
            let reference = &key["value"];
            let sprite = if reference["fileID"].as_i64() == Some(0) {
                None
            } else {
                let guid = reference["guid"]
                    .as_str()
                    .ok_or("sprite key lacks asset GUID")?;
                Some(
                    sprites_by_guid
                        .get(guid)
                        .ok_or_else(|| format!("unresolved sprite GUID {guid} at {path}"))?
                        .clone(),
                )
            };
            keys.push((f(&key["time"]), sprite));
        }
        sprite_tracks.insert(path, keys);
    }
    for section in [
        "m_PositionCurves",
        "m_ScaleCurves",
        "m_RotationCurves",
        "m_EulerCurves",
    ] {
        for item in seq(&data[section]) {
            let path = s(&item["path"]);
            let track = tracks
                .entry(path.clone())
                .or_insert_with(|| TransformTrack {
                    path,
                    ..Default::default()
                });
            let curve = &item["curve"];
            curve_diagnostics(curve, &track.path, &mut clip.unsupported);
            for (i, axis) in ["x", "y", "z", "w"].iter().enumerate() {
                if i == 3 && section != "m_RotationCurves" {
                    continue;
                }
                let keys = curve_keys(curve, Some(axis));
                match section {
                    "m_PositionCurves" => track.position[i] = keys,
                    "m_ScaleCurves" => track.scale[i] = keys,
                    "m_RotationCurves" => track.rotation[i] = keys,
                    "m_EulerCurves" if i == 2 => track.euler_z = keys,
                    "m_EulerCurves"
                        if keys.iter().any(|k| {
                            k.value != 0.0
                                || k.in_tangent != Tangent::Slope(0.0)
                                || k.out_tangent != Tangent::Slope(0.0)
                        }) =>
                    {
                        clip.unsupported
                            .push(format!("nonzero Euler {axis} at {}", track.path));
                    }
                    _ => {}
                }
            }
        }
    }
    for item in seq(&data["m_FloatCurves"]) {
        let path = s(&item["path"]);
        let attr = s(&item["attribute"]);
        let class = item["classID"].as_i64().unwrap_or_default();
        let channel = match (class, attr.as_str()) {
            (212, "m_Color.r") => Some(0),
            (212, "m_Color.g") => Some(1),
            (212, "m_Color.b") => Some(2),
            (212, "m_Color.a") => Some(3),
            _ => None,
        };
        if channel.is_none() && !(class == 212 && attr == "m_Enabled") {
            clip.unsupported
                .push(format!("float curve {attr} class {class} at {path}"));
            continue;
        }
        let track = tracks
            .entry(path.clone())
            .or_insert_with(|| TransformTrack {
                path,
                ..Default::default()
            });
        curve_diagnostics(&item["curve"], &track.path, &mut clip.unsupported);
        let keys = curve_keys(&item["curve"], None);
        if let Some(i) = channel {
            track.color[i] = keys;
        } else {
            track.visible = keys;
        }
    }
    if !seq(&data["m_CompressedRotationCurves"]).is_empty() {
        clip.unsupported.push("compressed rotation curves".into());
    }
    for item in seq(&data["m_Events"]) {
        let name = s(&item["functionName"]);
        let Some(kind) = EventKind::from_function(&name) else {
            clip.unsupported.push(format!("event {name}"));
            continue;
        };
        if item["objectReferenceParameter"]["fileID"]
            .as_i64()
            .unwrap_or_default()
            != 0
        {
            clip.unsupported
                .push(format!("object-reference event parameter {name}"));
            continue;
        }
        clip.animation.events.push(Event {
            time: f(&item["time"]),
            kind,
            float_parameter: f(&item["floatParameter"]),
            int_parameter: item["intParameter"].as_i64().unwrap_or_default() as i32,
            string_parameter: s(&item["data"]),
        });
    }
    clip.animation
        .events
        .sort_by(|a, b| a.time.total_cmp(&b.time));
    clip.animation.tracks = tracks.into_values().collect();
    clip.animation.sprites = sprite_tracks
        .into_iter()
        .map(|(path, keys)| SpriteTrack { path, keys })
        .collect();
    if !clip.animation.validate() {
        return Err("invalid/nonfinite animation keys or markers".into());
    }
    Ok(clip)
}

fn paths_with_extension(dir: &Path, extension: &str) -> Result<Vec<PathBuf>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.extension().is_some_and(|suffix| suffix == extension) {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn extract_all(
    dir: &Path,
    sprites_by_guid: &HashMap<String, String>,
) -> Result<BTreeMap<String, ExtractedClip>, String> {
    let mut clips = Vec::new();
    for path in paths_with_extension(dir, "anim")? {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        // Keep canonical embedded names aligned with the legacy clips table.
        let original = field(&text, "m_Name").unwrap_or_default().to_owned();
        let mut clip =
            parse(&text, sprites_by_guid).map_err(|e| format!("{}: {e}", path.display()))?;
        clip.source_file = path.file_name().unwrap().to_string_lossy().into_owned();
        clip.original_name = original;
        clips.push(clip);
    }
    let canonical: BTreeSet<_> = clips
        .iter()
        .map(|clip| clip.original_name.clone())
        .collect();
    let mut out = BTreeMap::new();
    for clip in clips {
        let name = if out.contains_key(&clip.original_name) || clip.original_name.is_empty() {
            let stem = Path::new(&clip.source_file)
                .file_stem()
                .unwrap()
                .to_string_lossy();
            let mut alias = stem.to_string();
            let mut suffix = 2;
            while canonical.contains(&alias) || out.contains_key(&alias) {
                alias = format!("{stem}#{suffix}");
                suffix += 1;
            }
            alias
        } else {
            clip.original_name.clone()
        };
        out.insert(name, clip);
    }
    Ok(out)
}

fn docs(text: &str) -> Result<Vec<(i64, Y)>, String> {
    let mut out = Vec::new();
    for part in text.split("--- !u!").skip(1) {
        let (head, body) = part.split_once('\n').ok_or("invalid Unity document")?;
        let id = head
            .split('&')
            .nth(1)
            .and_then(|s| s.trim().parse().ok())
            .ok_or("invalid Unity object ID")?;
        let doc: Y = serde_yaml::from_str(body).map_err(|e| e.to_string())?;
        out.push((id, doc));
    }
    Ok(out)
}

fn library(
    assets: &Path,
    guids: &HashMap<String, PathBuf>,
    clips: &BTreeMap<String, ExtractedClip>,
) -> Result<(Library, Report), String> {
    let mut library = Library {
        clips: clips
            .iter()
            .map(|(n, c)| (n.clone(), c.animation.clone()))
            .collect(),
        ..Default::default()
    };
    let mut report = Report::default();
    for (name, clip) in clips {
        report.duplicate_name_aliases += usize::from(name != &clip.original_name);
        report.sprite_keys += clip
            .animation
            .sprites
            .iter()
            .map(|track| track.keys.len())
            .sum::<usize>();
        report.null_sprite_keys += clip
            .animation
            .sprites
            .iter()
            .flat_map(|track| &track.keys)
            .filter(|key| key.1.is_none())
            .count();
        report.scalar_keys += clip
            .animation
            .tracks
            .iter()
            .map(|t| {
                t.position
                    .iter()
                    .chain(&t.rotation)
                    .chain(&t.scale)
                    .chain(&t.color)
                    .chain(core::iter::once(&t.euler_z))
                    .chain(core::iter::once(&t.visible))
                    .map(Vec::len)
                    .sum::<usize>()
            })
            .sum::<usize>();
        report.events += clip.animation.events.len();
        report
            .unsupported
            .extend(clip.unsupported.iter().map(|s| format!("{name}: {s}")));
    }
    let source_names: HashMap<_, _> = clips
        .iter()
        .map(|(name, clip)| (clip.source_file.as_str(), name.as_str()))
        .collect();
    let clip_name = |v: &Y| -> Option<String> {
        let path = guids.get(v["guid"].as_str()?)?;
        let file = path.file_name()?.to_str()?;
        Some(source_names.get(file)?.to_string())
    };
    for p in paths_with_extension(&assets.join("AnimatorController"), "controller")? {
        let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
        let objects = docs(&text)?;
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let states: HashMap<i64, String> = objects
            .iter()
            .filter(|(_, v)| !v["AnimatorState"].is_null())
            .map(|(id, v)| (*id, s(&v["AnimatorState"]["m_Name"])))
            .collect();
        let default = objects
            .iter()
            .find_map(|(_, v)| v["AnimatorStateMachine"]["m_DefaultState"]["fileID"].as_i64())
            .and_then(|id| states.get(&id).cloned());
        library.controllers.push(Controller {
            name: name.clone(),
            default_state: default,
        });
        let mut transition_sources: HashMap<i64, String> = HashMap::new();
        for (_, v) in &objects {
            let state = &v["AnimatorState"];
            if state.is_null() {
                continue;
            }
            let state_name = s(&state["m_Name"]);
            for t in seq(&state["m_Transitions"]) {
                if let Some(id) = t["fileID"].as_i64() {
                    transition_sources.insert(id, state_name.clone());
                }
            }
            let clip = clip_name(&state["m_Motion"]);
            if state["m_Motion"]["fileID"]
                .as_i64()
                .is_some_and(|id| id != 0)
                && clip.is_none()
            {
                report
                    .unsupported
                    .push(format!("{name}/{state_name}: unresolved/BlendTree motion"));
            }
            library.states.push(AnimatorState {
                controller: name.clone(),
                name: state_name,
                clip,
                speed: number(&state["m_Speed"]).unwrap_or(1.0),
                speed_parameter: boolean(&state["m_SpeedParameterActive"])
                    .then(|| s(&state["m_SpeedParameter"])),
                cycle_offset: f(&state["m_CycleOffset"]),
            });
        }
        for (id, v) in &objects {
            let t = &v["AnimatorStateTransition"];
            if t.is_null() {
                continue;
            }
            library.transitions.push(AnimatorTransition {
                controller: name.clone(),
                from: transition_sources
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| "*".into()),
                to: t["m_DstState"]["fileID"]
                    .as_i64()
                    .and_then(|id| states.get(&id).cloned()),
                conditions: seq(&t["m_Conditions"])
                    .iter()
                    .map(|c| Condition {
                        parameter: s(&c["m_ConditionEvent"]),
                        mode: c["m_ConditionMode"].as_i64().unwrap_or_default() as i32,
                        threshold: f(&c["m_EventTreshold"]),
                    })
                    .collect(),
                exit_time: boolean(&t["m_HasExitTime"]).then(|| f(&t["m_ExitTime"])),
                duration: f(&t["m_TransitionDuration"]),
                fixed_duration: boolean(&t["m_HasFixedDuration"]),
                offset: f(&t["m_TransitionOffset"]),
            });
        }
    }
    for p in paths_with_extension(
        &assets.join("AnimatorOverrideController"),
        "overrideController",
    )? {
        let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
        let parsed = docs(&text)?;
        let data =
            &parsed.first().ok_or("empty override controller")?.1["AnimatorOverrideController"];
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let base = guids
            .get(data["m_Controller"]["guid"].as_str().unwrap_or_default())
            .and_then(|p| p.file_stem())
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or_else(|| format!("{name}: unresolved base controller"))?;
        let replacements: HashMap<_, _> = seq(&data["m_Clips"])
            .iter()
            .filter_map(|v| {
                Some((
                    clip_name(&v["m_OriginalClip"])?,
                    clip_name(&v["m_OverrideClip"])?,
                ))
            })
            .collect();
        let Some(controller) = library.controllers.iter().find(|c| c.name == base).cloned() else {
            return Err(format!("{name}: missing base controller {base}"));
        };
        library.controllers.push(Controller {
            name: name.clone(),
            ..controller
        });
        let states: Vec<_> = library
            .states
            .iter()
            .filter(|s| s.controller == base)
            .map(|s| {
                let mut s = s.clone();
                s.controller = name.clone();
                if let Some(replacement) = s.clip.as_ref().and_then(|c| replacements.get(c)) {
                    s.clip = Some(replacement.clone());
                }
                s
            })
            .collect();
        library.states.extend(states);
        let transitions: Vec<_> = library
            .transitions
            .iter()
            .filter(|t| t.controller == base)
            .map(|t| {
                let mut t = t.clone();
                t.controller = name.clone();
                t
            })
            .collect();
        library.transitions.extend(transitions);
    }
    Ok((library, report))
}

/// Extract the optional authored library while legacy clip conversion keeps its format.
pub fn extract(
    assets: &Path,
    guids: &HashMap<String, PathBuf>,
    sprite_names: &HashMap<String, String>,
) -> Result<Library, String> {
    let clips = extract_all(&assets.join("AnimationClip"), sprite_names)?;
    let (library, report) = library(assets, guids, &clips)?;
    if !library.validate() {
        return Err("invalid authored clip or controller data".into());
    }
    println!(
        "authored animation: {} clips, {} sprite keys ({} null), {} scalar keys, {} markers, {} states, {} transitions, {} clip aliases",
        library.clips.len(),
        report.sprite_keys,
        report.null_sprite_keys,
        report.scalar_keys,
        report.events,
        library.states.len(),
        library.transitions.len(),
        report.duplicate_name_aliases,
    );
    if !report.unsupported.is_empty() {
        println!(
            "authored animation: {} unsupported tracks, markers or motions",
            report.unsupported.len()
        );
    }
    Ok(library)
}

#[cfg(test)]
mod tests {
    use super::*;
    use game::animation::Pose;

    const CLIP: &str = "AnimationClip:
  m_Name: test
  m_PositionCurves:
  - curve:
      m_Curve:
      - time: 0
        value: {x: 0, y: 3, z: 0}
        inSlope: {x: 0, y: 0, z: 0}
        outSlope: {x: 8, y: Infinity, z: 0}
      - time: 2
        value: {x: 4, y: 7, z: 0}
        inSlope: {x: 0, y: Infinity, z: 0}
        outSlope: {x: 0, y: 0, z: 0}
    path: Arm
  m_ScaleCurves:
  - curve:
      m_Curve:
      - time: 0
        value: {x: -1.3, y: 1.1, z: 1}
        inSlope: {x: 0, y: 0, z: 0}
        outSlope: {x: 0, y: 0, z: 0}
    path: Arm
  m_RotationCurves:
  - curve:
      m_Curve:
      - time: 0
        value: {x: 0, y: 0, z: 0.36361992, w: 0.9315474}
        inSlope: {x: 0, y: 0, z: 0, w: 0}
        outSlope: {x: 0, y: 0, z: 0, w: 0}
    path: Arm
  m_FloatCurves:
  - curve:
      m_Curve:
      - time: 0
        value: 0.353
        inSlope: 0
        outSlope: 9.705
      - time: 0.06666667
        value: 1
        inSlope: 4.852496
        outSlope: 4.8525
    attribute: m_Color.a
    classID: 212
    path: Arm
  - curve:
      m_Curve: []
    attribute: m_AnchoredPosition.x
    classID: 224
    path: Menu
  m_PPtrCurves:
  - curve:
    - time: 0
      value: {fileID: 21300000, guid: aaa, type: 2}
    - time: 0.75
      value: {fileID: 0}
    - time: 1
      value: {fileID: 21300000, guid: aaa, type: 2}
    attribute: m_Sprite
    path: Arm
  m_MuscleClip:
    m_StopTime: 2
    m_LoopTime: 1
  m_Events:
  - time: 0.15
    functionName: ReleaseProjectileFromAnimation
    data: safe-data
    objectReferenceParameter: {fileID: 0}
    floatParameter: 0.5
    intParameter: 2
  - time: 0.16666667
    functionName: ArmImpactFromAnimation
    objectReferenceParameter: {fileID: 0}
  - time: 1.1666666
    functionName: FiringDoneFromAnimation
    objectReferenceParameter: {fileID: 0}
  - time: 0.4
    functionName: ArbitraryMethod
";
    fn names() -> HashMap<String, String> {
        HashMap::from([("aaa".into(), "sprite".into())])
    }
    #[test]
    fn typed_curves_markers_and_null_frames_keep_authored_values() {
        let c = parse(CLIP, &names()).unwrap();
        let p = c.animation.pose("Arm", 1.0, Pose::default());
        assert_eq!(p.position, [4.0, 3.0, 0.0]);
        assert_eq!(p.scale, [-1.3, 1.1, 1.0]);
        assert!((p.rotation[2] - 0.36361992).abs() < 1e-6);
        let p = c.animation.pose("Arm", 1.0 / 30.0, Pose::default());
        assert!((p.color[3] - 0.7169375).abs() < 1e-5);
        assert_eq!(c.animation.events.len(), 3);
        assert_eq!(c.animation.events[0].kind, EventKind::ProjectileRelease);
        assert_eq!(c.animation.events[0].string_parameter, "safe-data");
        assert_eq!(c.animation.events[0].int_parameter, 2);
        assert_eq!(c.unsupported.len(), 2);
        assert_eq!(c.animation.sprite_frame("Arm", 0.8), Some(None));
        assert_eq!(c.animation.sprite_frame("Arm", 1.1), Some(Some("sprite")));
        assert_eq!(c.animation.sprite_frame("absent", 1.1), None);
        assert!(parse(CLIP, &HashMap::new())
            .unwrap_err()
            .contains("unresolved sprite"));
    }

    #[test]
    fn euler_and_renderer_enabled_tracks_are_applied() {
        let text = CLIP.replace("  m_ScaleCurves:", "  m_EulerCurves:\n  - curve:\n      m_Curve:\n      - time: 0\n        value: {x: 0, y: 0, z: -180}\n        inSlope: {x: 0, y: 0, z: 0}\n        outSlope: {x: 0, y: 0, z: 0}\n    path: Arm\n  m_ScaleCurves:")
            .replace("  m_FloatCurves:", "  m_FloatCurves:\n  - curve:\n      m_Curve:\n      - time: 0\n        value: 0\n        inSlope: Infinity\n        outSlope: Infinity\n    attribute: m_Enabled\n    classID: 212\n    path: Arm");
        let clip = parse(&text, &names()).unwrap();
        let pose = clip.animation.pose("Arm", 0.0, Pose::default());
        assert!(!pose.visible);
        assert!((pose.rotation[2] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn duplicate_embedded_names_keep_both_clip_assets() {
        let dir = std::env::temp_dir().join(format!(
            "kingdom-animation-duplicates-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("test.anim"), CLIP).unwrap();
        std::fs::write(
            dir.join("test_1.anim"),
            CLIP.replace("time: 0.15", "time: 0.25"),
        )
        .unwrap();
        let clips = extract_all(&dir, &names()).unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(
            clips["test"]
                .animation
                .first_event_time(EventKind::ProjectileRelease),
            Some(0.15)
        );
        assert_eq!(
            clips["test_1"]
                .animation
                .first_event_time(EventKind::ProjectileRelease),
            Some(0.25)
        );
        assert_eq!(clips["test_1"].source_file, "test_1.anim");
        assert_eq!(clips["test_1"].original_name, "test");

        // An alias must not steal a real embedded name from another file.
        std::fs::write(
            dir.join("test_2.anim"),
            CLIP.replace("m_Name: test", "m_Name: test_1"),
        )
        .unwrap();
        let clips = extract_all(&dir, &names()).unwrap();
        assert_eq!(clips["test_1#2"].source_file, "test_1.anim");
        assert_eq!(clips["test_1"].source_file, "test_2.anim");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "requires KINGDOM_REFERENCE_ASSETS pointing at a local AssetRipper Assets directory"]
    fn local_reference_animation_inventory() {
        let assets = PathBuf::from(
            std::env::var_os("KINGDOM_REFERENCE_ASSETS").expect("KINGDOM_REFERENCE_ASSETS"),
        );
        let guids = crate::guid_map(&assets);
        let sheet = crate::sprites::extract_all(&assets.join("Sprite"), &guids).unwrap();
        let library = extract(&assets, &guids, &sheet.names_by_guid).unwrap();
        assert_eq!(library.clips.len(), 244);
        assert_eq!(library.state_speed("player_bear", "Run"), Some(1.0));
        for (clip, marker, time) in [
            ("catapult_fire", EventKind::ProjectileRelease, 0.15),
            ("worker_hammer", EventKind::HammerHit, 0.41666666),
            ("squid_attack", EventKind::GrabComplete, 0.5),
        ] {
            assert_eq!(
                library.clip(clip).unwrap().first_event_time(marker),
                Some(time)
            );
        }
        let throw = library.state_clip("boss", "Throw").unwrap();
        let release = throw.first_event_time(EventKind::BoulderRelease).unwrap();
        assert_eq!(release, 0.8333333);
        assert_eq!(
            throw.pose("Hand", release, Pose::default()).position,
            [0.963, 0.876, 0.0]
        );
    }

    #[test]
    fn controller_speed_transition_and_override_resolve_to_authored_clip() {
        let dir = std::env::temp_dir().join(format!(
            "kingdom-animation-controller-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("AnimatorController")).unwrap();
        std::fs::create_dir_all(dir.join("AnimatorOverrideController")).unwrap();
        let base = dir.join("AnimatorController/base.controller");
        std::fs::write(&base,"--- !u!91 &91\nAnimatorController:\n  m_Name: base\n--- !u!1107 &7\nAnimatorStateMachine:\n  m_DefaultState: {fileID: 2}\n--- !u!1102 &2\nAnimatorState:\n  m_Name: Run\n  m_Speed: 1.5\n  m_CycleOffset: 0\n  m_SpeedParameterActive: 0\n  m_Motion: {fileID: 7400000, guid: old}\n  m_Transitions:\n  - {fileID: 3}\n--- !u!1101 &3\nAnimatorStateTransition:\n  m_DstState: {fileID: 2}\n  m_TransitionDuration: 0.1\n  m_HasFixedDuration: 1\n  m_HasExitTime: 1\n  m_ExitTime: 0.8\n  m_Conditions:\n  - m_ConditionMode: 3\n    m_ConditionEvent: Speed\n    m_EventTreshold: 2\n").unwrap();
        std::fs::write(dir.join("old.anim"), "AnimationClip:\n  m_Name: old\n").unwrap();
        std::fs::write(dir.join("new.anim"), "AnimationClip:\n  m_Name: new\n").unwrap();
        std::fs::write(dir.join("AnimatorOverrideController/skin.overrideController"),"--- !u!221 &221\nAnimatorOverrideController:\n  m_Controller: {fileID: 91, guid: base}\n  m_Clips:\n  - m_OriginalClip: {fileID: 7400000, guid: old}\n    m_OverrideClip: {fileID: 7400000, guid: new}\n").unwrap();
        let guids = HashMap::from([
            ("base".into(), base),
            ("old".into(), dir.join("old.anim")),
            ("new".into(), dir.join("new.anim")),
        ]);
        let clips = extract_all(&dir, &HashMap::new()).unwrap();
        let (library, report) = library(&dir, &guids, &clips).unwrap();
        assert!(library.validate());
        assert!(report.unsupported.is_empty());
        assert_eq!(library.state_speed("skin", "Run"), Some(1.5));
        assert_eq!(
            library
                .default_state("AnimatorOverrideController/skin.overrideController")
                .unwrap()
                .clip
                .as_deref(),
            Some("new")
        );
        let t = library
            .transitions
            .iter()
            .find(|t| t.controller == "skin")
            .unwrap();
        assert_eq!(t.exit_time, Some(0.8));
        assert_eq!(t.duration, 0.1);
        assert_eq!(t.conditions[0].threshold, 2.0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
