//! Asset pipeline: AssetRipper export of the user's Kingdom_Data → platform-ready assets.
//!
//! ```text
//! cargo run -p extract -- --ripped reference/ripped/ExportedProject --out assets
//! ```
//!
//! Current outputs (desktop):
//! - `sprites/page_N.png` + `sprites.json`: every Sprite, cut out of Unity's
//!   tightly packed atlases by its own mesh and repacked into ≤1024² pages.
//! - `clips.json`: AnimationClips (sprite swaps and child position keys).
//! - `scenes/{main,blocks}.json`, `prefabs.json`, `data.json`: object trees
//!   and ScriptableObjects with all serialized fields (see `scenes.rs`).
//! - `world.bin`: `game::data::WorldData` (postcard), what the game loads.

mod audio;
mod authored_clips;
mod clips;
mod fonts;
mod pack;
mod scenes;
mod sprites;
mod world;
mod yaml;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage: extract --ripped <AssetRipper ExportedProject dir> --out <assets dir>");
    ExitCode::FAILURE
}

/// Unity GUID → asset path, from every `.meta` file under `dir`.
fn guid_map(dir: &Path) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "meta") {
                let Ok(text) = std::fs::read_to_string(&p) else {
                    continue;
                };
                if let Some(guid) = text.lines().find_map(|l| l.strip_prefix("guid: ")) {
                    map.insert(guid.trim().to_owned(), p.with_extension(""));
                }
            }
        }
    }
    map
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let arg = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
    };
    let (Some(ripped), Some(out)) = (arg("--ripped"), arg("--out")) else {
        return usage();
    };
    let assets = Path::new(ripped).join("Assets");
    let out = PathBuf::from(out);
    if !assets.is_dir() {
        eprintln!(
            "{} is not an AssetRipper ExportedProject (no Assets/)",
            ripped
        );
        return ExitCode::FAILURE;
    }

    let guids = guid_map(&assets);
    println!("indexed {} assets", guids.len());

    let mut sheet = match sprites::extract_all(&assets.join("Sprite"), &guids) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sprites: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "cut {} sprites ({} skipped)",
        sheet.sprites.len(),
        sheet.skipped
    );

    // The game's pixel font, as glyph sprites.
    match fonts::extract(&assets.join("Resources/fonts"), "Kingdom", "Font Texture") {
        Ok((font, glyphs)) => {
            println!("font: {} glyphs", font.glyphs.len());
            sheet.sprites.extend(glyphs);
            let json = serde_json::to_string(&font).expect("font serialize");
            if let Err(e) = std::fs::create_dir_all(&out)
                .and_then(|_| std::fs::write(out.join("font.json"), json))
            {
                eprintln!("font: {e}");
                return ExitCode::FAILURE;
            }
        }
        Err(e) => eprintln!("warning: font: {e}"),
    }

    if let Err(e) = pack::write_pages(&sheet, &out.join("sprites"), &out.join("sprites.json")) {
        eprintln!("packing: {e}");
        return ExitCode::FAILURE;
    }

    match clips::extract_all(&assets.join("AnimationClip"), &sheet.names_by_guid) {
        Ok(all) => {
            let json = serde_json::to_string(&all).expect("clips serialize");
            if let Err(e) = std::fs::write(out.join("clips.json"), json) {
                eprintln!("clips: {e}");
                return ExitCode::FAILURE;
            }
            println!("wrote {} clips", all.len());
        }
        Err(e) => {
            eprintln!("clips: {e}");
            return ExitCode::FAILURE;
        }
    }

    let animations = match authored_clips::extract(&assets, &guids, &sheet.names_by_guid) {
        Ok(animations) => animations,
        Err(e) => {
            eprintln!("authored animation: {e}");
            return ExitCode::FAILURE;
        }
    };
    let sizes: world::SpriteSizes = sheet
        .sprites
        .iter()
        .map(|s| (s.name.clone(), (s.img.w, s.img.h, s.ppu)))
        .collect();
    if let Err(e) = export_scenes(
        &assets,
        &guids,
        &out,
        &sheet.names_by_path,
        &sizes,
        animations,
    ) {
        eprintln!("scenes: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn write_json(path: &Path, v: &serde_json::Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string(v).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn rel(assets: &Path, p: &Path) -> String {
    p.strip_prefix(assets)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn export_scenes(
    assets: &Path,
    guids: &HashMap<String, PathBuf>,
    out: &Path,
    sprite_names: &HashMap<String, String>,
    sprite_sizes: &world::SpriteSizes,
    animations: game::animation::Library,
) -> Result<(), String> {
    let r = scenes::Resolver::new(guids, assets);
    let main = scenes::export_file(&assets.join("main.unity"), &r)?;
    write_json(&out.join("scenes/main.json"), &main)?;
    let blocks = scenes::export_file(&assets.join("blocks.unity"), &r)?;
    write_json(&out.join("scenes/blocks.json"), &blocks)?;
    // The menu scene holds the island map.
    let loader = scenes::export_file(&assets.join("loader.unity"), &r)?;
    write_json(&out.join("scenes/loader.json"), &loader)?;

    let mut prefabs = serde_json::Map::new();
    for p in scenes::files_with_ext(assets, "prefab") {
        let v = scenes::export_file(&p, &r)?;
        if let Some(root) = v["roots"].get(0) {
            prefabs.insert(rel(assets, &p), root.clone());
        }
    }
    println!("exported {} prefabs", prefabs.len());
    write_json(
        &out.join("prefabs.json"),
        &serde_json::Value::Object(prefabs.clone()),
    )?;

    let mut data = serde_json::Map::new();
    for dir in ["Resources", "MonoBehaviour"] {
        for p in scenes::files_with_ext(&assets.join(dir), "asset") {
            // Some third-party assets (Rewired controller maps) aren't valid YAML
            // for our parser; they're irrelevant, so skip them.
            match scenes::export_data_asset(&p, &r) {
                Ok(Some(v)) => {
                    data.insert(rel(assets, &p), v);
                }
                Ok(None) => {}
                Err(e) => eprintln!("skip {e}"),
            }
        }
    }
    println!("exported {} data assets", data.len());
    write_json(
        &out.join("data.json"),
        &serde_json::Value::Object(data.clone()),
    )?;

    let world::Export {
        mut world,
        audio_paths,
        visuals,
        catapults,
    } = world::build(
        &world::Inputs {
            main: &main,
            blocks: &blocks,
            loader: &loader,
            prefabs: &prefabs,
            data: &data,
            sprite_names,
            sprite_sizes,
            assets_dir: assets,
        },
        &animations,
    )?;
    audio::convert(assets, out, &mut world.audio, &audio_paths)?;
    println!(
        "audio: {} sounds, {} clips ({:.0} s), {} script fields",
        world.audio.sounds.len(),
        world.audio.clips.len(),
        world.audio.clips.iter().map(|c| c.seconds).sum::<f32>(),
        world.audio.refs.len()
    );
    let bytes = postcard::to_allocvec(&world).map_err(|e| e.to_string())?;
    std::fs::write(out.join("world.bin"), &bytes).map_err(|e| e.to_string())?;
    let authored = scene::authored::AssetAnimation {
        prefabs: visuals,
        clips: animations,
        catapults,
    };
    let animation = authored.encode(game::save::fingerprint(&bytes))?;
    std::fs::write(out.join(scene::authored::FILENAME), animation).map_err(|e| e.to_string())?;
    println!(
        "world: {} blocks, {} protos, {} scatterables, {} islands, {} lights, {} days ({} KiB)",
        world.blocks.len(),
        world.protos.len(),
        world.scatterables.len(),
        world.levels.len(),
        world.lights.len(),
        world.days.len(),
        bytes.len() / 1024
    );
    Ok(())
}
