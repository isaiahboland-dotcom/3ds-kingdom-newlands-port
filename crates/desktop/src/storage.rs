//! Saves as files: `$KINGDOM_SAVES`, else `$XDG_DATA_HOME/kingdom-3ds`, else
//! `~/.local/share/kingdom-3ds`.

use std::path::PathBuf;

pub struct Files {
    dir: PathBuf,
}

impl Files {
    pub fn new() -> Self {
        let dir = std::env::var_os("KINGDOM_SAVES")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .filter(|d| d.is_absolute())
                    .map(|d| d.join("kingdom-3ds"))
            })
            .or_else(|| std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("kingdom-3ds")))
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/kingdom-3ds"))
            })
            .unwrap_or_else(|| PathBuf::from("saves"));
        Files { dir }
    }

    fn path(&self, slot: &str) -> PathBuf {
        self.dir.join(format!("{slot}.sav"))
    }
}

impl platform::Storage for Files {
    fn read(&self, slot: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(slot)).ok()
    }

    fn write(&mut self, slot: &str, data: &[u8]) -> bool {
        // Write beside it and rename, so a crash never leaves half a save.
        let path = self.path(slot);
        let tmp = path.with_extension("tmp");
        std::fs::create_dir_all(&self.dir).is_ok()
            && std::fs::write(&tmp, data).is_ok()
            && std::fs::File::open(&tmp).and_then(|f| f.sync_all()).is_ok()
            && std::fs::rename(&tmp, &path).is_ok()
    }

    fn delete(&mut self, slot: &str) {
        let _ = std::fs::remove_file(self.path(slot));
    }
}
