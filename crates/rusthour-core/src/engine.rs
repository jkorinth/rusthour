//! The simulation engine: fixed timestep, seeded RNG, sim clock, and the
//! step pipeline of ADR-0004 §2.

use std::collections::VecDeque;

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;

use crate::StateHasher;
use crate::demand::{DemandContext, RandomOd, Trip};
use crate::network::Network;
use crate::routing::Router;
use crate::vehicle::idm::{self, IdmParams, Leader};
use crate::vehicle::{NewVehicle, VehicleId, Vehicles};

/// Hardest braking the integration applies (m/s²).
const MAX_DECEL_MPS2: f64 = 9.0;
/// Fixed part of a lane head's look-ahead (m); the rest is `v²/b`.
const LOOK_AHEAD_M: f64 = 50.0;
/// Most trips routed and inserted per step; the rest wait (ADR-0004 §2).
// shortcut: a fixed guess (~3× the steady-state rate at 5,000 vehicles), tune when M2.8 times routing.
const MAX_ROUTES_PER_STEP: u32 = 8;

/// The M2 car, the same for every vehicle until M4 driver profiles.
fn car() -> IdmParams {
    let (a, b) = (1.5, 2.0);
    IdmParams {
        desired_speed_factor: 1.0,
        time_headway_s: 1.5,
        max_accel_mps2: a,
        comfort_decel_mps2: b,
        min_gap_m: 2.0,
        length_m: 4.5,
        sqrt_ab: f64::sqrt(a * b),
    }
}

/// A vehicle past its road end, waiting to enter the next road (phase 6).
#[derive(Debug, Clone, Copy)]
struct Entrant {
    /// Target `LaneId`.
    lane: usize,
    pos_m: f64,
    slot: u32,
    /// Index of the target road in the route.
    route_pos: u32,
}

/// Engine-wide settings fixed at construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Length of one simulation step in milliseconds.
    ///
    /// At large `dt` a car can wait one step behind a leader that is still
    /// on the same road at the step start, even if that leader moves on
    /// this step (ADR-0004, addendum M2.1b).
    pub dt_ms: u32,
    /// Vehicles (active plus waiting) that random demand keeps on the
    /// network; 0 means no demand. Until M5.1 scenarios replace it.
    pub target_vehicles: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dt_ms: 100,
            target_vehicles: 0,
        }
    }
}

/// The engine's random streams, one per subsystem (ADR-0004).
///
/// All streams share the scenario seed and differ in the `ChaCha` stream id, so
/// a new draw in one subsystem never shifts another's sequence. The
/// discriminants are the stream ids; stream 0 is left unused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum Stream {
    /// Trip generation.
    Demand = 1,
    /// Driver behaviour noise (M4).
    Behavior = 2,
}

const STREAMS: [Stream; 2] = [Stream::Demand, Stream::Behavior];

/// A deterministic simulation instance.
///
/// Time is an integer tick count, so there is no floating-point drift in the
/// clock. All randomness must come from the per-subsystem `rngs`, seeded once at
/// construction.
#[derive(Debug, Clone)]
pub struct Engine {
    config: Config,
    tick: u64,
    /// Indexed by `Stream as usize - 1`.
    rngs: [ChaCha8Rng; 2],
    network: Network,
    vehicles: Vehicles,
    /// Per `LaneId`: how far the head may move this step without passing
    /// its leader on a later road (phase 4 writes, phase 5 reads).
    head_limit_m: Vec<f64>,
    /// Phase 6 scratch, reused every step.
    entrants: Vec<Entrant>,
    router: Router,
    demand: RandomOd,
    /// Trips that have not entered yet, a FIFO per origin road.
    pending: Vec<VecDeque<Trip>>,
    /// Total length of `pending`.
    waiting: u32,
    /// Origin road where the next step starts inserting, so the route cap
    /// doesn't always favour low road ids.
    spawn_cursor: usize,
    /// Phase 1 scratch, reused every step.
    new_trips: Vec<Trip>,
    route: Vec<u32>,
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
            rngs: STREAMS.map(|stream| {
                let mut rng = ChaCha8Rng::seed_from_u64(seed);
                rng.set_stream(stream as u64);
                rng
            }),
            vehicles: Vehicles::new(&network),
            head_limit_m: vec![
                f64::INFINITY;
                network.roads.iter().map(|r| usize::from(r.lanes)).sum()
            ],
            entrants: Vec::new(),
            router: Router::new(&network),
            demand: RandomOd {
                target_vehicles: config.target_vehicles,
            },
            // Reserved so short queues need no allocation after construction.
            pending: (0..network.roads.len())
                .map(|_| VecDeque::with_capacity(4))
                .collect(),
            waiting: 0,
            spawn_cursor: 0,
            new_trips: Vec::new(),
            route: Vec::new(),
            network,
        }
    }

    /// Advances the simulation by `steps` fixed timesteps.
    pub fn step(&mut self, steps: u64) {
        for _ in 0..steps {
            self.spawn_trips();
            // 2. Route updates: none in M2 (M5.5).
            // 3. Intersection decisions: none until M2.5b, so there is no
            //    stop line and every head may pass its road end.
            self.accelerations();
            self.integrate();
            self.transfer();
            self.arrive();
            self.tick += 1;
        }
    }

    /// Inserts a vehicle at offset 0 of lane 0 of `route[0]`, now, if the
    /// gap to that lane's tail allows it (ADR-0004 §2, spawn); returns
    /// `None` otherwise. Demand (M2.4) uses the same rule.
    ///
    /// # Panics
    /// If `route` is empty or names a road not in the network.
    pub fn spawn(&mut self, route: &[u32], params: IdmParams) -> Option<VehicleId> {
        let v = self.entry_speed(route[0], &params)?;
        Some(self.vehicles.spawn(entering(route, params, v, self.tick)))
    }

    /// Trips generated but not yet on the network.
    #[must_use]
    pub fn waiting(&self) -> u32 {
        self.waiting
    }

    /// Speed `min(v0, v_tail)` for a vehicle entering `road` at offset 0, or
    /// `None` if the gap to the lane's tail is below `s0 + v·T`.
    fn entry_speed(&self, road: u32, params: &IdmParams) -> Option<f64> {
        let v0 = params.desired_speed_factor * self.network.roads[road as usize].speed_limit_mps;
        let vs = &self.vehicles;
        let Some(&t) = vs.lane(vs.lane_id(road, 0)).back() else {
            return Some(v0);
        };
        let t = t as usize;
        let v = v0.min(vs.speed_mps[t]);
        let gap = vs.pos_m[t] - vs.params[t].length_m;
        (gap >= params.min_gap_m + v * params.time_headway_s).then_some(v)
    }

    /// Phase 1: new trips join their origin's FIFO; then, origin by origin
    /// from the cursor, each FIFO head is routed and inserted if the entry
    /// gap allows, up to `MAX_ROUTES_PER_STEP` per step.
    fn spawn_trips(&mut self) {
        let ctx = DemandContext {
            tick: self.tick,
            dt_ms: self.config.dt_ms,
            active: u32::try_from(self.vehicles.live_count()).expect("vehicle count fits u32"),
            waiting: self.waiting,
        };
        self.new_trips.clear();
        let rng = &mut self.rngs[Stream::Demand as usize - 1];
        (self.demand).spawn(&ctx, self.router.routable(), rng, &mut self.new_trips);
        for &t in &self.new_trips {
            self.pending[t.origin as usize].push_back(t);
            self.waiting += 1;
        }
        let n = self.pending.len();
        let mut routed = 0;
        for i in 0..n {
            let origin = (self.spawn_cursor + i) % n;
            let Some(&trip) = self.pending[origin].front() else {
                continue;
            };
            if routed == MAX_ROUTES_PER_STEP {
                self.spawn_cursor = origin;
                break;
            }
            let params = car();
            let road = u32::try_from(origin).expect("road id fits u32");
            let Some(v) = self.entry_speed(road, &params) else {
                continue;
            };
            routed += 1;
            self.pending[origin].pop_front();
            self.waiting -= 1;
            let found = self.router.route(&self.network, trip, &mut self.route);
            debug_assert!(found, "demand only draws routable roads");
            if found {
                let new = entering(&self.route, params, v, self.tick);
                self.vehicles.spawn(new);
            }
        }
    }

    /// The vehicle store.
    #[must_use]
    pub fn vehicles(&self) -> &Vehicles {
        &self.vehicles
    }

    /// Phase 4: IDM accelerations from start-of-step state, lane by lane.
    fn accelerations(&mut self) {
        // Taken out so the rest of the store stays readable; no allocation.
        let mut accel = std::mem::take(&mut self.vehicles.accel_mps2);
        let vs = &self.vehicles;
        let net = &self.network;
        for (q, lane) in vs.lanes.iter().enumerate() {
            let mut ahead: Option<usize> = None;
            for &s in lane {
                let s = s as usize;
                let (p, v) = (&vs.params[s], vs.speed_mps[s]);
                let road = &net.roads[vs.road(s) as usize];
                let leader = if let Some(l) = ahead {
                    Some(Leader {
                        gap_m: (vs.pos_m[l] - vs.params[l].length_m - vs.pos_m[s]).max(0.0),
                        speed_mps: vs.speed_mps[l],
                    })
                } else {
                    let leader = leader_ahead(net, vs, s, road.length_m - vs.pos_m[s]);
                    self.head_limit_m[q] = leader.map_or(f64::INFINITY, |l| vs.pos_m[s] + l.gap_m);
                    leader
                };
                let v0 = p.desired_speed_factor * road.speed_limit_mps;
                accel[s] = idm::acceleration(p, v0, v, leader).max(-MAX_DECEL_MPS2);
                ahead = Some(s);
            }
        }
        self.vehicles.accel_mps2 = accel;
    }

    /// Phase 5: ballistic integration, head to tail, never passing the leader.
    fn integrate(&mut self) {
        let dt = f64::from(self.config.dt_ms) / 1000.0;
        let vs = &mut self.vehicles;
        for (q, lane) in vs.lanes.iter().enumerate() {
            let mut limit = self.head_limit_m[q];
            for &s in lane {
                let s = s as usize;
                let (x0, v, a) = (vs.pos_m[s], vs.speed_mps[s], vs.accel_mps2[s]);
                let (x, v) = if v + a * dt < 0.0 {
                    (x0 - v * v / (2.0 * a), 0.0)
                } else {
                    (x0 + v * dt + a * dt * dt / 2.0, v + a * dt)
                };
                // `max(x0)`: a leader that cut in at a merge never pushes back.
                let x = x.min(limit).max(x0);
                debug_assert!(x.is_finite() && v.is_finite(), "state must stay finite");
                vs.pos_m[s] = x;
                vs.speed_mps[s] = v.min((x - x0) / dt);
                limit = x - vs.params[s].length_m;
            }
        }
    }

    /// Phase 6: move vehicles past their road end onto the next route road.
    fn transfer(&mut self) {
        let net = &self.network;
        let vs = &mut self.vehicles;
        self.entrants.clear();
        for lane in &vs.lanes {
            for &s in lane {
                let u = s as usize;
                let (route, mut k) = (&vs.route[u], vs.route_pos[u] as usize);
                let mut pos = vs.pos_m[u];
                if pos < net.roads[route[k] as usize].length_m {
                    break;
                }
                // Arriving vehicles stay for phase 7.
                while k + 1 < route.len() && pos >= net.roads[route[k] as usize].length_m {
                    pos -= net.roads[route[k] as usize].length_m;
                    k += 1;
                }
                if k == vs.route_pos[u] as usize {
                    continue;
                }
                self.entrants.push(Entrant {
                    lane: vs.lane_id(route[k], 0),
                    pos_m: pos,
                    slot: s,
                    route_pos: u32::try_from(k).expect("route fits u32"),
                });
            }
        }
        self.entrants.sort_unstable_by(|a, b| {
            (a.lane.cmp(&b.lane))
                .then(b.pos_m.total_cmp(&a.pos_m))
                .then(a.slot.cmp(&b.slot))
        });
        for e in &self.entrants {
            let s = e.slot as usize;
            let old_road = vs.road(s);
            let old_lane = vs.lane_id(old_road, vs.lane_of(s));
            let is_head = vs.lanes[old_lane].front() == Some(&e.slot);
            let fits = vs.lanes[e.lane]
                .back()
                .is_none_or(|&t| e.pos_m <= vs.pos_m[t as usize] - vs.params[t as usize].length_m);
            if is_head && fits {
                let road = vs.route[s][e.route_pos as usize];
                vs.transfer(vs.id(s), road, 0, e.pos_m);
                vs.route_pos[s] = e.route_pos;
            } else {
                // Blocked: hold at the road end and retry next step.
                vs.pos_m[s] = net.roads[old_road as usize].length_m;
                vs.speed_mps[s] = 0.0;
            }
        }
        // A held vehicle moved back to its road end; keep its followers behind.
        for e in &self.entrants {
            let s = e.slot as usize;
            if vs.road(s) == vs.route[s][e.route_pos as usize] {
                continue;
            }
            let q = vs.lane_id(vs.road(s), vs.lane_of(s));
            let mut front = f64::INFINITY;
            for &f in &vs.lanes[q] {
                let f = f as usize;
                if vs.pos_m[f] > front {
                    vs.pos_m[f] = front;
                    vs.speed_mps[f] = 0.0;
                }
                front = vs.pos_m[f] - vs.params[f].length_m;
            }
        }
    }

    /// Phase 7: free the slots of vehicles past their destination's end.
    fn arrive(&mut self) {
        let vs = &mut self.vehicles;
        for q in 0..vs.lanes.len() {
            while let Some(&s) = vs.lanes[q].front() {
                let s = s as usize;
                let road = vs.road(s);
                let last = vs.route_pos[s] as usize + 1 == vs.route[s].len();
                if !last || vs.pos_m[s] < self.network.roads[road as usize].length_m {
                    break;
                }
                vs.despawn(vs.id(s));
            }
        }
    }

    /// The random stream of one subsystem.
    pub fn rng(&mut self, stream: Stream) -> &mut ChaCha8Rng {
        &mut self.rngs[stream as usize - 1]
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
        h.write_u64(u64::from(self.config.target_vehicles));
        h.write_u64(self.tick);
        for (stream, rng) in STREAMS.iter().zip(&self.rngs) {
            h.write_bytes(&rng.get_seed());
            h.write_u64(*stream as u64);
            h.write_u128(rng.get_word_pos());
        }
        self.network.hash_into(&mut h);
        self.vehicles.hash_into(&mut h);
        for q in &self.pending {
            h.write_u64(q.len() as u64);
            for t in q {
                h.write_u64(u64::from(t.origin));
                h.write_u64(u64::from(t.destination));
            }
        }
        h.write_u64(self.spawn_cursor as u64);
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

/// The leader of lane head `slot`, `dist_m` before its road end: the tail of
/// the next route road that has one, within `50 m + v²/b` (ADR-0004 §2). The
/// destination's end is free road.
fn leader_ahead(net: &Network, vs: &Vehicles, slot: usize, mut dist_m: f64) -> Option<Leader> {
    let p = &vs.params[slot];
    let v = vs.speed_mps[slot];
    let horizon = LOOK_AHEAD_M + v * v / p.comfort_decel_mps2;
    let route = &vs.route[slot];
    for &r in &route[vs.route_pos[slot] as usize + 1..] {
        if dist_m > horizon {
            return None;
        }
        if let Some(&t) = vs.lane(vs.lane_id(r, 0)).back() {
            let t = t as usize;
            return Some(Leader {
                gap_m: (dist_m + vs.pos_m[t] - vs.params[t].length_m).max(0.0),
                speed_mps: vs.speed_mps[t],
            });
        }
        dist_m += net.roads[r as usize].length_m;
    }
    None
}

/// A vehicle entering `route[0]` at offset 0, lane 0, with speed `v`.
fn entering(route: &[u32], params: IdmParams, v: f64, tick: u64) -> NewVehicle<'_> {
    NewVehicle {
        road: route[0],
        lane: 0,
        pos_m: 0.0,
        speed_mps: v,
        params,
        route,
        depart_tick: tick,
    }
}
