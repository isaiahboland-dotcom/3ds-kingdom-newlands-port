//! Traits implemented by each backend (desktop, n3ds).
#![no_std]

extern crate alloc;

use alloc::vec::Vec;

/// Where saves live: files on desktop, the SD card on 3DS
/// (`sdmc:/3ds/kingdom-3ds/`). Slots are short names like `game::save::GAME_SLOT`.
pub trait Storage {
    fn read(&self, slot: &str) -> Option<Vec<u8>>;
    /// Writes the whole slot; returns whether it worked.
    fn write(&mut self, slot: &str, data: &[u8]) -> bool;
    fn delete(&mut self, slot: &str);
}
