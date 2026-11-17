//! Downloads AssetRipper (pinned and checksummed, like `tools/setup.sh`) and
//! drives it headless to rip the game's files into a Unity project.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::Log;

const VERSION: &str = "2.0.0";

/// (release file name, SHA-256 from the release page) for this system.
fn release() -> Result<(&'static str, &'static str), String> {
    let r = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => (
            "linux_x64.tar.xz",
            "6f5d4352d36795b944243eb3ec790ec37dbca6d6756e52605bc8616e042d384e",
        ),
        ("linux", "aarch64") => (
            "linux_arm64.tar.xz",
            "8031752621d7fb27addab6108bad8bfbd43546bbb07b36912a987dfb4029c914",
        ),
        ("macos", "x86_64") => (
            "mac_x64.tar.xz",
            "1b3836f135d16845cb57595fadff5739cbb86d28caaf621005b4301dba9b5a56",
        ),
        ("macos", "aarch64") => (
            "mac_arm64.tar.xz",
            "237a6da404bf089c5e5c71a5a9a698c2ea580872292dfa5ce21e25fd1dbe9045",
        ),
        ("windows", "x86_64") => (
            "win_x64.zip",
            "9a7ef0e7c5c3ea5b90b4e6d855e2d98d5f7ec8c3f9e26fccbc194c6a7b01baf7",
        ),
        ("windows", "aarch64") => (
            "win_arm64.zip",
            "c2c3a335948aa0ecaf26baf22ac77e36993d1a1695b5db291cb63f66872627f1",
        ),
        (os, arch) => return Err(format!("no AssetRipper build for {os}/{arch}")),
    };
    Ok(r)
}

fn exe_name() -> String {
    format!("AssetRipper.GUI.Free{}", std::env::consts::EXE_SUFFIX)
}

/// Path to an AssetRipper executable, downloading it into `cache` once.
pub fn ensure(cache: &Path, log: &Log) -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("ASSETRIPPER") {
        return Ok(PathBuf::from(p));
    }
    let (file, sha) = release()?;
    let dir = cache.join(format!(
        "assetripper-{VERSION}-{}",
        file.split('.').next().unwrap()
    ));
    let exe = dir.join(exe_name());
    if exe.is_file() {
        return Ok(exe);
    }
    log(&format!(
        "Downloading AssetRipper {VERSION} (one time, about 100 MB)..."
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let archive = dir.join(format!("AssetRipper_{file}"));
    let url = format!(
        "https://github.com/AssetRipper/AssetRipper/releases/download/{VERSION}/AssetRipper_{file}"
    );
    let got = download(&url, &archive, log)?;
    if got != sha {
        std::fs::remove_file(&archive).ok();
        return Err(format!(
            "the AssetRipper download failed its checksum (got {got})"
        ));
    }
    log("Unpacking AssetRipper...");
    if file.ends_with(".zip") {
        let f = std::fs::File::open(&archive).map_err(|e| e.to_string())?;
        zip::ZipArchive::new(f)
            .and_then(|mut z| z.extract(&dir))
            .map_err(|e| format!("unzip: {e}"))?;
    } else {
        let tar_path = dir.join("ar.tar");
        {
            let mut input =
                std::io::BufReader::new(std::fs::File::open(&archive).map_err(|e| e.to_string())?);
            let mut output = std::io::BufWriter::new(
                std::fs::File::create(&tar_path).map_err(|e| e.to_string())?,
            );
            lzma_rs::xz_decompress(&mut input, &mut output).map_err(|e| format!("xz: {e}"))?;
            output.flush().map_err(|e| e.to_string())?;
        }
        let f = std::fs::File::open(&tar_path).map_err(|e| e.to_string())?;
        tar::Archive::new(f)
            .unpack(&dir)
            .map_err(|e| format!("untar: {e}"))?;
        std::fs::remove_file(&tar_path).ok();
    }
    std::fs::remove_file(&archive).ok();
    if !exe.is_file() {
        return Err(format!("AssetRipper unpacked without {}", exe_name()));
    }
    Ok(exe)
}

fn download(url: &str, to: &Path, log: &Log) -> Result<String, String> {
    let mut resp = agent(Duration::from_secs(1800))
        .get(url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let total: Option<u64> = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let mut reader = resp.body_mut().as_reader();
    let mut out = std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let (mut done, mut shown) = (0u64, 0u64);
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if done - shown >= 10 << 20 {
            shown = done;
            match total {
                Some(t) => log(&format!("  {} of {} MB", done >> 20, t >> 20)),
                None => log(&format!("  {} MB", done >> 20)),
            }
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(timeout))
        .build()
        .into()
}

/// A running headless AssetRipper; stopped when dropped.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

/// Rips the game's `*_Data` folder into `out` (an `ExportedProject` inside).
pub fn rip(exe: &Path, data: &Path, out: &Path, work: &Path, log: &Log) -> Result<(), String> {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        l.local_addr().map_err(|e| e.to_string())?.port()
    };
    let url = format!("http://127.0.0.1:{port}");
    let mut cmd = Command::new(exe);
    cmd.args(["--headless", "--port", &port.to_string(), "--log-path"])
        .arg(work.join("assetripper.log"))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::hide_window(&mut cmd);
    let mut server = Server(
        cmd.spawn()
            .map_err(|e| format!("couldn't start AssetRipper: {e}"))?,
    );

    let quick = agent(Duration::from_secs(10));
    let mut up = false;
    for _ in 0..240 {
        if quick.get(&format!("{url}/")).call().is_ok() {
            up = true;
            break;
        }
        if server.0.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Err(
                "AssetRipper exited right away (see assetripper.log in the work folder)".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if !up {
        return Err("AssetRipper didn't start answering".into());
    }

    log("Ripping the game's assets with AssetRipper (a minute or two)...");
    // The settings the extract tool expects, in case a saved configuration says otherwise.
    quick
        .post(&format!("{url}/Settings/Update"))
        .send_form([
            ("BundledAssetsExportMode", "DirectExport"),
            ("ScriptContentLevel", "Level2"),
            ("DefaultVersion", "0.0.0a0"),
            ("TargetVersion", "0.0.0a0"),
            ("AudioExportFormat", "Default"),
            ("ImageExportFormat", "Png"),
            ("LightmapTextureExportFormat", "Yaml"),
            ("SpriteExportMode", "Yaml"),
            ("ShaderExportMode", "Dummy"),
            ("TextExportMode", "Parse"),
            ("ScriptLanguageVersion", "AutoSafe"),
            ("ScriptExportMode", "Hybrid"),
            ("PreferOriginalTextureExtension", ""),
        ])
        .map_err(|e| format!("AssetRipper refused its settings: {e}"))?;
    agent(Duration::from_secs(600))
        .post(&format!("{url}/LoadFolder"))
        .send_form([("Path", data.to_string_lossy().as_ref())])
        .map_err(|e| format!("AssetRipper couldn't load the game folder: {e}"))?;
    agent(Duration::from_secs(3600))
        .post(&format!("{url}/Export/UnityProject"))
        .send_form([("Path", out.to_string_lossy().as_ref())])
        .map_err(|e| {
            format!("AssetRipper's export failed: {e} (see assetripper.log in the work folder)")
        })?;
    drop(server);
    if !out.join("ExportedProject/Assets").is_dir() {
        return Err("AssetRipper exported nothing (see assetripper.log in the work folder)".into());
    }
    Ok(())
}
