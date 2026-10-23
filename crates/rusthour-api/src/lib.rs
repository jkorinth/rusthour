//! Rust Hour standard interface.
//!
//! Frontends (`rusthour-viz`, `rusthour-cli`) and remote clients talk to the
//! simulation **only** through this crate. Protocol types are serde-serializable
//! and versioned by [`PROTOCOL_VERSION`].

#[cfg(all(feature = "server", target_arch = "wasm32"))]
compile_error!("the `server` feature is native-only");

mod network;
mod pacer;
mod placement;
mod snapshot;

use serde::{Deserialize, Serialize};

pub use network::{
    Connection, NETWORK_SCHEMA_VERSION, Network, NetworkError, Node, NodeId, OsmWayPart, Point,
    Road, RoadId,
};
pub use pacer::{DEFAULT_BUDGET_S, MAX_FRAME_S, Pacer, scale_from_slider, slider_from_scale};
pub use placement::Placement;
pub use rusthour_core::Config;
pub use snapshot::{VehicleId, VehicleSnapshot};

/// Version of the wire protocol. Bump on any incompatible change to the
/// serialized types in this crate.
pub const PROTOCOL_VERSION: u32 = 2;

/// A point-in-time summary of a simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// Protocol version of the producer.
    pub protocol_version: u32,
    /// Steps taken so far.
    pub tick: u64,
    /// Simulated time elapsed, in milliseconds.
    pub sim_time_ms: u64,
    /// Vehicles on the network.
    pub vehicles: u32,
    /// Trips generated but not yet on the network.
    pub waiting: u32,
}

/// The facade every simulation backend implements: in-process now, remote in M8.
pub trait Simulation {
    /// Advances the simulation by `steps` fixed timesteps.
    fn step(&mut self, steps: u64);

    /// Returns the current status.
    fn status(&self) -> Status;

    /// Returns the road network the simulation runs on, e.g. for rendering.
    fn network(&self) -> &Network;

    /// Writes all vehicles into `out`, replacing its contents and reusing its
    /// buffers, so a steady-state frame allocates nothing.
    fn snapshot_into(&self, out: &mut VehicleSnapshot);

    /// Stable hash of the full simulation state. Costs O(state), so it is
    /// asked for on demand and is not part of [`Status`].
    fn state_hash(&self) -> u64;
}

/// A simulation running in the caller's process (and thread).
#[derive(Debug, Clone)]
pub struct InProcess {
    engine: rusthour_core::Engine,
    // The core keeps its own layout; this copy answers geometry queries.
    network: Network,
}

impl InProcess {
    /// Starts a new in-process simulation on an empty network.
    #[must_use]
    pub fn new(config: Config, seed: u64) -> Self {
        Self {
            engine: rusthour_core::Engine::new(config, seed),
            network: Network::default(),
        }
    }

    /// Starts a new in-process simulation on `network`.
    ///
    /// # Errors
    ///
    /// Returns a [`NetworkError`] if the network is inconsistent, e.g. a road
    /// references a missing node or has zero length.
    pub fn with_network(config: Config, seed: u64, network: Network) -> Result<Self, NetworkError> {
        let core = network.to_core()?;
        Ok(Self {
            engine: rusthour_core::Engine::with_network(config, seed, core),
            network,
        })
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
            vehicles: u32::try_from(self.engine.vehicles().live_count()).unwrap_or(u32::MAX),
            waiting: self.engine.waiting(),
        }
    }

    fn snapshot_into(&self, out: &mut VehicleSnapshot) {
        out.clear();
        out.tick = self.engine.tick();
        let vs = self.engine.vehicles();
        for lane in 0..vs.lane_count() {
            for &slot in vs.lane(lane) {
                let s = slot as usize;
                let id = vs.id(s);
                out.id.push(VehicleId(
                    u64::from(id.generation) << 32 | u64::from(id.slot),
                ));
                out.road.push(RoadId(vs.road(s)));
                out.lane.push(vs.lane_of(s));
                // The wire format is f32 (ADR-0004 §3).
                #[allow(clippy::cast_possible_truncation)]
                {
                    out.offset_m.push(vs.pos_m[s] as f32);
                    out.speed_mps.push(vs.speed_mps[s] as f32);
                    out.length_m.push(vs.params[s].length_m as f32);
                }
            }
        }
    }

    fn state_hash(&self) -> u64 {
        self.engine.state_hash()
    }

    fn network(&self) -> &Network {
        &self.network
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three one-lane 200 m roads in a ring, so every road is routable.
    fn ring() -> Network {
        let p = |x, y| Point { x, y };
        let pts = [p(0.0, 0.0), p(200.0, 0.0), p(100.0, 150.0)];
        let node = |i: usize| Node {
            position: pts[i],
            osm_node: None,
        };
        let road = |i: usize| Road {
            from_node: NodeId(u32::try_from(i).unwrap()),
            to_node: NodeId(u32::try_from((i + 1) % 3).unwrap()),
            geometry: vec![pts[i], pts[(i + 1) % 3]],
            speed_limit_mps: 13.9,
            lanes: 1,
            name: None,
            osm: None,
            roundabout: false,
        };
        let conn = |i: u32| Connection {
            from_road: RoadId(i),
            to_road: RoadId((i + 1) % 3),
        };
        Network {
            nodes: (0..3).map(node).collect(),
            roads: (0..3).map(road).collect(),
            connections: (0..3).map(conn).collect(),
        }
    }

    fn busy() -> InProcess {
        let config = Config {
            target_vehicles: 8,
            ..Config::default()
        };
        InProcess::with_network(config, 7, ring()).unwrap()
    }

    #[test]
    fn snapshot_matches_the_core_vehicles() {
        let mut sim = busy();
        sim.step(300);
        let mut snap = VehicleSnapshot::default();
        sim.snapshot_into(&mut snap);
        let status = sim.status();
        assert!(status.vehicles > 0, "demand should have spawned vehicles");
        assert_eq!(snap.len(), status.vehicles as usize);
        assert_eq!(snap.tick, status.tick);
        for i in 0..snap.len() {
            assert!(snap.road[i].0 < 3 && snap.lane[i] == 0);
            assert!((0.0..=200.0).contains(&snap.offset_m[i]));
            assert!(snap.speed_mps[i] >= 0.0 && snap.length_m[i] > 0.0);
        }
    }

    #[test]
    fn snapshot_buffers_are_reused_after_warm_up() {
        let mut sim = busy();
        sim.step(300);
        let mut snap = VehicleSnapshot::default();
        sim.snapshot_into(&mut snap);
        let caps = |s: &VehicleSnapshot| [s.id.capacity(), s.offset_m.capacity()];
        let before = caps(&snap);
        for _ in 0..20 {
            sim.step(10);
            sim.snapshot_into(&mut snap);
        }
        assert_eq!(caps(&snap), before);
    }

    #[test]
    fn status_carries_counts_and_no_hash() {
        let mut sim = busy();
        sim.step(50);
        let status = sim.status();
        assert_eq!(status.protocol_version, 2);
        assert!(status.vehicles + status.waiting > 0);
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains("state_hash"));
        assert_eq!(serde_json::from_str::<Status>(&json).unwrap(), status);
        assert_ne!(
            sim.state_hash(),
            InProcess::new(Config::default(), 7).state_hash()
        );
    }

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
