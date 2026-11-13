//! Both screens of the game as the player sees them, drawn through a
//! backend's [`render::Renderer`]. Desktop and 3DS share all of it; the
//! backends only supply drawing, input, files and timing.

pub mod adpcm;
#[cfg(feature = "clipfile")]
pub mod clipfile;
pub mod heraldry;
pub mod render;
pub mod saves;
pub mod session;
pub mod sheet;
pub mod sound;
pub mod text;
pub mod top;
pub mod touch;
