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
    fn read(&self, slot: &str) -> Result<Option<Vec<u8>>, platform::ReadError> {
        match crate::savefile::read(std::path::Path::new(&Self::path(slot))) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(platform::ReadError),
        }
    }

    fn write(&mut self, slot: &str, data: &[u8]) -> bool {
        crate::savefile::write(std::path::Path::new(&Self::path(slot)), data).is_ok()
    }

    fn delete(&mut self, slot: &str) {
        let path = Self::path(slot);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{path}.bak"));
        let _ = std::fs::remove_file(format!("{path}.tmp"));
    }
}
