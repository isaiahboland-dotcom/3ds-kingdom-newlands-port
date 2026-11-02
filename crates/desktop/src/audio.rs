//! Desktop sound: a small software mixer on a cpal output stream, playing
//! the same `.adp` clips the 3DS does (decoded to 16-bit on load; the
//! soundtrack when it starts).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use game::data::{AudioClipId, AudioDef};
use scene::adpcm::{self, Adp};
use scene::sound::{Mixer, Voice};

struct Clip {
    rate: u32,
    /// One buffer per channel.
    pcm: Vec<Vec<i16>>,
    /// Back to the source level (clips are stored normalized).
    gain: f32,
}

struct Playing {
    id: Voice,
    clip: Arc<Clip>,
    /// Position in source samples.
    pos: f64,
    gain: f32,
    pan: f32,
    looping: bool,
}

#[derive(Default)]
struct Shared {
    voices: Vec<Playing>,
    paused: bool,
    /// Voices that ended (polled by `playing`).
    live: Vec<Voice>,
}

pub struct Audio {
    _stream: Option<cpal::Stream>,
    shared: Arc<Mutex<Shared>>,
    clips: Vec<Option<Arc<Clip>>>,
    names: Vec<(String, bool)>,
    dir: PathBuf,
    next: Voice,
}

fn load(path: &Path) -> Option<Clip> {
    let a = Adp::from_bytes(&std::fs::read(path).ok()?)?;
    let pcm = a
        .channels
        .iter()
        .map(|c| adpcm::decode(c, a.samples as usize))
        .collect();
    Some(Clip {
        rate: a.rate,
        pcm,
        gain: a.gain,
    })
}

impl Audio {
    /// Opens the default output and loads every effect clip from `dir/audio`,
    /// Without a device it stays silent.
    pub fn new(dir: &Path, def: &AudioDef) -> Audio {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stream = Self::open(shared.clone())
            .map_err(|e| eprintln!("no sound: {e}"))
            .ok();
        let dir = dir.join("audio");
        let clips = def
            .clips
            .iter()
            .map(|c| {
                if c.stream {
                    None
                } else {
                    load(&dir.join(format!("{}.adp", c.name))).map(Arc::new)
                }
            })
            .collect();
        Audio {
            _stream: stream,
            shared,
            clips,
            names: def
                .clips
                .iter()
                .map(|c| (c.name.clone(), c.stream))
                .collect(),
            dir,
            next: 1,
        }
    }

    fn open(shared: Arc<Mutex<Shared>>) -> Result<cpal::Stream, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no output device")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let rate = config.sample_rate().0 as f64;
        let channels = config.channels() as usize;
        if config.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!(
                "unsupported sample format {:?}",
                config.sample_format()
            ));
        }
        let stream = device
            .build_output_stream(
                &config.into(),
                move |out: &mut [f32], _| {
                    out.fill(0.0);
                    let Ok(mut s) = shared.lock() else { return };
                    if s.paused {
                        return;
                    }
                    let mut ended = Vec::new();
                    for v in &mut s.voices {
                        let step = v.clip.rate as f64 / rate;
                        let len = v.clip.pcm[0].len();
                        // Equal-power-ish pan.
                        let g = v.gain * v.clip.gain;
                        let l = g * (1.0 - v.pan).min(1.0);
                        let r = g * (1.0 + v.pan).min(1.0);
                        for frame in out.chunks_mut(channels) {
                            if v.pos >= len as f64 {
                                if v.looping && len > 0 {
                                    v.pos -= len as f64;
                                } else {
                                    ended.push(v.id);
                                    break;
                                }
                            }
                            let i = v.pos as usize;
                            let f = (v.pos - i as f64) as f32;
                            let at = |c: &Vec<i16>| {
                                let a = c[i] as f32;
                                let b = c.get(i + 1).copied().unwrap_or(c[i]) as f32;
                                (a + (b - a) * f) / 32768.0
                            };
                            let (sl, sr) = match v.clip.pcm.len() {
                                1 => {
                                    let m = at(&v.clip.pcm[0]);
                                    (m, m)
                                }
                                _ => (at(&v.clip.pcm[0]), at(&v.clip.pcm[1])),
                            };
                            frame[0] += sl * l;
                            if channels > 1 {
                                frame[1] += sr * r;
                            }
                            v.pos += step;
                        }
                    }
                    s.voices.retain(|v| !ended.contains(&v.id));
                    let live: Vec<Voice> = s.voices.iter().map(|v| v.id).collect();
                    s.live = live;
                },
                |e| eprintln!("sound: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(stream)
    }
}

impl Mixer for Audio {
    fn play(&mut self, clip: AudioClipId, gain: f32, pan: f32, looping: bool) -> Option<Voice> {
        let c = match self.clips.get(clip as usize)? {
            Some(c) => c.clone(),
            // The soundtrack: decoded when it starts.
            None => {
                let (name, _) = self.names.get(clip as usize)?;
                let c = load(&self.dir.join(format!("{name}.adp")));
                Arc::new(c?)
            }
        };
        let id = self.next;
        self.next += 1;
        let mut s = self.shared.lock().ok()?;
        s.voices.push(Playing {
            id,
            clip: c,
            pos: 0.0,
            gain,
            pan,
            looping,
        });
        s.live.push(id);
        Some(id)
    }

    fn set(&mut self, voice: Voice, gain: f32, pan: f32) {
        if let Ok(mut s) = self.shared.lock() {
            if let Some(v) = s.voices.iter_mut().find(|v| v.id == voice) {
                v.gain = gain;
                v.pan = pan;
            }
        }
    }

    fn stop(&mut self, voice: Voice) {
        if let Ok(mut s) = self.shared.lock() {
            s.voices.retain(|v| v.id != voice);
            s.live.retain(|&v| v != voice);
        }
    }

    fn playing(&self, voice: Voice) -> bool {
        self.shared.lock().is_ok_and(|s| s.live.contains(&voice))
    }

    fn pause(&mut self, paused: bool) {
        if let Ok(mut s) = self.shared.lock() {
            s.paused = paused;
        }
    }
}
