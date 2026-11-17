//! Puts a RomFS on a `.3dsx`: the part of `3dsxtool --romfs` that needs the
//! game's own files, so it can run on the player's machine without devkitPro.
//!
//! `pack3dsx <base.3dsx> <romfs dir> <out.3dsx>`
//!
//! The base is the game built with an (empty) romfs, so its header already
//! has the RomFS offset field. Whatever follows that offset is replaced.

#[path = "../romfs.rs"]
mod romfs;

use std::process::ExitCode;

fn run(base: &str, dir: &str, out: &str) -> Result<usize, String> {
    let mut bin = std::fs::read(base).map_err(|e| format!("{base}: {e}"))?;
    if bin.len() < 0x2C || &bin[..4] != b"3DSX" {
        return Err(format!("{base} isn't a .3dsx"));
    }
    let header = u16::from_le_bytes([bin[4], bin[5]]);
    let off = u32::from_le_bytes(bin[0x28..0x2C].try_into().unwrap()) as usize;
    if header < 0x2C || off == 0 || off > bin.len() {
        return Err(format!(
            "{base} was built without a romfs (no RomFS offset in its header)"
        ));
    }
    bin.truncate(off);
    let image = romfs::build(std::path::Path::new(dir))?;
    bin.extend_from_slice(&image);
    std::fs::write(out, &bin).map_err(|e| format!("{out}: {e}"))?;
    Ok(image.len())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [base, dir, out] = &args[..] else {
        eprintln!("usage: pack3dsx <base.3dsx> <romfs dir> <out.3dsx>");
        return ExitCode::FAILURE;
    };
    match run(base, dir, out) {
        Ok(n) => {
            println!("{out}: romfs {n} bytes");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("pack3dsx: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// With the entry order `3dsxtool` happened to use, the image must be
    /// byte-for-byte what it made for the 3DS build in this checkout (skipped
    /// when that build isn't there).
    #[test]
    fn matches_3dsxtool() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let built = root.join("crates/n3ds/target/armv6k-nintendo-3ds/release/kingdom.3dsx");
        let dir = root.join("crates/n3ds/romfs");
        let (Ok(bin), true) = (std::fs::read(&built), dir.join("world.bin").exists()) else {
            eprintln!("no 3DS build here; skipping");
            return;
        };
        let off = u32::from_le_bytes(bin[0x28..0x2C].try_into().unwrap()) as usize;
        let image = super::romfs::build_ordered(&dir, |names| {
            let rank = |n: &String| match n.as_str() {
                "world.bin" => 0,
                "sheet.bin" => 2,
                _ => 1,
            };
            names.sort_by_key(|n| (rank(n), n.clone()));
        })
        .unwrap();
        assert_eq!(image.len(), bin.len() - off, "size");
        assert!(image == bin[off..], "romfs bytes differ from 3dsxtool's");
    }
}
