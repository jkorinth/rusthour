//! Rust Hour standard interface.
//!
//! Frontends (`rusthour-viz`, `rusthour-cli`) and remote clients talk to the
//! simulation **only** through this crate. Protocol types are serde-serializable
//! and versioned by [`PROTOCOL_VERSION`].

#[cfg(all(feature = "server", target_arch = "wasm32"))]
compile_error!("the `server` feature is native-only");

use serde::{Deserialize, Serialize};

pub use rusthour_core::Config;

/// Version of the wire protocol. Bump on any incompatible change to the
/// serialized types in this crate.
pub const PROTOCOL_VERSION: u32 = 1;

/// A point-in-time summary of a simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// Protocol version of the producer.
    pub protocol_version: u32,
    /// Steps taken so far.
    pub tick: u64,
    /// Simulated time elapsed, in milliseconds.
    pub sim_time_ms: u64,
    /// Stable hash of the full simulation state.
    pub state_hash: u64,
}

/// The facade every simulation backend implements: in-process now, remote in M8.
pub trait Simulation {
    /// Advances the simulation by `steps` fixed timesteps.
    fn step(&mut self, steps: u64);

    /// Returns the current status.
    fn status(&self) -> Status;
}

/// A simulation running in the caller's process (and thread).
#[derive(Debug, Clone)]
pub struct InProcess {
    engine: rusthour_core::Engine,
}

impl InProcess {
    /// Starts a new in-process simulation.
    #[must_use]
    pub fn new(config: Config, seed: u64) -> Self {
        Self {
            engine: rusthour_core::Engine::new(config, seed),
        }
    }
}

impl Simulation for InProcess {
    fn step(&mut self, steps: u64) {
        self.engine.step(steps);
    }

    fn status(&self) -> Status {
        Status {
            protocol_version: PROTOCOL_VERSION,
            tick: self.engine.tick(),
            sim_time_ms: self.engine.sim_time_ms(),
            state_hash: self.engine.state_hash(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_process_reports_progress() {
        let mut sim = InProcess::new(Config::default(), 42);
        sim.step(10);
        let status = sim.status();
        assert_eq!(status.tick, 10);
        assert_eq!(status.sim_time_ms, 1000);
        assert_eq!(status.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn status_round_trips_through_json() {
        let status = InProcess::new(Config::default(), 1).status();
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(serde_json::from_str::<Status>(&json).unwrap(), status);
    }
}
