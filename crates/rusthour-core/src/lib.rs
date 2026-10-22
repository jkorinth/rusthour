//! Rust Hour simulation core.
//!
//! Pure, deterministic simulation logic: no I/O, no rendering, no async, no
//! wall-clock time, no global RNG. Must compile to `wasm32-unknown-unknown`.
//! See `AGENTS.md` §6 for the rules and §7 for module ownership.

pub mod behavior;
pub mod control;
pub mod demand;
pub mod network;
pub mod routing;
pub mod scenario;
pub mod stats;
pub mod vehicle;

mod engine;
mod hash;

pub use engine::{Config, Engine, Stream, run_and_hash};
pub use hash::StateHasher;
