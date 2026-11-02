//! Saves on the SD card, `sdmc:/3ds/kingdom-3ds/<slot>.sav`.

const DIR: &str = "sdmc:/3ds/kingdom-3ds";

pub struct SdCard;

impl SdCard {
    pub fn new() -> SdCard {
        let _ = std::fs::create_dir_all(DIR);
        SdCard
    }

    fn path(slot: &str) -> String {
        format!("{DIR}/{slot}.sav")
    }
}

impl platform::Storage for SdCard {
    fn read(&self, slot: &str) -> Option<Vec<u8>> {
        std::fs::read(Self::path(slot)).ok()
    }

    fn write(&mut self, slot: &str, data: &[u8]) -> bool {
        // Write beside it and rename, so pulling the power never leaves half a save.
        let path = Self::path(slot);
        let tmp = format!("{path}.tmp");
        std::fs::write(&tmp, data).is_ok()
            && (std::fs::rename(&tmp, &path).is_ok()
                || (std::fs::remove_file(&path).is_ok() && std::fs::rename(&tmp, &path).is_ok()))
    }

    fn delete(&mut self, slot: &str) {
        let _ = std::fs::remove_file(Self::path(slot));
    }
}
