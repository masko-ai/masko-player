//! The Masko player for Windows apps (spec/player.md). The engine decides
//! every clip; this crate keeps a canvas's files on the computer, decodes its
//! stacked AV1 clips with dav1d, plays their sound and draws the mascot in a
//! transparent window.
//!
//! Everything but the window builds and tests on any desktop, so the Mac runs
//! the same tests as Windows.

pub mod clock;
pub mod delivery;
pub mod kept;
mod loader;
pub mod media;
pub mod player;
mod slot;
#[cfg(windows)]
pub mod view;

pub use player::{ClipKind, Event, MaskoPlayer, Options, Position, Value};
