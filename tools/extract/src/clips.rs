//! AnimationClips → sprite-swap tracks and child position tracks.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::Serialize;

use crate::yaml::{field, guid_of, inline_map, parse_float};

#[derive(Serialize, Default)]
pub struct Clip {
    /// Seconds. orig: `AnimationClip.m_MuscleClip.m_StopTime`.
    pub length: f32,
    pub looping: bool,
    /// Child path ("" = root) → (time, sprite name) keys.
    pub sprites: BTreeMap<String, Vec<(f32, String)>>,
    /// Child path → (time, x, y, stepped) local-position keys, in units.
    pub offsets: BTreeMap<String, Vec<(f32, f32, f32, bool)>>,
}

/// Splits a top-level YAML list (lines starting with `  - ` at `indent`) into items.
fn list_items(block: &str, indent: usize) -> Vec<&str> {
    let marker = format!("{}- ", " ".repeat(indent));
    let mut items = Vec::new();
    let mut start = None;
    let mut pos = 0;
    for line in block.split_inclusive('\n') {
        if line.starts_with(&marker) {
            if let Some(s) = start {
                items.push(&block[s..pos]);
            }
            start = Some(pos);
        }
        pos += line.len();
    }
    if let Some(s) = start {
        items.push(&block[s..]);
    }
    items
}

/// The text of a top-level `  key:` section, up to the next key at the same indent.
fn section<'a>(text: &'a str, key: &str) -> &'a str {
    let header = format!("\n  {key}:");
    let Some(start) = text.find(&header) else {
        return "";
    };
    let body = &text[start + header.len()..];
    let end = body.find("\n  m_").unwrap_or(body.len());
    &body[..end]
}

fn parse(text: &str, sprites_by_guid: &HashMap<String, String>) -> Clip {
    let mut clip = Clip {
        length: field(text, "m_StopTime")
            .and_then(parse_float)
            .unwrap_or(0.0),
        looping: field(text, "m_LoopTime") == Some("1"),
        ..Default::default()
    };

    for item in list_items(section(text, "m_PPtrCurves"), 2) {
        if field(item, "attribute") != Some("m_Sprite") {
            continue;
        }
        let path = field(item, "path").unwrap_or("").to_owned();
        let mut keys = Vec::new();
        let mut time = None;
        for line in item.lines() {
            let l = line.trim_start().trim_start_matches("- ");
            if let Some(t) = l.strip_prefix("time:") {
                time = parse_float(t.trim());
            } else if let (Some(v), Some(t)) = (l.strip_prefix("value:"), time) {
                if let Some(name) = guid_of(v).and_then(|g| sprites_by_guid.get(g)) {
                    keys.push((t, name.clone()));
                }
                time = None;
            }
        }
        clip.sprites.insert(path, keys);
    }

    for item in list_items(section(text, "m_PositionCurves"), 2) {
        let path = field(item, "path").unwrap_or("").to_owned();
        let mut keys = Vec::new();
        let mut cur: Option<(f32, f32, f32)> = None;
        for line in item.lines() {
            let l = line.trim_start().trim_start_matches("- ");
            if let Some(t) = l.strip_prefix("time:") {
                cur = parse_float(t.trim()).map(|t| (t, 0.0, 0.0));
            } else if let (Some(v), Some(c)) = (l.strip_prefix("value:"), cur.as_mut()) {
                if let Some(xy) = inline_map(v, &["x", "y"]) {
                    (c.1, c.2) = (xy[0], xy[1]);
                }
            } else if let Some(v) = l.strip_prefix("outSlope:") {
                if let Some(c) = cur.take() {
                    let stepped = inline_map(v, &["x", "y"])
                        .is_some_and(|s| s.iter().any(|f| f.is_infinite()));
                    keys.push((c.0, c.1, c.2, stepped));
                }
            }
        }
        if !keys.is_empty() {
            clip.offsets.insert(path, keys);
        }
    }
    clip
}

pub fn extract_all(
    dir: &Path,
    sprites_by_guid: &HashMap<String, String>,
) -> Result<BTreeMap<String, Clip>, String> {
    let mut out = BTreeMap::new();
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "anim"))
        .collect();
    // In name order, the first of clips sharing a name kept (a few UI ones
    // do), so every extraction comes out the same.
    paths.sort();
    for p in paths {
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let name = field(&text, "m_Name").unwrap_or_default().to_owned();
        out.entry(name)
            .or_insert_with(|| parse(&text, sprites_by_guid));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIP: &str = "AnimationClip:
  m_Name: test_walk
  m_PositionCurves:
  - curve:
      m_Curve:
      - serializedVersion: 2
        time: 0
        value: {x: 0, y: 0, z: -0.001}
        inSlope: {x: 0, y: 0, z: 0}
        outSlope: {x: 0, y: Infinity, z: 0}
        tangentMode: 1
      - serializedVersion: 2
        time: 0.1
        value: {x: 0, y: 0.03125, z: -0.001}
        inSlope: {x: 0, y: 0, z: 0}
        outSlope: {x: 0, y: 0, z: 0}
        tangentMode: 1
    path: King
  m_ScaleCurves: []
  m_PPtrCurves:
  - curve:
    - time: 0
      value: {fileID: 21300000, guid: aaa, type: 2}
    - time: 0.1
      value: {fileID: 21300000, guid: bbb, type: 2}
    attribute: m_Sprite
    path:
    classID: 212
  m_SampleRate: 10
    m_StopTime: 0.2
    m_LoopTime: 1
";

    #[test]
    fn parses_tracks() {
        let names = HashMap::from([
            ("aaa".to_owned(), "a".to_owned()),
            ("bbb".to_owned(), "b".to_owned()),
        ]);
        let c = parse(CLIP, &names);
        assert_eq!(c.length, 0.2);
        assert!(c.looping);
        assert_eq!(
            c.sprites[""],
            vec![(0.0, "a".to_owned()), (0.1, "b".to_owned())]
        );
        assert_eq!(
            c.offsets["King"],
            vec![(0.0, 0.0, 0.0, true), (0.1, 0.0, 0.03125, false)]
        );
    }
}
