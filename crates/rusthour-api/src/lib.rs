//! Rust Hour standard interface.
//!
//! Frontends (`rusthour-viz`, `rusthour-cli`) and remote clients talk to the
//! simulation **only** through this crate. Protocol types are serde-serializable
//! and versioned by [`PROTOCOL_VERSION`].

#[cfg(all(feature = "server", target_arch = "wasm32"))]
compile_error!("the `server` feature is native-only");

mod inspect;
mod network;
mod pacer;
mod placement;
mod signal;
mod snapshot;

use serde::{Deserialize, Serialize};

pub use inspect::{
    DriverInfo, Gone, MovementInfo, RoadInfo, StopCause, TurnType, VehicleInfo, WaitReason,
};
pub use network::{
    Connection, EndControl, MAX_LANES, NETWORK_SCHEMA_VERSION, Network, NetworkError, Node, NodeId,
    OsmWayPart, Point, Road, RoadClass, RoadId, Turns,
};
pub use pacer::{DEFAULT_BUDGET_S, MAX_FRAME_S, Pacer, scale_from_slider, slider_from_scale};
pub use placement::Placement;
pub use rusthour_core::Config;
pub use signal::{SignalSnapshot, SignalState};
pub use snapshot::{VehicleId, VehicleSnapshot};

/// Version of the wire protocol. Bump on any incompatible change to the
/// serialized types in this crate.
pub const PROTOCOL_VERSION: u32 = 3;

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
    /// Vehicles that reached their destination so far.
    pub arrived: u32,
    /// Vehicles removed so far for standing still too long (gridlock).
    pub removed_stuck: u32,
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

    /// Writes `(road, vehicles removed there as stuck)` into `out`, replacing
    /// its contents and reusing its buffer. Only roads with removals, by id.
    fn stuck_by_road_into(&self, out: &mut Vec<(RoadId, u32)>);

    /// Describes one live vehicle, or says what became of it. Meant for a
    /// selected vehicle a few times per second, not for all of them.
    ///
    /// # Errors
    ///
    /// [`Gone`] if the id no longer names a live vehicle.
    fn inspect_vehicle(&self, id: VehicleId) -> Result<VehicleInfo, Gone>;

    /// Describes one road; `None` for an unknown id.
    fn inspect_road(&self, road: RoadId) -> Option<RoadInfo>;

    /// Writes every signal's state and seconds to its next change into
    /// `out`, replacing its contents and reusing its buffers.
    fn signals_into(&self, out: &mut SignalSnapshot);
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
            arrived: self.engine.arrived(),
            removed_stuck: self.engine.removed_stuck(),
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
                out.id.push(VehicleId::from_core(id));
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

    fn stuck_by_road_into(&self, out: &mut Vec<(RoadId, u32)>) {
        out.clear();
        let counts = self.engine.stuck_by_road();
        out.extend(
            (0u32..)
                .zip(counts)
                .filter(|&(_, &n)| n > 0)
                .map(|(r, &n)| (RoadId(r), n)),
        );
    }

    fn network(&self) -> &Network {
        &self.network
    }

    fn inspect_vehicle(&self, id: VehicleId) -> Result<VehicleInfo, Gone> {
        inspect::vehicle(&self.engine, id)
    }

    fn inspect_road(&self, road: RoadId) -> Option<RoadInfo> {
        inspect::road(&self.engine, &self.network, road)
    }

    fn signals_into(&self, out: &mut SignalSnapshot) {
        use rusthour_core::control::signal::Light;
        out.clear();
        out.tick = self.engine.tick();
        for (road, light, next_s) in self.engine.signals() {
            out.road.push(RoadId(road));
            out.state.push(match light {
                Light::Green => SignalState::Green,
                Light::Amber => SignalState::Amber,
                Light::Red => SignalState::Red,
            });
            #[allow(clippy::cast_possible_truncation)] // the wire format is f32
            out.next_change_s.push(next_s as f32);
        }
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
            class: crate::RoadClass::Unknown,
            priority_road: false,
            end_control: crate::EndControl::None,
            turn_lanes: vec![],
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
        assert_eq!(status.protocol_version, 3);
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
    fn arrivals_and_per_road_removals_match_the_status() {
        let config = Config {
            target_vehicles: 40,
            stuck_timeout_s: 1,
            ..Config::default()
        };
        let mut sim = InProcess::with_network(config, 7, ring()).unwrap();
        sim.step(3000);
        let status = sim.status();
        let mut by_road = vec![(RoadId(9), 9)]; // replaced, not appended to
        sim.stuck_by_road_into(&mut by_road);
        assert!(status.arrived > 0 && status.removed_stuck > 0, "{status:?}");
        assert!(by_road.iter().all(|&(r, n)| r.0 < 3 && n > 0));
        assert_eq!(
            by_road.iter().map(|&(_, n)| n).sum::<u32>(),
            status.removed_stuck
        );
    }

    #[test]
    fn inspecting_every_step_does_not_change_the_state() {
        let config = Config {
            target_vehicles: 40,
            stuck_timeout_s: 1,
            ..Config::default()
        };
        let run = |inspect: bool| {
            let mut sim = InProcess::with_network(config, 7, ring()).unwrap();
            let mut snap = VehicleSnapshot::default();
            for _ in 0..1500 {
                sim.step(1);
                if inspect {
                    sim.snapshot_into(&mut snap);
                    for &id in &snap.id {
                        sim.inspect_vehicle(id).unwrap();
                    }
                    for r in 0..3 {
                        sim.inspect_road(RoadId(r)).unwrap();
                    }
                }
            }
            sim.state_hash()
        };
        assert_eq!(run(true), run(false));
    }

    #[test]
    fn inspect_road_matches_the_snapshot_and_rejects_unknown_ids() {
        let mut sim = busy();
        sim.step(300);
        let mut snap = VehicleSnapshot::default();
        sim.snapshot_into(&mut snap);
        assert!(sim.inspect_road(RoadId(3)).is_none());
        let mut total = 0;
        for r in 0..3 {
            let info = sim.inspect_road(RoadId(r)).unwrap();
            let on_road = (0..snap.len()).filter(|&i| snap.road[i] == RoadId(r));
            assert_eq!(info.vehicles as usize, on_road.clone().count());
            assert_eq!(info.mean_speed_mps.is_some(), info.vehicles > 0);
            assert!((180.0..=200.5).contains(&info.length_m));
            assert_eq!(info.movements.len(), 1);
            assert_eq!(info.movements[0].to_road, RoadId((r + 1) % 3));
            total += info.vehicles;
        }
        assert_eq!(total, sim.status().vehicles);
    }

    #[test]
    fn a_vehicle_is_described_until_it_arrives_and_then_reported_gone() {
        let mut sim = busy();
        sim.step(100);
        let mut snap = VehicleSnapshot::default();
        sim.snapshot_into(&mut snap);
        let id = snap.id[0];
        let info = sim.inspect_vehicle(id).unwrap();
        assert_eq!(info.id, id);
        assert_eq!(info.route_ahead[0], info.road);
        assert!(info.speed_mps >= 0.0 && info.trip_time_s > 0.0);
        assert!(matches!(
            sim.inspect_vehicle(VehicleId(u64::MAX)),
            Err(Gone::Unknown)
        ));
        sim.step(20_000);
        assert!(matches!(
            sim.inspect_vehicle(id),
            Err(Gone::Arrived { .. } | Gone::Unknown)
        ));
    }

    #[test]
    fn signals_report_their_state_and_seconds_to_the_next_change() {
        // Signals on road 0 only: the node at its end gets a one-phase plan.
        let mut net = ring();
        net.roads[0].end_control = crate::EndControl::Signals;
        let mut sim = InProcess::with_network(Config::default(), 7, net).unwrap();
        let mut snap = SignalSnapshot::default();
        sim.signals_into(&mut snap);
        assert_eq!(snap.road, [RoadId(0)]);
        assert_eq!(snap.state, [SignalState::Green]);
        assert!((snap.next_change_s[0] - 25.0).abs() < 1e-6);
        sim.step(260);
        sim.signals_into(&mut snap);
        assert_eq!((snap.tick, snap.len()), (260, 1));
        assert_eq!(snap.state, [SignalState::Amber]);
        assert!((snap.next_change_s[0] - 2.0).abs() < 1e-6);
        let json = serde_json::to_string(&snap).unwrap();
        assert_eq!(serde_json::from_str::<SignalSnapshot>(&json).unwrap(), snap);
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
