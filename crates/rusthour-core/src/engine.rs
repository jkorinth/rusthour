//! The simulation engine: fixed timestep, seeded RNG, sim clock, and the
//! step pipeline of ADR-0004 §2.

use std::collections::VecDeque;

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;

use crate::StateHasher;
use crate::behavior::mobil::MobilParams;
use crate::behavior::profile::{Driver, Profile, Tuning};
use crate::control::priority::Priority;
use crate::control::roundabout::{self, Roundabout};
use crate::control::signal::{Light, Plan, Signal};
use crate::control::{Approach, Controller, NodeControl};
use crate::demand::{DemandContext, RandomOd, Trip};
use crate::network::lanes::Lanes;
use crate::network::{Network, movement};
use crate::routing::Router;
use crate::vehicle::class::Class;
use crate::vehicle::idm::{self, IdmParams, Leader};
use crate::vehicle::{NewVehicle, VehicleId, Vehicles};
use inspect::{Exits, LaneStop};

pub use inspect::{Gone, StopCause, WaitReason};

/// Hardest braking the integration applies (m/s²).
const MAX_DECEL_MPS2: f64 = 9.0;
/// Fixed part of a lane head's look-ahead (m); the rest is `v²/b`.
const LOOK_AHEAD_M: f64 = 50.0;
/// Most trips routed and inserted per step; the rest wait (ADR-0004 §2).
// shortcut: a fixed guess (~3× the steady-state rate at 5,000 vehicles), tune when M2.8 times routing.
const MAX_ROUTES_PER_STEP: u32 = 8;
/// Below this speed a vehicle counts as standing (m/s).
const STUCK_SPEED_MPS: f64 = 0.1;
/// Wait after which a head among equal-ranked conflicting heads is served
/// first-come, so a steady stream from the right can't starve it (M3.3); also
/// before a higher-ranked head crawling in a jam (M4.12).
/// 15 s brings the starvation back; below saturation it hardly matters
/// (ADR-0005, addendum M4.7).
const PATIENCE_MS: u64 = 10_000;
/// Speed limits above this are outside towns, where passing on the right is
/// banned (`StVO` §7; just above 50 km/h, so 13.9 m/s maps count as town).
const OUT_OF_TOWN_MPS: f64 = 14.0;
/// The ban applies to passing a leader faster than this (60 km/h).
const NO_RIGHT_PASS_MPS: f64 = 60.0 / 3.6;
/// Simulated time between updates of the routing travel-time estimates.
const COST_PERIOD_MS: u64 = 5_000;
/// Simulated time between two re-plans of the same vehicle (ADR-0004, M2.9).
const REROUTE_PERIOD_MS: u64 = 180_000;
/// Most vehicles re-planned per step, on top of `MAX_ROUTES_PER_STEP`.
// shortcut: due vehicles past the cap skip a period; never binds below 4 × 1,800 = 7,200 slots at dt = 100 ms.
const MAX_REROUTES_PER_STEP: u32 = 4;
/// A head standing this long at the line (red excepted) in a lane that
/// doesn't serve its movement re-plans from a movement its lane serves (ADR-0005 §7).
const WRONG_LANE_MS: u64 = 10_000;
/// Most such fallback re-plans per step; due heads past the cap retry next
/// step, lowest node id first.
// shortcut: like the reroute cap; spread over nodes if the trial shows starvation.
const MAX_FALLBACK_REPLANS_PER_STEP: u32 = 4;
/// `wrong_lane_ticks` of a head with no route from its lane: it goes from
/// the wrong one rather than stand until the stuck net removes it.
const NO_ROUTE: u16 = u16::MAX;
/// No lane known.
const NO_LANE: u8 = u8::MAX;

/// The M2 car, now the normal driver.
#[cfg(test)]
fn car() -> IdmParams {
    Profile::Normal.base().idm
}

/// A vehicle past its road end, waiting to enter the next road (phase 6).
#[derive(Debug, Clone, Copy)]
struct Entrant {
    /// Target `LaneId`.
    lane: usize,
    /// The entered lane of the target road.
    to_lane: u8,
    pos_m: f64,
    slot: u32,
    /// Index of the target road in the route.
    route_pos: u32,
}

/// Engine-wide settings fixed at construction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// Length of one simulation step in milliseconds.
    ///
    /// At large `dt` a car can wait one step behind a leader that is still
    /// on the same road at the step start, even if that leader moves on
    /// this step (ADR-0004, addendum M2.1b).
    pub dt_ms: u32,
    /// Vehicles (active plus waiting) that random demand keeps on the
    /// network; 0 means no demand. Until M5.5 scenarios replace it.
    pub target_vehicles: u32,
    /// A vehicle that has not moved (below 0.1 m/s) for this many simulated
    /// seconds is removed and counted in [`Engine::removed_stuck`]; 0 disables
    /// it (ADR-0004, addendum M2.10).
    pub stuck_timeout_s: u32,
    /// Relative weights of the driver profiles, in [`Profile::ALL`] order;
    /// only their ratios matter (ADR-0006 §3). Not hashed: the profiles drawn
    /// are.
    pub profile_mix: [u32; 4],
    /// Relative weights of the vehicle classes, in [`Class::ALL`] order
    /// (ADR-0007 §4). Not hashed: the classes drawn are.
    pub class_mix: [u32; 4],
    /// Each profile's parameters, in [`Profile::ALL`] order (ADR-0006, M4.4
    /// addendum). Not hashed: the drivers' parameters are.
    pub tuning: [Tuning; 4],
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dt_ms: 100,
            target_vehicles: 0,
            stuck_timeout_s: 300,
            profile_mix: crate::behavior::profile::DEFAULT_MIX,
            class_mix: crate::vehicle::class::DEFAULT_MIX,
            tuning: Profile::ALL.map(Profile::tuning),
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
    /// Lane sets and entry lanes, derived from `network`; not hashed.
    lanes: Lanes,
    vehicles: Vehicles,
    /// Per `LaneId`: how far the head may move this step without passing
    /// its leader on a later road (phase 4 writes, phase 5 reads).
    head_limit_m: Vec<f64>,
    /// Per `LaneId`: the head was told to stop at the road end (phase 3
    /// writes, phase 4 reads).
    stop: Vec<bool>,
    /// Per `LaneId`: the lane the head enters on its next road, if phase 3
    /// found it (it does for every head within its look-ahead), else
    /// `NO_LANE`; phase 4 reads it.
    head_lane: Vec<u8>,
    /// Per `LaneId`: why `stop` is set; written with it, never read by a
    /// phase, not hashed (ADR-0005, addendum M3.11).
    stop_cause: Vec<LaneStop>,
    /// The last exits, for `inspect`; not hashed.
    exits: Exits,
    /// One controller per node.
    controls: Vec<NodeControl>,
    /// Roads by end node, ascending: node `n`'s are
    /// `incoming[incoming_first[n]..incoming_first[n + 1]]`.
    incoming: Vec<u32>,
    incoming_first: Vec<u32>,
    /// Phase 3 scratch, reused every step.
    approaches: Vec<Approach>,
    go: Vec<bool>,
    /// Phase 6 scratch, reused every step.
    entrants: Vec<Entrant>,
    router: Router,
    demand: RandomOd,
    /// Trips that have not entered yet, with their driver's profile and
    /// vehicle class, a FIFO per origin road.
    pending: Vec<VecDeque<(Trip, Profile, Class)>>,
    /// Total length of `pending`.
    waiting: u32,
    /// Origin road where the next step starts inserting, so the route cap
    /// doesn't always favour low road ids.
    spawn_cursor: usize,
    /// Phase 1 scratch, reused every step.
    new_trips: Vec<Trip>,
    route: Vec<u32>,
    /// Vehicles that reached their destination so far.
    arrived: u32,
    /// Vehicles removed by the stuck-vehicle safety net so far.
    removed_stuck: u32,
    /// `removed_stuck` per road, sized once.
    stuck_by_road: Vec<u32>,
    /// `stuck_timeout_s` in ticks; 0 = off.
    stuck_limit: u32,
    /// Lane changes so far.
    lane_changes: u64,
    /// Fallback re-plans from the wrong lane so far.
    fallback_replans: u64,
    /// Phase 2b scratch, reused every step: `(LaneId, index in it, slot,
    /// target lane)`.
    lane_change_candidates: Vec<(usize, usize, usize, u8)>,
}

impl Engine {
    /// Creates an engine at time zero with an empty network.
    #[must_use]
    pub fn new(config: Config, seed: u64) -> Self {
        Self::with_network(config, seed, Network::default())
    }

    /// Creates an engine at time zero running on `network`.
    ///
    /// # Panics
    /// If the network has more than `u32::MAX` roads.
    #[must_use]
    pub fn with_network(config: Config, seed: u64, network: Network) -> Self {
        let lanes = network.roads.iter().map(|r| usize::from(r.lanes)).sum();
        let (incoming_first, incoming) = movement::incoming(&network);
        let (out_first, outgoing) = movement::outgoing(&network);
        let patience = PATIENCE_MS / u64::from(config.dt_ms).max(1);
        let controls = (0..network.nodes.len())
            .map(|n| {
                let i = &incoming[incoming_first[n] as usize..incoming_first[n + 1] as usize];
                let o = &outgoing[out_first[n] as usize..out_first[n + 1] as usize];
                match Plan::new(&network, &incoming_first, &incoming, n) {
                    Some(plan) => NodeControl::Signal(Signal::new(
                        &network,
                        plan,
                        i,
                        o,
                        patience,
                        config.dt_ms,
                    )),
                    None => match roundabout::ring_into(&network, &incoming_first, &incoming, n) {
                        Some(ring) => NodeControl::Roundabout(Roundabout::new(
                            &network,
                            &ring,
                            ring.len() - 1,
                            i,
                            o,
                            patience,
                        )),
                        None => NodeControl::Priority(Priority::new(&network, i, o, patience)),
                    },
                }
            })
            .collect();
        Self {
            config,
            tick: 0,
            rngs: STREAMS.map(|stream| {
                let mut rng = ChaCha8Rng::seed_from_u64(seed);
                rng.set_stream(stream as u64);
                rng
            }),
            vehicles: Vehicles::new(&network),
            head_limit_m: vec![f64::INFINITY; lanes],
            stop: vec![false; lanes],
            head_lane: vec![NO_LANE; lanes],
            stop_cause: vec![LaneStop::Go; lanes],
            exits: Exits::new(),
            controls,
            lanes: Lanes::new(&network),
            incoming,
            incoming_first,
            approaches: Vec::new(),
            go: Vec::new(),
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
            arrived: 0,
            removed_stuck: 0,
            stuck_by_road: vec![0; network.roads.len()],
            stuck_limit: match config.stuck_timeout_s {
                0 => 0,
                s => u32::try_from(u64::from(s) * 1000 / u64::from(config.dt_ms))
                    .unwrap_or(u32::MAX)
                    .max(1),
            },
            lane_changes: 0,
            fallback_replans: 0,
            lane_change_candidates: Vec::new(),
            network,
        }
    }

    /// Advances the simulation by `steps` fixed timesteps.
    pub fn step(&mut self, steps: u64) {
        for _ in 0..steps {
            self.spawn_trips();
            self.update_routes();
            self.change_lanes();
            self.decide();
            self.redraw_noise();
            self.accelerations();
            self.integrate();
            self.transfer();
            self.arrive();
            self.tick += 1;
        }
    }

    /// Inserts a normal driver in a car with IDM `params` at offset 0 of lane 0 of
    /// `route[0]`, now, if the gap to that lane's tail allows it (ADR-0004 §2,
    /// spawn); returns `None` otherwise. Demand (M2.4) uses the same rule.
    ///
    /// # Panics
    /// If `route` is empty or names a road not in the network.
    pub fn spawn(&mut self, route: &[u32], params: IdmParams) -> Option<VehicleId> {
        let v = self.entry_speed(route[0], &params)?;
        let driver = Driver {
            idm: params,
            mobil: MobilParams::default(),
            gap: Profile::Normal.base().gap,
        };
        let new = entering(route, driver, (Profile::Normal, Class::Car), v, self.tick);
        Some(self.vehicles.spawn(new))
    }

    /// Inserts a car with a driver of `profile`, tuned as the run's config
    /// says, as [`Engine::spawn`] does; an erratic driver draws its jitter
    /// from the Behavior stream. For tests and studies of driver mixes.
    ///
    /// # Panics
    /// If `route` is empty or names a road not in the network.
    pub fn spawn_profile(&mut self, route: &[u32], profile: Profile) -> Option<VehicleId> {
        let tuning = self.config.tuning[profile as usize];
        let v = self.entry_speed(route[0], &profile.base_with(&tuning).idm)?;
        let rng = &mut self.rngs[Stream::Behavior as usize - 1];
        let driver = Class::Car.apply(profile.driver_with(&tuning, rng));
        let new = entering(route, driver, (profile, Class::Car), v, self.tick);
        Some(self.vehicles.spawn(new))
    }

    /// Trips generated but not yet on the network.
    #[must_use]
    pub fn waiting(&self) -> u32 {
        self.waiting
    }

    /// Speed `min(v0, v_tail)` for a vehicle entering `road` at offset 0, or
    /// `None` if the gap to the lane's tail is below `s0 + v·T`.
    fn entry_speed(&self, road: u32, params: &IdmParams) -> Option<f64> {
        let v0 = params.v0(self.network.roads[road as usize].speed_limit_mps);
        let vs = &self.vehicles;
        let Some(&t) = vs.lane(vs.lane_id(road, 0)).back() else {
            return Some(v0);
        };
        let t = t as usize;
        let v = v0.min(vs.speed_mps[t]);
        let gap = vs.pos_m[t] - vs.params[t].length_m;
        (gap >= params.min_gap_m + v * params.time_headway_s).then_some(v)
    }

    /// Whether a vehicle `length_m` long, entering lane 0 of `road` at its
    /// start, leaves a lane head about to enter that lane from the node
    /// behind less than `s0 + v·T`: traffic entering from outside the
    /// network yields (`StVO` §10, M4.12).
    // shortcut: looks at lane heads only, not at the vehicles behind them.
    fn cuts_in(&self, road: u32, length_m: f64) -> bool {
        let (net, vs) = (&self.network, &self.vehicles);
        let n = net.roads[road as usize].from as usize;
        let first = self.incoming_first[n] as usize..self.incoming_first[n + 1] as usize;
        self.incoming[first].iter().any(|&r| {
            (0..net.roads[r as usize].lanes).any(|lane| {
                vs.lane(vs.lane_id(r, lane)).front().is_some_and(|&h| {
                    let h = h as usize;
                    let (p, v) = (&vs.params[h], vs.speed_mps[h]);
                    let gap = net.roads[r as usize].length_m - vs.pos_m[h] - length_m;
                    vs.route[h].get(vs.route_pos[h] as usize + 1) == Some(&road)
                        && next_lane(net, &self.lanes, vs, h, r, lane) == 0
                        && gap < p.min_gap_m + v * p.time_headway_s
                })
            })
        })
    }

    /// Phase 1: new trips draw a driver profile (ADR-0006) and a vehicle
    /// class (ADR-0007) and join their
    /// origin's FIFO; then, origin by origin
    /// from the cursor, each FIFO head is routed and inserted if the entry
    /// gap allows and it doesn't cut in front of a vehicle about to enter its
    /// road, up to `MAX_ROUTES_PER_STEP` per step.
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
        let behavior = &mut self.rngs[Stream::Behavior as usize - 1];
        for &t in &self.new_trips {
            let profile = Profile::draw(behavior, &self.config.profile_mix);
            let class = Class::draw(behavior, &self.config.class_mix);
            self.pending[t.origin as usize].push_back((t, profile, class));
            self.waiting += 1;
        }
        let n = self.pending.len();
        let mut routed = 0;
        for i in 0..n {
            let origin = (self.spawn_cursor + i) % n;
            let Some(&(trip, profile, class)) = self.pending[origin].front() else {
                continue;
            };
            if routed == MAX_ROUTES_PER_STEP {
                self.spawn_cursor = origin;
                break;
            }
            let road = u32::try_from(origin).expect("road id fits u32");
            let tuning = self.config.tuning[profile as usize];
            let idm = class.apply(profile.base_with(&tuning)).idm;
            let Some(v) = self.entry_speed(road, &idm) else {
                continue;
            };
            if self.cuts_in(road, idm.length_m) {
                continue;
            }
            routed += 1;
            self.pending[origin].pop_front();
            self.waiting -= 1;
            let found = self.router.route(trip, &mut self.route);
            debug_assert!(found, "demand only draws routable roads");
            if found {
                let rng = &mut self.rngs[Stream::Behavior as usize - 1];
                let driver = profile.driver_with(&tuning, rng);
                let new = entering(
                    &self.route,
                    class.apply(driver),
                    (profile, class),
                    v,
                    self.tick,
                );
                self.vehicles.spawn(new);
            }
        }
    }

    /// Phase 2: refresh the travel-time and movement-delay estimates every
    /// `COST_PERIOD_MS`; then each vehicle whose turn it is re-plans from its
    /// current road to its destination, keeping that road. Turns are
    /// staggered by slot, so a step re-plans at most `ceil(slots / period)`
    /// vehicles. Returns how
    /// many it re-planned.
    fn update_routes(&mut self) -> u32 {
        let dt = u64::from(self.config.dt_ms);
        if self.tick.is_multiple_of((COST_PERIOD_MS / dt).max(1)) {
            self.router.observe(
                &self.network,
                &self.vehicles,
                f64::from(self.config.dt_ms) / 1000.0,
            );
        }
        let period = (REROUTE_PERIOD_MS / dt).max(1);
        let vs = &mut self.vehicles;
        let mut n = 0;
        // The slots with `(tick + slot) % period == 0`.
        let first = (period - self.tick % period) % period;
        for s in
            (first..vs.slot_count() as u64).step_by(usize::try_from(period).unwrap_or(usize::MAX))
        {
            if n == MAX_REROUTES_PER_STEP {
                break;
            }
            let s = usize::try_from(s).expect("slot fits usize");
            let k = vs.route_pos[s] as usize;
            if k + 1 >= vs.route[s].len() {
                continue; // free slot (empty route) or on its destination
            }
            let (net, lanes) = (&self.network, &self.lanes);
            let (r, lane) = (vs.road(s), vs.lane_of(s));
            let dest = vs.route[s][vs.route[s].len() - 1];
            n += 1;
            // Near the line, only the movements its lane serves (ADR-0005, M3.12).
            let road = &net.roads[r as usize];
            let found = if road.lanes > 1 && road.length_m - vs.pos_m[s] < route_zone_m(vs, s) {
                let serves = |to: u32| lanes.from_mask(net, r, to) >> lane & 1 == 1;
                self.router.route_from_any(r, serves, dest, &mut self.route)
            } else {
                let trip = Trip {
                    origin: r,
                    destination: dest,
                };
                self.router.route(trip, &mut self.route)
            };
            let route = &mut vs.route[s];
            if found {
                // Drop the roads already driven, so a detour needs less room.
                route.clear();
                route.extend_from_slice(&self.route);
                vs.route_pos[s] = 0;
            }
        }
        n
    }

    /// The vehicle store.
    #[must_use]
    pub fn vehicles(&self) -> &Vehicles {
        &self.vehicles
    }

    /// Phase 3: each node's controller decides for the lane heads within
    /// their look-ahead of its end; heads told to stop get a stop line. A
    /// head whose lane doesn't serve its movement stops without competing,
    /// and after `WRONG_LANE_MS` standing re-plans from its lane (ADR-0005 §7).
    fn decide(&mut self) {
        self.stop.fill(false);
        self.head_lane.fill(NO_LANE);
        self.stop_cause.fill(LaneStop::Go);
        let (net, lanes, vs) = (&self.network, &self.lanes, &mut self.vehicles);
        let wrong_lane_ticks = WRONG_LANE_MS / u64::from(self.config.dt_ms);
        let mut replans = 0;
        for (n, control) in self.controls.iter_mut().enumerate() {
            let roads = self.incoming_first[n] as usize..self.incoming_first[n + 1] as usize;
            self.approaches.clear();
            for &r in &self.incoming[roads] {
                for lane in 0..net.roads[r as usize].lanes {
                    let Some(&s) = vs.lane(vs.lane_id(r, lane)).front() else {
                        continue;
                    };
                    let u = s as usize;
                    // The destination's end is no node passage (it arrives there).
                    let Some(&to_road) = vs.route[u].get(vs.route_pos[u] as usize + 1) else {
                        continue;
                    };
                    let (p, v) = (&vs.params[u], vs.speed_mps[u]);
                    let dist_m = net.roads[r as usize].length_m - vs.pos_m[u];
                    if dist_m > horizon(p, v) {
                        continue;
                    }
                    let to_lane = next_lane(net, lanes, vs, u, r, lane);
                    self.head_lane[vs.lane_id(r, lane)] = to_lane;
                    let mut ap = Approach {
                        from_road: r,
                        lane,
                        to_road,
                        to_lane,
                        slot: s,
                        dist_m,
                        speed_mps: v,
                        min_gap_m: p.min_gap_m,
                        can_stop: dist_m >= v * v / (2.0 * p.comfort_decel_mps2),
                        blocked: blocked(net, vs, u, r, to_lane),
                    };
                    let serves = |to: u32| lanes.from_mask(net, r, to) >> lane & 1 == 1;
                    if !serves(to_road) {
                        let lane_id = vs.lane_id(r, lane);
                        self.stop[lane_id] = true;
                        if control.holds_at_red(&ap, self.tick) {
                            self.stop_cause[lane_id] = LaneStop::Red;
                            continue;
                        }
                        let waited = &mut vs.wrong_lane_ticks[u];
                        if *waited != NO_ROUTE {
                            if v < STUCK_SPEED_MPS {
                                *waited = (*waited + 1).min(NO_ROUTE - 1);
                            }
                            if u64::from(*waited) < wrong_lane_ticks
                                || replans == MAX_FALLBACK_REPLANS_PER_STEP
                            {
                                self.stop_cause[lane_id] = LaneStop::WrongLane;
                                continue;
                            }
                            replans += 1;
                            self.fallback_replans += 1;
                            let route = &mut vs.route[u];
                            let dest = route[route.len() - 1];
                            if self.router.route_from_any(r, serves, dest, &mut self.route) {
                                *waited = 0;
                                route.clear();
                                route.extend_from_slice(&self.route);
                                vs.route_pos[u] = 0;
                                ap.to_road = route[1];
                                ap.to_lane = next_lane(net, lanes, vs, u, r, lane);
                                self.head_lane[vs.lane_id(r, lane)] = ap.to_lane;
                                ap.blocked = blocked(net, vs, u, r, ap.to_lane);
                            } else {
                                *waited = NO_ROUTE;
                            }
                        }
                        self.stop[lane_id] = false;
                    }
                    self.approaches.push(ap);
                }
            }
            self.go.clear();
            self.go.resize(self.approaches.len(), false);
            control.decide(net, vs, self.tick, &self.approaches, &mut self.go);
            for (a, &go) in self.approaches.iter().zip(&self.go) {
                let q = vs.lane_id(a.from_road, a.lane);
                self.stop[q] = !go;
                if !go {
                    self.stop_cause[q] = if control.holds_at_red(a, self.tick) {
                        LaneStop::Red
                    } else if a.blocked {
                        LaneStop::NoRoom
                    } else if control.holds_at_stop_sign(a) {
                        LaneStop::Stop
                    } else {
                        LaneStop::Yield
                    };
                    vs.held_ticks[a.slot as usize] += 1;
                }
            }
        }
    }

    /// Phase 3b: each vehicle whose second it is (`(tick + slot) % (1000 /
    /// dt_ms) == 0`, as MOBIL) redraws its acceleration noise, in slot
    /// order; only erratic drivers draw (ADR-0006 §4).
    fn redraw_noise(&mut self) {
        let period = (1000 / u64::from(self.config.dt_ms)).max(1);
        let first = (period - self.tick % period) % period;
        let (vs, rng) = (
            &mut self.vehicles,
            &mut self.rngs[Stream::Behavior as usize - 1],
        );
        let step = usize::try_from(period).expect("period fits usize");
        for s in (first..vs.slot_count() as u64).step_by(step) {
            let s = usize::try_from(s).expect("slot fits usize");
            vs.noise_mps2[s] = vs.profile[s].noise_mps2(rng);
        }
    }

    /// Phase 4: IDM accelerations from start-of-step state, lane by lane.
    /// Outside towns a vehicle doesn't pass a slower leader in the lane to
    /// its left on the right (ADR-0005 §6). No vehicle exceeds its class's
    /// speed cap (ADR-0007 §3).
    fn accelerations(&mut self) {
        let dt = f64::from(self.config.dt_ms) / 1000.0;
        // Taken out so the rest of the store stays readable; no allocation.
        let mut accel = std::mem::take(&mut self.vehicles.accel_mps2);
        let vs = &self.vehicles;
        let (net, lanes) = (&self.network, &self.lanes);
        for (q, lane) in vs.lanes.iter().enumerate() {
            // Every vehicle of a lane is on the same road and lane.
            let Some(&head) = lane.front() else {
                continue;
            };
            let (road, ix) = (
                &net.roads[vs.road(head as usize) as usize],
                vs.lane_of(head as usize),
            );
            // The leader's rear and speed.
            let mut ahead: Option<(f64, f64)> = None;
            // The lane to the left on a road outside towns, and how many of
            // its vehicles have their rear ahead of the current one.
            let has_left = ix + 1 < road.lanes;
            let left = (has_left && road.speed_limit_mps > OUT_OF_TOWN_MPS).then(|| vs.lane(q + 1));
            let mut passed = 0;
            // The lanes to either side on a multi-lane road.
            let sides = [
                (ix > 0).then(|| vs.lane(q - 1)),
                has_left.then(|| vs.lane(q + 1)),
            ];
            let mut side_ahead = [(0, None); 2];
            for &s in lane {
                let s = s as usize;
                let (p, v, pos) = (&vs.params[s], vs.speed_mps[s], vs.pos_m[s]);
                let leader = if let Some((rear, speed_mps)) = ahead {
                    Some(Leader {
                        gap_m: (rear - pos).max(0.0),
                        speed_mps,
                    })
                } else {
                    let dist_m = road.length_m - pos;
                    let entered = (self.head_lane[q] != NO_LANE).then_some(self.head_lane[q]);
                    let mut leader =
                        leader_ahead(net, lanes, vs, s, (ix, entered), dist_m).map(|(l, _)| l);
                    let mut limit = leader.map_or(f64::INFINITY, |l| pos + l.gap_m);
                    if self.stop[q] {
                        // Stop line: a standing obstacle at the road end, unless
                        // the next road's tail still sticks out nearer. The
                        // clamp keeps the head `s0` short of it, never across.
                        limit = limit.min(road.length_m - p.min_gap_m);
                        if leader.is_none_or(|l| l.gap_m >= dist_m) {
                            leader = Some(Leader {
                                gap_m: dist_m,
                                speed_mps: 0.0,
                            });
                        }
                    }
                    self.head_limit_m[q] = limit;
                    leader
                };
                let v0 = p.v0(road.speed_limit_mps);
                let mut a = idm::acceleration(p, v0, v, leader) + vs.noise_mps2[s];
                if let Some(left) = left {
                    a = a.min(right_pass_cap(vs, left, &mut passed, s, v0));
                }
                // Let in a car ahead beside that must merge into this lane (M3.12).
                for (side, state) in sides.iter().zip(&mut side_ahead) {
                    if let &Some(side) = side {
                        a = a.min(merge_cap(net, lanes, vs, side, state, s, v0));
                    }
                }
                // Speed governor: noise can't push a truck past its cap.
                accel[s] = a.min((p.max_speed_mps - v) / dt).max(-MAX_DECEL_MPS2);
                ahead = Some((pos - p.length_m, v));
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
        let (net, lanes) = (&self.network, &self.lanes);
        let vs = &mut self.vehicles;
        self.entrants.clear();
        for lane in &vs.lanes {
            for &s in lane {
                let u = s as usize;
                let mut pos = vs.pos_m[u];
                // `road(u)`, not the route: most heads stay, and this skips a load.
                if pos < net.roads[vs.road(u) as usize].length_m {
                    break;
                }
                let (route, mut k) = (&vs.route[u], vs.route_pos[u] as usize);
                // Arriving vehicles stay for phase 7.
                let mut lane = vs.lane_of(u);
                while k + 1 < route.len() && pos >= net.roads[route[k] as usize].length_m {
                    pos -= net.roads[route[k] as usize].length_m;
                    let after = route.get(k + 2).copied();
                    lane = lanes.entry(net, route[k], lane, route[k + 1], after);
                    k += 1;
                }
                if k == vs.route_pos[u] as usize {
                    continue;
                }
                self.entrants.push(Entrant {
                    lane: vs.lane_id(route[k], lane),
                    to_lane: lane,
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
                // The delay sample of the movement it makes (ADR-0005 §2).
                let to = vs.route[s][vs.route_pos[s] as usize + 1];
                self.router.passed(old_road, to, vs.held_ticks[s]);
                vs.held_ticks[s] = 0;
                vs.wrong_lane_ticks[s] = 0;
                let road = vs.route[s][e.route_pos as usize];
                vs.transfer(vs.id(s), road, e.to_lane, e.pos_m);
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
                let (id, length_m) = (vs.id(s), self.network.roads[road as usize].length_m);
                let gone = Gone::Arrived {
                    road,
                    offset_m: vs.pos_m[s].min(length_m),
                };
                vs.despawn(id);
                self.exits.push(id, gone);
                self.arrived += 1;
            }
        }
        // Safety net: gridlock can still happen (ADR-0004, M2.10). Slot order.
        for s in 0..vs.slot_count() {
            if !vs.is_live(s) {
                continue;
            }
            if vs.speed_mps[s] >= STUCK_SPEED_MPS {
                vs.stuck_ticks[s] = 0;
                continue;
            }
            vs.stuck_ticks[s] += 1;
            if self.stuck_limit > 0 && vs.stuck_ticks[s] >= self.stuck_limit {
                self.stuck_by_road[vs.road(s) as usize] += 1;
                let id = vs.id(s);
                let gone = Gone::RemovedStuck {
                    road: vs.road(s),
                    offset_m: vs.pos_m[s],
                };
                vs.despawn(id);
                self.exits.push(id, gone);
                self.removed_stuck += 1;
            }
        }
    }

    /// Vehicles removed so far for standing still longer than
    /// `Config::stuck_timeout_s`. A symptom of gridlock, not a result.
    #[must_use]
    pub fn removed_stuck(&self) -> u32 {
        self.removed_stuck
    }

    /// Each signal arm's `(road, light, seconds to the next change)`, as the
    /// next step applies them, by node.
    pub fn signals(&self) -> impl Iterator<Item = (u32, Light, f64)> + '_ {
        (self.controls.iter())
            .filter_map(|c| {
                let NodeControl::Signal(s) = c else {
                    return None;
                };
                Some(s.lights(self.tick))
            })
            .flatten()
    }

    /// Vehicles that reached the end of their route so far.
    #[must_use]
    pub fn arrived(&self) -> u32 {
        self.arrived
    }

    /// [`Self::removed_stuck`] per road, indexed by road id.
    #[must_use]
    pub fn stuck_by_road(&self) -> &[u32] {
        &self.stuck_by_road
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
        h.write_u64(u64::from(self.config.stuck_timeout_s));
        h.write_u64(u64::from(self.arrived));
        h.write_u64(u64::from(self.removed_stuck));
        h.write_u64(self.lane_changes);
        h.write_u64(self.fallback_replans);
        for &n in &self.stuck_by_road {
            h.write_u64(u64::from(n));
        }
        h.write_u64(self.tick);
        for (stream, rng) in STREAMS.iter().zip(&self.rngs) {
            h.write_bytes(&rng.get_seed());
            h.write_u64(*stream as u64);
            h.write_u128(rng.get_word_pos());
        }
        self.network.hash_into(&mut h);
        self.vehicles.hash_into(&mut h);
        for c in &self.controls {
            c.hash_into(&mut h);
        }
        for q in &self.pending {
            h.write_u64(q.len() as u64);
            for (t, p, c) in q {
                h.write_u64(u64::from(t.origin));
                h.write_u64(u64::from(t.destination));
                h.write_u64(*p as u64);
                h.write_u64(*c as u64);
            }
        }
        h.write_u64(self.spawn_cursor as u64);
        self.router.hash_into(&mut h);
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

/// The leader of lane head `slot`, `dist_m` before its road end in `lane`
/// (its own, or one it weighs changing to), and its slot:
/// the tail of the next route road that has one, within `50 m + v²/b`
/// (ADR-0004 §2). The destination's end is free road. `entered` is the lane it
/// enters on its next road, if already known.
fn leader_ahead(
    net: &Network,
    lanes: &Lanes,
    vs: &Vehicles,
    slot: usize,
    (mut lane, entered): (u8, Option<u8>),
    mut dist_m: f64,
) -> Option<(Leader, u32)> {
    let horizon = horizon(&vs.params[slot], vs.speed_mps[slot]);
    let route = &vs.route[slot];
    let k = vs.route_pos[slot] as usize;
    let mut from = route[k];
    for (i, &r) in route[k + 1..].iter().enumerate() {
        if dist_m > horizon {
            return None;
        }
        let after = route.get(k + 2 + i).copied();
        lane = match entered {
            Some(e) if i == 0 => {
                debug_assert_eq!(e, lanes.entry(net, from, lane, r, after));
                e
            }
            _ => lanes.entry(net, from, lane, r, after),
        };
        from = r;
        if let Some(&slot_t) = vs.lane(vs.lane_id(r, lane)).back() {
            let t = slot_t as usize;
            let leader = Leader {
                gap_m: (dist_m + vs.pos_m[t] - vs.params[t].length_m).max(0.0),
                speed_mps: vs.speed_mps[t],
            };
            return Some((leader, slot_t));
        }
        dist_m += net.roads[r as usize].length_m;
    }
    None
}

/// The lane of its next road that `slot`, on `lane` of `from`, enters.
fn next_lane(net: &Network, lanes: &Lanes, vs: &Vehicles, slot: usize, from: u32, lane: u8) -> u8 {
    let (route, k) = (&vs.route[slot], vs.route_pos[slot] as usize + 1);
    lanes.entry(net, from, lane, route[k], route.get(k + 1).copied())
}

/// Whether lane head `slot` on `road` waits for room in `to_lane` of its
/// next road; circulating vehicles keep priority on the ring (ADR-0005 §4).
fn blocked(net: &Network, vs: &Vehicles, slot: usize, road: u32, to_lane: u8) -> bool {
    let to_road = vs.route[slot][vs.route_pos[slot] as usize + 1];
    no_room(net, vs, slot, to_lane)
        && !(net.roads[road as usize].roundabout && net.roads[to_road as usize].roundabout)
}

/// Don't block the box: the lane `to_lane` of lane head `slot`'s next road
/// will have no room for the whole vehicle when it gets there. The tail's
/// rear counts as driving on while the head covers the distance to the line,
/// in proportion to the two speeds and at most as far (M4.12); a standing
/// tail must have left the room already (M4.14).
fn no_room(net: &Network, vs: &Vehicles, slot: usize, to_lane: u8) -> bool {
    let Some(&to_road) = vs.route[slot].get(vs.route_pos[slot] as usize + 1) else {
        return false;
    };
    vs.lane(vs.lane_id(to_road, to_lane))
        .back()
        .is_some_and(|&t| {
            let t = t as usize;
            let (v, vt) = (vs.speed_mps[slot], vs.speed_mps[t]);
            let dist_m = net.roads[vs.road(slot) as usize].length_m - vs.pos_m[slot];
            // vt / v is +inf for a standing head, so min gives dist_m.
            let ahead_m = if vt < STUCK_SPEED_MPS {
                0.0
            } else {
                dist_m * (vt / v).min(1.0)
            };
            vs.pos_m[t] - vs.params[t].length_m + ahead_m < vs.params[slot].length_m
        })
}

/// Phase 4's cap for `slot` outside towns: its IDM acceleration behind the
/// nearest vehicle in the lane to its left with its rear ahead, if it must
/// not pass that one on the right; else infinity. `passed` counts the
/// vehicles of `left` ahead and only grows down a lane, so a lane costs
/// O(both lanes).
fn right_pass_cap(
    vs: &Vehicles,
    left: &VecDeque<u32>,
    passed: &mut usize,
    slot: usize,
    v0: f64,
) -> f64 {
    let x = vs.pos_m[slot];
    let rear = |o: usize| vs.pos_m[o] - vs.params[o].length_m;
    while left.get(*passed).is_some_and(|&o| rear(o as usize) > x) {
        *passed += 1;
    }
    let Some(l) = passed.checked_sub(1).map(|k| left[k] as usize) else {
        return f64::INFINITY;
    };
    let (lv, v) = (vs.speed_mps[l], vs.speed_mps[slot]);
    if !no_right_pass(lv, v) {
        return f64::INFINITY;
    }
    let leader = Leader {
        gap_m: rear(l) - x,
        speed_mps: lv,
    };
    idm::acceleration(&vs.params[slot], v0, v, Some(leader))
}

/// Whether, outside towns, a vehicle at `v` must not pass a leader in the
/// lane to its left driving `leader_v`: one faster than 60 km/h and slower
/// than itself. Slower traffic, such as a queue, may be passed (`StVO` §7(3)).
fn no_right_pass(leader_v: f64, v: f64) -> bool {
    leader_v > NO_RIGHT_PASS_MPS && leader_v < v
}

/// How far ahead a vehicle at speed `v` looks: `50 m + v²/b`.
fn horizon(p: &IdmParams, v: f64) -> f64 {
    LOOK_AHEAD_M + v * v / p.comfort_decel_mps2
}

/// A vehicle entering `route[0]` at offset 0, lane 0, with speed `v`.
fn entering(
    route: &[u32],
    d: Driver,
    (profile, class): (Profile, Class),
    v: f64,
    tick: u64,
) -> NewVehicle<'_> {
    NewVehicle {
        road: route[0],
        lane: 0,
        pos_m: 0.0,
        speed_mps: v,
        params: d.idm,
        mobil: d.mobil,
        gap: d.gap,
        profile,
        class,
        route,
        depart_tick: tick,
    }
}

mod inspect;
mod lane_change;
use lane_change::{merge_cap, route_zone_m};

#[cfg(test)]
mod tests;
