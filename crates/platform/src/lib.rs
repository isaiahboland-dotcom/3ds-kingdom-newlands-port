//! Traits implemented by each backend (desktop, n3ds).
#![no_std]

extern crate alloc;

#[cfg(any(feature = "std", test))]
extern crate std;

#[cfg(any(feature = "std", test))]
pub mod savefile;

use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadError;

/// Where saves live: files on desktop, the SD card on 3DS
/// (`sdmc:/3ds/kingdom-3ds/`). Slots are short names like `game::save::GAME_SLOT`.
pub trait Storage {
    /// Returns `Ok(None)` only when no save file exists.
    fn read(&self, slot: &str) -> Result<Option<Vec<u8>>, ReadError>;
    fn read_backup(&self, _slot: &str) -> Result<Option<Vec<u8>>, ReadError> {
        Ok(None)
    }
    fn write_preserving_backup(&mut self, _slot: &str, _data: &[u8]) -> bool {
        false
    }
    /// Writes the whole slot; returns whether it worked.
    fn write(&mut self, slot: &str, data: &[u8]) -> bool;
    fn delete(&mut self, slot: &str);
}
