//! Finds a Kingdom: New Lands install in the usual places.

use std::path::{Path, PathBuf};

/// The Unity data folder inside a game folder (`Kingdom_Data` on Windows and
/// Linux, `Contents/Resources/Data` in the macOS app).
pub fn data_dir(game: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(rd) = std::fs::read_dir(game) {
        for e in rd.flatten() {
            let p = e.path();
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with("_Data"))
            {
                candidates.push(p);
            } else if p.extension().is_some_and(|x| x == "app") {
                candidates.push(p.join("Contents/Resources/Data"));
            }
        }
    }
    candidates.push(game.join("Contents/Resources/Data"));
    candidates.push(game.to_path_buf());
    candidates
        .into_iter()
        .find(|d| d.join("globalgamemanagers").is_file())
}

fn steam_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(h) = &home {
        v.push(h.join(".steam/steam"));
        v.push(h.join(".local/share/Steam"));
        v.push(h.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
        v.push(h.join("Library/Application Support/Steam"));
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(p) = std::env::var_os(var) {
            v.push(PathBuf::from(p).join("Steam"));
        }
    }
    v.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    v
}

/// Steam library folders: each root plus the ones listed in `libraryfolders.vdf`.
fn steam_libraries() -> Vec<PathBuf> {
    let mut libs: Vec<PathBuf> = Vec::new();
    for root in steam_roots() {
        if !root.join("steamapps").is_dir() {
            continue;
        }
        libs.push(root.clone());
        if let Ok(text) = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
            for line in text.lines() {
                let mut parts = line.split('"').filter(|s| !s.trim().is_empty());
                if parts.next() == Some("path") {
                    if let Some(p) = parts.next() {
                        libs.push(PathBuf::from(p.replace("\\\\", "\\")));
                    }
                }
            }
        }
    }
    libs.dedup();
    libs
}

pub fn find_game() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = steam_libraries()
        .into_iter()
        .map(|l| l.join("steamapps/common/Kingdom New Lands"))
        .collect();
    if let Some(h) = std::env::var_os("HOME").map(PathBuf::from) {
        candidates.push(h.join("GOG Games/Kingdom New Lands"));
        candidates.push(h.join("Games/kingdom-new-lands"));
    }
    candidates.push(PathBuf::from(r"C:\GOG Games\Kingdom New Lands"));
    candidates.push(PathBuf::from(
        r"C:\Program Files (x86)\GOG Galaxy\Games\Kingdom New Lands",
    ));
    candidates.into_iter().find(|c| data_dir(c).is_some())
}
