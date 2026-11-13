//! Traits implemented by each backend (desktop, n3ds).
#![no_std]

extern crate alloc;

use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadError;

/// Where saves live: files on desktop, the SD card on 3DS
/// (`sdmc:/3ds/kingdom-3ds/`). Slots are short names like `game::save::GAME_SLOT`.
pub trait Storage {
    /// Returns `Ok(None)` only when no save file exists.
    fn read(&self, slot: &str) -> Result<Option<Vec<u8>>, ReadError>;
    /// The previous copy a backend keeps beside the slot (the 3DS keeps the
    /// last save when it replaces a file), for when the slot is damaged.
    fn read_backup(&self, _slot: &str) -> Result<Option<Vec<u8>>, ReadError> {
        Ok(None)
    }
    /// Makes the backup the slot again, keeping the damaged file aside.
    /// Every step leaves a copy `read` finds. Returns whether it worked.
    fn restore_backup(&mut self, _slot: &str) -> bool {
        false
    }
    /// Writes the whole slot; returns whether it worked.
    fn write(&mut self, slot: &str, data: &[u8]) -> bool;
    fn delete(&mut self, slot: &str);
}
