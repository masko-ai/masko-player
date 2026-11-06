//! The Masko player engine: every canvas rule, no video, no network, no
//! screen. `spec/rules.md` is what it implements; `spec/fixtures` proves it.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod engine;
mod format;

pub use engine::{At, ClipKind, Command, Engine, Next, State, Value};
pub use format::{LoadError, FEATURES, PLAYER_CONTRACT};

/// Reads a release delivery and makes its engine, or says why it is refused
/// (rules.md 1.2, 1.3).
pub fn load(delivery_json: &str) -> Result<Engine, LoadError> {
    Ok(Engine::new(format::decode_delivery(delivery_json)?))
}
