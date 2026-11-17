//! The install itself: rip the game, convert it, build the 3DS files, copy
//! them to the SD card. The same steps as `build.sh`, with the helper tools
//! taken from the release package instead of built.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{assetripper, gamefind, Log};

#[derive(Clone)]
pub struct Options {
    pub game: PathBuf,
    pub sd: PathBuf,
    pub want_3dsx: bool,
    pub want_cia: bool,
}

/// Folders searched for the package's helper programs and data files.
fn search_dirs() -> Vec<PathBuf> {
    let base = std::env::var_os("KINGDOM_PACKAGE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(Path::to_path_buf))
        })
        .unwrap_or_default();
    vec![base.clone(), base.join("bin"), base.join("data")]
}

fn find_file(name: &str) -> Option<PathBuf> {
    search_dirs()
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn find_tool(name: &str) -> Option<PathBuf> {
    find_file(&format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

const CORE_TOOLS: [&str; 3] = ["extract", "pack3ds", "pack3dsx"];
const CIA_TOOLS: [&str; 3] = ["ciaart", "makerom", "bannertool"];

/// What the package is missing for a `.3dsx` (and for a `.cia`), if anything.
pub fn missing(cia: bool) -> Vec<String> {
    let mut m: Vec<String> = CORE_TOOLS
        .iter()
        .filter(|t| find_tool(t).is_none())
        .map(|t| t.to_string())
        .collect();
    if find_file("kingdom-base.3dsx").is_none() {
        m.push("kingdom-base.3dsx".into());
    }
    if cia {
        m.extend(
            CIA_TOOLS
                .iter()
                .filter(|t| find_tool(t).is_none())
                .map(|t| t.to_string()),
        );
        for f in ["kingdom.elf", "kingdom.rsf"] {
            if find_file(f).is_none() {
                m.push(f.into());
            }
        }
    }
    m
}

/// Where downloads and scratch files go.
pub fn work_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("KINGDOM_WORK") {
        return PathBuf::from(p);
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("kingdom-3ds-installer")
}

/// Runs a helper program, passing its output lines to the log.
fn run(cmd: &mut Command, log: &Log) -> Result<(), String> {
    crate::hide_window(cmd);
    let name = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{name}: {e}"))?;
    let (out, err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    std::thread::scope(|s| {
        for stream in [
            Box::new(out) as Box<dyn std::io::Read + Send>,
            Box::new(err),
        ] {
            s.spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(stream)
                    .lines()
                    .map_while(Result::ok)
                {
                    if !line.trim().is_empty() {
                        log(&format!("  {line}"));
                    }
                }
            });
        }
    });
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} failed ({status})",
            Path::new(&name)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ))
    }
}

pub fn install(o: &Options, log: &Log) -> Result<(), String> {
    if !o.want_3dsx && !o.want_cia {
        return Err("choose .3dsx, .cia or both".into());
    }
    let data = gamefind::data_dir(&o.game).ok_or_else(|| {
        format!("{} doesn't look like Kingdom: New Lands (no Kingdom_Data folder with globalgamemanagers inside)", o.game.display())
    })?;
    log(&format!("Game: {}", data.display()));
    let lost = gamefind::missing_files(&data);
    if !lost.is_empty() {
        return Err(format!(
            "the game folder is incomplete, it's missing: {}. Copy the whole Kingdom New Lands folder (about 400 MB), or let Steam download it again.",
            lost.join(", ")
        ));
    }
    if !o.sd.is_dir() {
        return Err(format!(
            "{} isn't a folder (is the SD card inserted?)",
            o.sd.display()
        ));
    }
    let probe = o.sd.join(".kingdom-write-test");
    std::fs::write(&probe, b"x").map_err(|e| format!("can't write to {}: {e}", o.sd.display()))?;
    std::fs::remove_file(&probe).ok();
    let gone = missing(o.want_cia);
    if !gone.is_empty() {
        return Err(format!(
            "this installer package is incomplete, missing: {}",
            gone.join(", ")
        ));
    }
    let tool = |n: &str| find_tool(n).unwrap();

    let work = work_dir();
    let rip = work.join("ripped");
    let assets = work.join("assets");
    let out = work.join("out");
    for d in [&rip, &assets, &out] {
        std::fs::remove_dir_all(d).ok();
    }
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;

    log("Step 1/5: getting AssetRipper");
    let exe = assetripper::ensure(&work, log)?;
    log("Step 2/5: reading the game's files");
    assetripper::rip(&exe, &data, &rip, &work, log)?;
    let project = rip.join("ExportedProject");
    log("Step 3/5: converting the game's data");
    run(
        Command::new(tool("extract"))
            .arg("--ripped")
            .arg(&project)
            .arg("--out")
            .arg(&assets),
        log,
    )?;
    log("Step 4/5: building the 3DS files");
    let romfs = out.join("romfs");
    run(Command::new(tool("pack3ds")).arg(&assets).arg(&romfs), log)?;
    let card = out.join("sdcard");
    if o.want_3dsx {
        std::fs::create_dir_all(card.join("3ds")).map_err(|e| e.to_string())?;
        run(
            Command::new(tool("pack3dsx"))
                .arg(find_file("kingdom-base.3dsx").unwrap())
                .arg(&romfs)
                .arg(card.join("3ds/kingdom.3dsx")),
            log,
        )?;
    }
    if o.want_cia {
        make_cia(&project, &assets, &romfs, &out, &card, log)?;
    }

    log("Step 5/5: copying to the SD card");
    let n = copy_merge(&card, &o.sd, log)?;
    log(&format!("Copied {n} files."));
    std::fs::remove_dir_all(work.join("ripped")).ok();
    std::fs::remove_dir_all(&assets).ok();
    std::fs::remove_dir_all(&out).ok();

    if !o.sd.join("3ds/dspfirm.cdc").is_file() {
        log("Note: there's no 3ds/dspfirm.cdc on the card. Run DSP1 on the console once, or the game has no sound.");
    }
    log("Done! Eject the SD card safely.");
    if o.want_3dsx {
        log("  .3dsx: start Kingdom from the Homebrew Launcher.");
    }
    if o.want_cia {
        log("  .cia: install cias/kingdom.cia with FBI, then start it from the HOME Menu.");
    }
    Ok(())
}

fn make_cia(
    project: &Path,
    assets: &Path,
    romfs: &Path,
    out: &Path,
    card: &Path,
    log: &Log,
) -> Result<(), String> {
    let art = out.join("cia");
    std::fs::create_dir_all(&art).map_err(|e| e.to_string())?;
    let tool = |n: &str| find_tool(n).unwrap();
    log("Making the .cia (HOME Menu icon, banner, package)");
    run(
        Command::new(tool("ciaart"))
            .arg(project)
            .arg(assets)
            .arg(&art),
        log,
    )?;
    run(
        Command::new(tool("bannertool"))
            .args(["makebanner", "-i"])
            .arg(art.join("banner.png"))
            .arg("-a")
            .arg(art.join("banner.wav"))
            .arg("-o")
            .arg(art.join("banner.bnr")),
        log,
    )?;
    run(
        Command::new(tool("bannertool"))
            .args([
                "makesmdh",
                "-s",
                "Kingdom 3DS",
                "-l",
                "Kingdom: New Lands, fan port",
                "-p",
                "Fan port (your own game files)",
                "-i",
            ])
            .arg(art.join("icon.png"))
            .arg("-o")
            .arg(art.join("icon.icn")),
        log,
    )?;
    std::fs::create_dir_all(card.join("cias")).map_err(|e| e.to_string())?;
    let mut romfs_arg = std::ffi::OsString::from("-DAPP_ROMFS=");
    romfs_arg.push(romfs);
    run(
        Command::new(tool("makerom"))
            .args(["-f", "cia", "-target", "t", "-o"])
            .arg(card.join("cias/kingdom.cia"))
            .arg("-elf")
            .arg(find_file("kingdom.elf").unwrap())
            .arg("-rsf")
            .arg(find_file("kingdom.rsf").unwrap())
            .arg("-icon")
            .arg(art.join("icon.icn"))
            .arg("-banner")
            .arg(art.join("banner.bnr"))
            .arg(romfs_arg),
        log,
    )
}

/// Copies `from` over `to`, merging folders; files already on the card are
/// kept (saves live beside the game) and only the audio is skipped when it's
/// already there with the same size.
fn copy_merge(from: &Path, to: &Path, log: &Log) -> Result<usize, String> {
    fn go(from: &Path, to: &Path, count: &mut usize, log: &Log) -> Result<(), String> {
        std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
        for e in std::fs::read_dir(from).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            let (src, dst) = (e.path(), to.join(e.file_name()));
            if src.is_dir() {
                go(&src, &dst, count, log)?;
                continue;
            }
            let same = src.extension().is_some_and(|x| x == "adp")
                && std::fs::metadata(&dst).ok().map(|m| m.len())
                    == std::fs::metadata(&src).ok().map(|m| m.len());
            if !same {
                std::fs::copy(&src, &dst)
                    .map_err(|e| format!("copying to {}: {e}", dst.display()))?;
                if let Ok(f) = std::fs::File::open(&dst) {
                    f.sync_all().ok();
                }
                *count += 1;
                if (*count).is_multiple_of(50) {
                    log(&format!("  {count} files..."));
                }
            }
        }
        Ok(())
    }
    let mut n = 0;
    go(from, to, &mut n, log)?;
    Ok(n)
}
