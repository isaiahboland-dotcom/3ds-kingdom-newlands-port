//! Sounds: `AudioEmitter` prefabs (and plain `AudioSource`s) → `SoundDef`s,
//! the script fields that point at them, and their clips converted to
//! DSP-ADPCM `.adp` files (`scene::adpcm`, via `scene::clipfile`) at
//! 22050 Hz: mono for effects, stereo for the soundtrack.

use std::collections::HashMap;
use std::path::Path;

use game::data::{AudioClipDef, AudioDef, Rolloff, SoundDef, SoundGroup, SoundId};
use scene::adpcm::Adp;
use scene::clipfile::{self, RATE};
use serde_json::{Map, Value};

use crate::world::Inputs;


fn comp<'v>(node: &'v Value, ty: &str) -> Option<&'v Map<String, Value>> {
    node["components"]
        .as_array()?
        .iter()
        .find(|c| c["type"] == ty)?
        .get("fields")?
        .as_object()
}

fn f(m: &Map<String, Value>, k: &str, default: f64) -> f32 {
    m.get(k).and_then(Value::as_f64).unwrap_or(default) as f32
}

fn stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Sounds found while walking prefabs and the main scene.
pub struct Found {
    pub audio: AudioDef,
    /// Clip asset paths, by `AudioClipId`.
    pub clip_paths: Vec<String>,
    /// Emitter prefab path → sound.
    pub by_prefab: HashMap<String, SoundId>,
}

struct Builder {
    found: Found,
    clip_ids: HashMap<String, u16>,
    refs: HashMap<String, SoundId>,
}

impl Builder {
    fn clip(&mut self, v: &Value) -> Option<u16> {
        let path = v.get("asset")?.as_str()?;
        if let Some(&id) = self.clip_ids.get(path) {
            return Some(id);
        }
        let id = self.found.clip_paths.len() as u16;
        self.found.clip_paths.push(path.to_owned());
        self.found.audio.clips.push(AudioClipDef {
            name: stem(path),
            ..Default::default()
        });
        self.clip_ids.insert(path.to_owned(), id);
        Some(id)
    }

    /// A sound from an `AudioSource` (and the `AudioEmitter` beside it).
    fn sound(&mut self, name: String, node: &Value) -> Option<SoundId> {
        let src = comp(node, "AudioSource")?;
        let em = comp(node, "AudioEmitter");
        let clips: Vec<u16> = match em {
            Some(e) => e
                .get("clips")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|c| self.clip(c))
                .collect(),
            None => src.get("m_audioClip").and_then(|c| self.clip(c)).into_iter().collect(),
        };
        if clips.is_empty() {
            return None;
        }
        let group = match src.get("OutputAudioMixerGroup") {
            Some(g) => {
                let mixer = g.get("asset").and_then(Value::as_str).unwrap_or("");
                let id = g.get("id").and_then(Value::as_i64).unwrap_or(0);
                if mixer.contains("Soundtrack") {
                    SoundGroup::Music
                } else if mixer.contains("Ambient") {
                    SoundGroup::Ambient
                } else {
                    match id {
                        24300008 => SoundGroup::Ui,
                        24300016 => SoundGroup::Music,
                        24300020 => SoundGroup::Ambient,
                        _ => SoundGroup::World,
                    }
                }
            }
            None => SoundGroup::World,
        };
        let keys = |curve: &str| -> Vec<(f32, f32)> {
            src.get(curve)
                .and_then(|c| c["m_Curve"].as_array())
                .into_iter()
                .flatten()
                .map(|k| (f(k.as_object().unwrap(), "time", 0.0), f(k.as_object().unwrap(), "value", 0.0)))
                .collect()
        };
        // orig: spatialBlend is the first key of `panLevelCustomCurve`.
        let spatial = keys("panLevelCustomCurve").first().map_or(0.0, |k| k.1);
        let rolloff = match src.get("rolloffMode").and_then(Value::as_i64) {
            Some(0) => Rolloff::Logarithmic,
            Some(2) => Rolloff::Custom(keys("rolloffCustomCurve")),
            _ => Rolloff::Linear,
        };
        let int = |m: Option<&Map<String, Value>>, k: &str, d: i64| {
            m.and_then(|m| m.get(k)).and_then(Value::as_i64).unwrap_or(d)
        };
        let id = self.found.audio.sounds.len() as SoundId;
        self.found.audio.sounds.push(SoundDef {
            name,
            clips,
            random: int(em, "random", 0) != 0,
            max_simultaneous: int(em, "maxSimultaneous", 8).clamp(1, 32) as u8,
            min_retrigger: em.map_or(0.0, |e| f(e, "minRetriggerTime", 0.0)),
            in_range_only: int(em, "playInRangeOnly", 0) != 0,
            volume: f(src, "m_Volume", 1.0),
            looping: int(Some(src), "Loop", 0) != 0,
            spatial,
            min_distance: f(src, "MinDistance", 1.0),
            max_distance: f(src, "MaxDistance", 500.0),
            rolloff,
            group,
            schedule: comp(node, "TimeBasedAudio").map(|t| {
                (f(t, "startTime", 0.0), f(t, "endTime", 0.0), int(Some(t), "season", 7) as u8)
            }),
        });
        Some(id)
    }

    fn add_ref(&mut self, key: String, id: SoundId) {
        self.refs.entry(key).or_insert(id);
    }

    /// Script fields under `node` that point at emitter prefabs, and plain
    /// sources (`<owner>:AudioSource/<node name>`).
    fn walk(&mut self, owner: &str, node: &Value, emitters: &HashMap<String, SoundId>) {
        for c in node["components"].as_array().into_iter().flatten() {
            let ty = c["type"].as_str().unwrap_or("");
            if ty == "AudioSource" && comp(node, "AudioEmitter").is_none() {
                let fields = &c["fields"];
                let has_clip = fields["m_audioClip"].get("asset").is_some();
                let enabled = fields["m_Enabled"].as_i64() != Some(0);
                if has_clip && enabled {
                    let name = node["name"].as_str().unwrap_or("");
                    if let Some(id) = self.sound(format!("{owner}/{name}"), node) {
                        self.add_ref(format!("{owner}:AudioSource/{name}"), id);
                    }
                }
                continue;
            }
            let Some(fields) = c["fields"].as_object() else {
                continue;
            };
            for (field, v) in fields {
                let mut add = |key: String, v: &Value| {
                    if let Some(&id) = v.get("asset").and_then(Value::as_str).and_then(|p| emitters.get(p)) {
                        self.add_ref(format!("{owner}:{key}"), id);
                        self.add_ref(key, id);
                    }
                };
                match v {
                    Value::Array(items) => {
                        for (i, item) in items.iter().enumerate() {
                            add(format!("{ty}.{field}#{i}"), item);
                        }
                    }
                    _ => add(format!("{ty}.{field}"), v),
                }
            }
        }
        for child in node["children"].as_array().into_iter().flatten() {
            self.walk(owner, child, emitters);
        }
    }
}

/// Every emitter prefab, every reference to one, and every plain source.
pub fn find(inp: &Inputs) -> Found {
    let mut b = Builder {
        found: Found {
            audio: AudioDef::default(),
            clip_paths: Vec::new(),
            by_prefab: HashMap::new(),
        },
        clip_ids: HashMap::new(),
        refs: HashMap::new(),
    };
    let mut paths: Vec<&String> = inp.prefabs.keys().collect();
    paths.sort();
    for p in &paths {
        let root = &inp.prefabs[p.as_str()];
        if comp(root, "AudioEmitter").is_some() {
            if let Some(id) = b.sound(stem(p), root) {
                b.found.by_prefab.insert((*p).clone(), id);
            }
        }
    }
    let emitters = b.found.by_prefab.clone();
    for p in &paths {
        let root = &inp.prefabs[p.as_str()];
        if comp(root, "AudioEmitter").is_none() {
            b.walk(&stem(p), root, &emitters);
        }
    }
    for root in inp.main["roots"].as_array().into_iter().flatten() {
        b.walk("main", root, &emitters);
    }
    let mut refs: Vec<(String, SoundId)> = b.refs.into_iter().collect();
    refs.sort();
    b.found.audio.refs = refs;
    b.found
}

/// Converts every clip into `out/audio/<name>.adp` (kept when newer than
/// its source), filling in lengths and channel counts.
pub fn convert(assets: &Path, out: &Path, audio: &mut AudioDef, clip_paths: &[String]) -> Result<(), String> {
    let dir = out.join("audio");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Clips any music sound plays are streamed, in stereo.
    let mut music = vec![false; clip_paths.len()];
    for s in &audio.sounds {
        if s.group == SoundGroup::Music {
            for &c in &s.clips {
                music[c as usize] = true;
            }
        }
    }
    let jobs: Vec<usize> = (0..clip_paths.len()).collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let results: Vec<Result<(usize, f32, u8), String>> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .chunks(jobs.len().div_ceil(threads).max(1))
            .map(|chunk| {
                let (dir, music, clips) = (&dir, &music, &audio.clips);
                s.spawn(move || {
                    chunk
                        .iter()
                        .map(|&i| {
                            let src = assets.join(&clip_paths[i]);
                            let dst = dir.join(format!("{}.adp", clips[i].name));
                            let fresh = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
                            if let (Some(d), Some(s)) = (fresh(&dst), fresh(&src)) {
                                if d >= s {
                                    let head = std::fs::read(&dst).map_err(|e| e.to_string())?;
                                    if let Some(a) = Adp::header(&head) {
                                        let n = a.channels.len() as u8;
                                        if (n == 2) == music[i] || n == 1 {
                                            return Ok((i, a.samples as f32 / a.rate as f32, n));
                                        }
                                    }
                                }
                            }
                            let pcm = clipfile::prepare(&src, music[i])?;
                            let samples = pcm.channels[0].len();
                            let adp = pcm.to_adp();
                            std::fs::write(&dst, adp.to_bytes()).map_err(|e| e.to_string())?;
                            Ok((i, samples as f32 / RATE as f32, adp.channels.len() as u8))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    for r in results {
        let (i, seconds, channels) = r?;
        let c = &mut audio.clips[i];
        c.seconds = seconds;
        c.channels = channels;
        c.stream = music[i];
    }
    Ok(())
}
