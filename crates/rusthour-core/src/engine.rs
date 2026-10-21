//! The simulation engine: fixed timestep, seeded RNG, sim clock.

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;

use crate::StateHasher;
use crate::network::Network;

/// Engine-wide settings fixed at construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Length of one simulation step in milliseconds.
    pub dt_ms: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self { dt_ms: 100 }
    }
}

/// A deterministic simulation instance.
///
/// Time is an integer tick count, so there is no floating-point drift in the
/// clock. All randomness must come from `rng`, seeded once at construction.
#[derive(Debug, Clone)]
pub struct Engine {
    config: Config,
    tick: u64,
    rng: ChaCha8Rng,
    network: Network,
}

impl Engine {
    /// Creates an engine at time zero with an empty network.
    #[must_use]
    pub fn new(config: Config, seed: u64) -> Self {
        Self::with_network(config, seed, Network::default())
    }

    /// Creates an engine at time zero running on `network`.
    #[must_use]
    pub fn with_network(config: Config, seed: u64, network: Network) -> Self {
        Self {
            config,
            tick: 0,
            rng: ChaCha8Rng::seed_from_u64(seed),
            network,
        }
    }

    /// Advances the simulation by `steps` fixed timesteps.
    pub fn step(&mut self, steps: u64) {
        for _ in 0..steps {
            self.tick += 1;
        }
    }

    /// Number of steps taken so far.
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Simulated time elapsed, in milliseconds.
    #[must_use]
    pub fn sim_time_ms(&self) -> u64 {
        self.tick * u64::from(self.config.dt_ms)
    }

    /// The engine configuration.
    #[must_use]
    pub fn config(&self) -> Config {
        self.config
    }

    /// Stable hash of the complete simulation state.
    ///
    /// New state added to the engine must be fed in here, or the determinism
    /// harness will not notice when it diverges.
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        let mut h = StateHasher::new();
        h.write_u64(u64::from(self.config.dt_ms));
        h.write_u64(self.tick);
        h.write_bytes(&self.rng.get_seed());
        h.write_u128(self.rng.get_word_pos());
        self.network.hash_into(&mut h);
        h.finish()
    }
}

/// Runs a fresh engine for `steps` steps and returns its state hash.
///
/// This is the determinism harness entry point used by tests and the CLI.
#[must_use]
pub fn run_and_hash(config: Config, seed: u64, steps: u64) -> u64 {
    let mut engine = Engine::new(config, seed);
    engine.step(steps);
    engine.state_hash()
}
