//! Route finding over the network: A*, later contraction hierarchies (M2, M5).
//!
//! The road graph has roads as vertices and `connections` (movements) as
//! edges, so turn restrictions are honored by construction. The cost of a
//! route is the sum of each road's current travel-time estimate, origin and
//! destination included: `length / speed limit` on an empty road,
//! `length / mean speed` of its vehicles otherwise ([`Router::observe`],
//! M2.9); plus, per movement, a static turn penalty and the observed delay
//! at the stop line (ADR-0005 §2). Both are non-negative and the heuristic
//! ignores them, so A* stays exact.
//! M2 traffic stays inside the largest strongly connected component (SCC), so
//! every origin–destination pair taken from [`Router::routable`] has a route
//! (ADR-0004).

use std::collections::BinaryHeap;

use crate::StateHasher;
use crate::control::priority::rank;
use crate::demand::Trip;
use crate::network::Network;
use crate::network::movement::{self, Turn};
use crate::vehicle::Vehicles;

const UNSEEN: u32 = u32::MAX;
/// Floor of the mean speed in the travel-time estimate (m/s). Low on
/// purpose: a road whose cars all stand gets a near-prohibitive cost, which
/// keeps routes out of forming gridlocks (ADR-0004, addendum M2.9).
const MIN_SPEED_MPS: f64 = 0.01;
/// Weight of a new observation in the travel-time and delay estimates (EMAs).
const SMOOTHING: f64 = 0.2;
/// Delay prior of a movement whose road ranks below another at the node (s).
const YIELD_PRIOR_S: f64 = 4.0;

/// Static cost of a turn (s).
// shortcut: guessed values from ADR-0005, tune in the M3 trial.
fn turn_s(turn: Turn) -> f64 {
    match turn {
        Turn::Straight => 0.0,
        Turn::Right => 2.0,
        Turn::Left => 4.0,
        Turn::LeftAcross => 8.0,
        Turn::UTurn => 20.0,
    }
}

/// A* router with scratch buffers reused across calls (no allocation per
/// route after warm-up). Must be used with the network it was built from.
#[derive(Debug, Clone)]
pub struct Router {
    /// CSR adjacency: successors of road `r` are `succ[first[r]..first[r + 1]]`.
    first: Vec<u32>,
    succ: Vec<u32>,
    /// Per movement (parallel to `succ`): `turn_s + delay_s`, what A* adds
    /// on top of the next road's travel time.
    move_s: Vec<f64>,
    /// Per movement: static turn penalty (s).
    turn_s: Vec<f64>,
    /// Per movement: what `delay_s` decays to without samples (s).
    prior_s: Vec<f64>,
    /// Per movement: smoothed time heads stand held at the line (s).
    delay_s: Vec<f64>,
    /// Per movement: held ticks and count of passages since the last `observe`.
    held_sum: Vec<u64>,
    held_n: Vec<u32>,
    /// `observe` scratch: per movement, the longest wait of a current head (s).
    head_s: Vec<f64>,
    routable: Vec<u32>,
    /// Per road: travel time at the speed limit (s).
    free_s: Vec<f64>,
    /// Per road: smoothed current travel time (s), never below `free_s`, so
    /// the free-flow heuristic stays admissible.
    travel_s: Vec<f64>,
    /// Seconds per meter of straight-line distance that no route can beat.
    h_per_m: f64,
    cost: Vec<f64>,
    prev: Vec<u32>,
    /// `epoch` at which `cost`/`prev` of a road were last written.
    stamp: Vec<u32>,
    epoch: u32,
    heap: BinaryHeap<Entry>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    f: f64,
    g: f64,
    road: u32,
}

impl Eq for Entry {}

impl Ord for Entry {
    /// Reversed, so the max-heap pops the lowest `f`, ties to the lowest road id.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.f.total_cmp(&self.f).then(other.road.cmp(&self.road))
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Router {
    /// Builds the road graph and finds its largest SCC (ties go to the
    /// component with the lowest road id).
    ///
    /// # Panics
    ///
    /// If the network has more than `u32::MAX` roads.
    #[must_use]
    pub fn new(net: &Network) -> Self {
        let n = net.roads.len();
        let mut first = vec![0u32; n + 1];
        for &(from, _) in &net.connections {
            first[from as usize + 1] += 1;
        }
        for i in 0..n {
            first[i + 1] += first[i];
        }
        let mut fill = first.clone();
        let mut succ = vec![0u32; net.connections.len()];
        for &(from, to) in &net.connections {
            succ[fill[from as usize] as usize] = to;
            fill[from as usize] += 1;
        }
        // Turn penalties and yield priors, per movement.
        let (in_first, incoming) = movement::incoming(net);
        let mut turn = vec![0.0; succ.len()];
        let mut prior = vec![0.0; succ.len()];
        for from in 0..n {
            let node = net.roads[from].to as usize;
            let arms = &incoming[in_first[node] as usize..in_first[node + 1] as usize];
            let from = u32::try_from(from).expect("road count fits u32");
            let mine = rank(net, from);
            let yields = arms.iter().any(|&r| rank(net, r) > mine);
            for i in first[from as usize] as usize..first[from as usize + 1] as usize {
                turn[i] = turn_s(Turn::of(net, from, succ[i], arms));
                prior[i] = if yields { YIELD_PRIOR_S } else { 0.0 };
            }
        }
        let in_scc = largest_scc(&first, &succ);
        let routable = (0..n)
            .filter(|&r| in_scc[r])
            .map(|r| u32::try_from(r).expect("road count fits u32"))
            .collect();

        // Scale the straight-line bound down if a road is shorter than the
        // distance between its nodes, so the heuristic stays admissible.
        let mut scale = 1.0_f64;
        let mut vmax = 0.0_f64;
        for r in &net.roads {
            let d = dist(net.nodes[r.from as usize], net.nodes[r.to as usize]);
            if d > 0.0 {
                scale = scale.min(r.length_m / d);
            }
            vmax = vmax.max(r.speed_limit_mps);
        }
        let h_per_m = if vmax > 0.0 { scale / vmax } else { 0.0 };
        let free_s: Vec<f64> = (net.roads.iter())
            .map(|r| r.length_m / r.speed_limit_mps)
            .collect();

        let m = succ.len();
        Self {
            move_s: turn.iter().zip(&prior).map(|(t, p)| t + p).collect(),
            turn_s: turn,
            delay_s: prior.clone(),
            prior_s: prior,
            held_sum: vec![0; m],
            held_n: vec![0; m],
            head_s: vec![0.0; m],
            first,
            succ,
            routable,
            travel_s: free_s.clone(),
            free_s,
            h_per_m,
            cost: vec![0.0; n],
            prev: vec![0; n],
            stamp: vec![0; n],
            epoch: 0,
            heap: BinaryHeap::new(),
        }
    }

    /// Roads in the largest SCC, ascending.
    #[must_use]
    pub fn routable(&self) -> &[u32] {
        &self.routable
    }

    /// Number of roads outside the largest SCC.
    #[must_use]
    pub fn excluded_count(&self) -> usize {
        self.stamp.len() - self.routable.len()
    }

    /// Share of roads outside the largest SCC, `0.0` for an empty network.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // road counts are far below 2^52
    pub fn excluded_share(&self) -> f64 {
        if self.stamp.is_empty() {
            0.0
        } else {
            self.excluded_count() as f64 / self.stamp.len() as f64
        }
    }

    /// Roads outside the largest SCC, ascending.
    ///
    /// # Panics
    ///
    /// If the network has more than `u32::MAX` roads.
    pub fn excluded_roads(&self) -> impl Iterator<Item = u32> + '_ {
        let mut routable = self.routable.iter().copied().peekable();
        (0..u32::try_from(self.stamp.len()).expect("road count fits u32")).filter(move |&r| {
            if routable.peek() == Some(&r) {
                routable.next();
                false
            } else {
                true
            }
        })
    }

    /// Finds the cheapest route from `trip.origin` to `trip.destination` and
    /// writes the road ids, origin to destination inclusive, into `out`
    /// (cleared first). Returns `false`, leaving `out` empty, if there is none.
    pub fn route(&mut self, net: &Network, trip: Trip, out: &mut Vec<u32>) -> bool {
        out.clear();
        self.epoch += 1;
        self.heap.clear();
        let (origin, dest) = (trip.origin, trip.destination);
        let goal = net.nodes[net.roads[dest as usize].to as usize];
        let h_per_m = self.h_per_m;
        let h = |road: u32| {
            let r = &net.roads[road as usize];
            dist(net.nodes[r.to as usize], goal) * h_per_m
        };
        let g0 = self.travel_s[origin as usize];
        self.set(origin, g0, UNSEEN);
        let f0 = g0 + h(origin);
        self.heap.push(Entry {
            f: f0,
            g: g0,
            road: origin,
        });
        while let Some(Entry { g, road, .. }) = self.heap.pop() {
            if g > self.cost[road as usize] {
                continue; // stale entry
            }
            if road == dest {
                let mut r = dest;
                while r != UNSEEN {
                    out.push(r);
                    r = self.prev[r as usize];
                }
                out.reverse();
                return true;
            }
            let (a, b) = (self.first[road as usize], self.first[road as usize + 1]);
            for i in a..b {
                let nb = self.succ[i as usize];
                let g2 = g + self.move_s[i as usize] + self.travel_s[nb as usize];
                if self.stamp[nb as usize] != self.epoch || g2 < self.cost[nb as usize] {
                    self.set(nb, g2, road);
                    self.heap.push(Entry {
                        f: g2 + h(nb),
                        g: g2,
                        road: nb,
                    });
                }
            }
        }
        false
    }

    /// Records a vehicle passing from road `from` to road `to` after it was
    /// held at the line for `held_ticks` steps. Ignores pairs that are no
    /// movement (hand-built routes).
    pub fn passed(&mut self, from: u32, to: u32, held_ticks: u32) {
        if let Some(m) = self.movement(from, to) {
            self.held_sum[m] += u64::from(held_ticks);
            self.held_n[m] += 1;
        }
    }

    /// Index of movement `(from, to)` in `succ`: a scan of `from`'s few
    /// successors.
    fn movement(&self, from: u32, to: u32) -> Option<usize> {
        let (a, b) = (self.first[from as usize], self.first[from as usize + 1]);
        (a as usize..b as usize).find(|&i| self.succ[i] == to)
    }

    /// Moves each road's travel-time estimate towards what its vehicles
    /// show now: `length / mean speed`, or the free-flow time on an empty
    /// road. Moves each movement's delay towards the mean hold of the
    /// passages since the last call, or its prior without any, but at least
    /// the wait so far of a head still held for it. `dt_s` is the step length.
    #[allow(clippy::cast_precision_loss)] // tick sums stay far below 2^52
    pub fn observe(&mut self, net: &Network, vs: &Vehicles, dt_s: f64) {
        self.head_s.fill(0.0);
        for (r, road) in (0u32..).zip(&net.roads) {
            for lane in 0..road.lanes {
                let Some(&s) = vs.lane(vs.lane_id(r, lane)).front() else {
                    continue;
                };
                let s = s as usize;
                let to = vs.route[s].get(vs.route_pos[s] as usize + 1);
                if let Some(m) = to.and_then(|&to| self.movement(r, to)) {
                    let wait = f64::from(vs.held_ticks[s]) * dt_s;
                    self.head_s[m] = self.head_s[m].max(wait);
                }
            }
        }
        for m in 0..self.succ.len() {
            let base = match self.held_n[m] {
                0 => self.prior_s[m],
                n => self.held_sum[m] as f64 * dt_s / f64::from(n),
            };
            let target = base.max(self.head_s[m]);
            let d = &mut self.delay_s[m];
            *d = (*d + SMOOTHING * (target - *d)).max(0.0);
            self.move_s[m] = self.turn_s[m] + *d;
            (self.held_sum[m], self.held_n[m]) = (0, 0);
        }
        self.observe_roads(net, vs);
    }

    fn observe_roads(&mut self, net: &Network, vs: &Vehicles) {
        for (r, road) in (0u32..).zip(&net.roads) {
            let (mut n, mut sum) = (0u32, 0.0);
            for lane in 0..road.lanes {
                for &s in vs.lane(vs.lane_id(r, lane)) {
                    n += 1;
                    sum += vs.speed_mps[s as usize];
                }
            }
            let (free, t) = (self.free_s[r as usize], &mut self.travel_s[r as usize]);
            let now = if n == 0 {
                free
            } else {
                road.length_m / (sum / f64::from(n)).max(MIN_SPEED_MPS)
            };
            *t = (*t + SMOOTHING * (now - *t)).max(free);
        }
    }

    /// Feeds the travel-time and delay estimates and the delay accumulators
    /// into `h` (the rest is fixed or scratch).
    pub fn hash_into(&self, h: &mut StateHasher) {
        for &t in &self.travel_s {
            h.write_f64(t);
        }
        for m in 0..self.succ.len() {
            h.write_f64(self.delay_s[m]);
            h.write_u64(self.held_sum[m]);
            h.write_u64(u64::from(self.held_n[m]));
        }
    }

    fn set(&mut self, road: u32, cost: f64, prev: u32) {
        let i = road as usize;
        self.stamp[i] = self.epoch;
        self.cost[i] = cost;
        self.prev[i] = prev;
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    // Plain products, not `powi`: ADR-0004 allows only basic ops and `sqrt`.
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt()
}

/// Iterative Tarjan (no recursion: small wasm stack). Returns, per vertex,
/// whether it is in the largest SCC; size ties go to the lowest member id.
fn largest_scc(first: &[u32], succ: &[u32]) -> Vec<bool> {
    let n = first.len() - 1;
    let mut index = vec![UNSEEN; n];
    let mut low = vec![0u32; n];
    let mut on_stack = vec![false; n];
    let mut comp = vec![UNSEEN; n];
    let mut comps: Vec<(u32, u32)> = Vec::new(); // (size, min id)
    let mut stack: Vec<u32> = Vec::new();
    let mut calls: Vec<(u32, u32)> = Vec::new(); // (vertex, next edge)
    let mut counter = 0u32;

    for root in 0..n {
        if index[root] != UNSEEN {
            continue;
        }
        let root = u32::try_from(root).expect("road count fits u32");
        let visit = |v: u32,
                     index: &mut [u32],
                     low: &mut [u32],
                     on_stack: &mut [bool],
                     counter: &mut u32,
                     stack: &mut Vec<u32>| {
            index[v as usize] = *counter;
            low[v as usize] = *counter;
            *counter += 1;
            on_stack[v as usize] = true;
            stack.push(v);
        };
        visit(
            root,
            &mut index,
            &mut low,
            &mut on_stack,
            &mut counter,
            &mut stack,
        );
        calls.push((root, first[root as usize]));
        while let Some(&(v, e)) = calls.last() {
            let vi = v as usize;
            if e < first[vi + 1] {
                calls.last_mut().expect("non-empty").1 += 1;
                let w = succ[e as usize];
                let wi = w as usize;
                if index[wi] == UNSEEN {
                    visit(
                        w,
                        &mut index,
                        &mut low,
                        &mut on_stack,
                        &mut counter,
                        &mut stack,
                    );
                    calls.push((w, first[wi]));
                } else if on_stack[wi] {
                    low[vi] = low[vi].min(index[wi]);
                }
                continue;
            }
            calls.pop();
            if low[vi] == index[vi] {
                let id = u32::try_from(comps.len()).expect("fits u32");
                let (mut size, mut min) = (0, v);
                loop {
                    let w = stack.pop().expect("v is on the stack");
                    on_stack[w as usize] = false;
                    comp[w as usize] = id;
                    size += 1;
                    min = min.min(w);
                    if w == v {
                        break;
                    }
                }
                comps.push((size, min));
            }
            if let Some(&(p, _)) = calls.last() {
                low[p as usize] = low[p as usize].min(low[vi]);
            }
        }
    }

    let best = (0..comps.len())
        .max_by_key(|&c| (comps[c].0, std::cmp::Reverse(comps[c].1)))
        .map_or(UNSEEN, |c| u32::try_from(c).expect("fits u32"));
    comp.iter().map(|&c| c == best).collect()
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)] // tiny generated networks
mod tests {
    use super::*;
    use crate::network::Road;
    use proptest::prelude::*;

    fn road(from: u32, to: u32, length_m: f64) -> Road {
        Road {
            from,
            to,
            length_m,
            speed_limit_mps: 10.0,
            lanes: 1,
            start_dir: [1.0, 0.0],
            end_dir: [1.0, 0.0],
            roundabout: false,
        }
    }

    fn trip(origin: u32, destination: u32) -> Trip {
        Trip {
            origin,
            destination,
        }
    }

    /// Roads 0..=4: straight 0-1-4 versus detour 0-2-3-4 around the square.
    fn detour_net(straight_turn_allowed: bool) -> Network {
        let mut connections = vec![(0, 1), (0, 2), (2, 3), (3, 4)];
        if straight_turn_allowed {
            connections.push((1, 4));
        }
        Network {
            nodes: vec![
                [0.0, 0.0],
                [100.0, 0.0],
                [200.0, 0.0],
                [100.0, 100.0],
                [300.0, 0.0],
            ],
            roads: vec![
                road(0, 1, 100.0),
                road(1, 2, 100.0),
                road(1, 3, 100.0),
                road(3, 2, 142.0),
                road(2, 4, 100.0),
            ],
            connections,
        }
    }

    #[test]
    fn shortest_route_and_turn_restriction_detour() {
        let mut out = Vec::new();
        let net = detour_net(true);
        assert!(Router::new(&net).route(&net, trip(0, 4), &mut out));
        assert_eq!(out, [0, 1, 4]);
        let net = detour_net(false);
        assert!(Router::new(&net).route(&net, trip(0, 4), &mut out));
        assert_eq!(out, [0, 2, 3, 4]);
        assert!(!Router::new(&net).route(&net, trip(4, 0), &mut out));
        assert!(out.is_empty());
    }

    /// From road 0 (heading N into node 1) to road 6: left across the
    /// oncoming road 1, right, left (roads 2, 3), or straight, left,
    /// straight (roads 4, 5), 3 s slower to drive.
    fn left_or_around() -> Network {
        let r = |from, to, length_m, d: [f64; 2]| Road {
            start_dir: d,
            end_dir: d,
            ..road(from, to, length_m)
        };
        let (n, s, w) = ([0.0, 1.0], [0.0, -1.0], [-1.0, 0.0]);
        Network {
            nodes: vec![
                [0.0, -100.0],
                [0.0, 0.0],
                [0.0, 100.0],
                [-100.0, 0.0],
                [-100.0, 100.0],
                [-200.0, 100.0],
            ],
            roads: vec![
                r(0, 1, 100.0, n),
                r(2, 1, 100.0, s),
                r(1, 3, 100.0, w),
                r(3, 4, 100.0, n),
                r(1, 2, 100.0, n),
                r(2, 4, 130.0, w),
                r(4, 5, 100.0, w),
            ],
            connections: vec![(0, 2), (0, 4), (2, 3), (3, 6), (4, 5), (5, 6)],
        }
    }

    #[test]
    fn a_left_turn_across_oncoming_traffic_is_avoided_for_a_near_equal_route() {
        let net = left_or_around();
        let mut out = Vec::new();
        assert!(Router::new(&net).route(&net, trip(0, 6), &mut out));
        assert_eq!(out, [0, 4, 5, 6]);
    }

    #[test]
    fn a_movement_that_keeps_blocking_gets_costlier_and_then_recovers() {
        let net = left_or_around();
        let mut router = Router::new(&net);
        let mut vs = Vehicles::new(&net);
        let mut out = Vec::new();
        // A head on road 0 waits to go straight onto road 4, 30 s so far.
        let id = vs.spawn(crate::vehicle::NewVehicle {
            road: 0,
            lane: 0,
            pos_m: 90.0,
            speed_mps: 0.0,
            params: crate::vehicle::idm::IdmParams::default(),
            route: &[0, 4, 5, 6],
            depart_tick: 0,
        });
        for k in 0..12 {
            vs.held_ticks[id.slot as usize] = 300 + k * 50;
            router.observe(&net, &vs, 0.1);
        }
        assert!(router.route(&net, trip(0, 6), &mut out));
        assert_eq!(out, [0, 2, 3, 6], "the blocked movement is avoided");
        // It leaves; quick passages pull the cost back down.
        vs.despawn(id);
        for _ in 0..40 {
            router.passed(0, 4, 0);
            router.observe(&net, &vs, 0.1);
        }
        assert!(router.route(&net, trip(0, 6), &mut out));
        assert_eq!(out, [0, 4, 5, 6]);
    }

    fn two_cycles(a: [u32; 2], b: [u32; 2]) -> Network {
        // Cycles a and b, road 4 is a one-way trap fed from a[0].
        Network {
            nodes: vec![[0.0; 2]; 6],
            roads: (0..5).map(|_| road(0, 1, 10.0)).collect(),
            connections: vec![
                (a[0], a[1]),
                (a[1], a[0]),
                (b[0], b[1]),
                (b[1], b[0]),
                (a[0], 4),
            ],
        }
    }

    #[test]
    fn scc_excludes_trap_and_ties_go_to_lowest_id() {
        let r = Router::new(&two_cycles([0, 1], [2, 3]));
        assert_eq!(r.routable(), [0, 1]);
        assert_eq!(r.excluded_roads().collect::<Vec<_>>(), [2, 3, 4]);
        assert_eq!(r.excluded_count(), 3);
        let r = Router::new(&two_cycles([1, 2], [0, 3]));
        assert_eq!(r.routable(), [0, 3]);
    }

    prop_compose! {
        fn arb_net()(
            nodes in prop::collection::vec((0.0..1000.0_f64, 0.0..1000.0_f64), 2..8),
            roads in prop::collection::vec((any::<u8>(), any::<u8>(), 1.0..2.0_f64, 5.0..30.0_f64, 0..8usize, 0..8usize), 1..20),
            keep in prop::collection::vec(any::<bool>(), 400),
        ) -> Network {
            let n = nodes.len();
            let nodes: Vec<[f64; 2]> = nodes.into_iter().map(|(x, y)| [x, y]).collect();
            // Eight compass directions.
            let k = std::f64::consts::FRAC_1_SQRT_2;
            let dirs = [[1.0, 0.0], [k, k], [0.0, 1.0], [-k, k],
                [-1.0, 0.0], [-k, -k], [0.0, -1.0], [k, -k]];
            let roads: Vec<Road> = roads.into_iter().map(|(f, t, stretch, v, d0, d1)| {
                let (f, t) = (f as usize % n, t as usize % n);
                Road {
                    speed_limit_mps: v,
                    start_dir: dirs[d0],
                    end_dir: dirs[d1],
                    length_m: dist(nodes[f], nodes[t]) * stretch + 1.0,
                    ..road(f as u32, t as u32, 0.0)
                }
            }).collect();
            let mut connections = Vec::new();
            let mut k = 0;
            for (a, ra) in roads.iter().enumerate() {
                for (b, rb) in roads.iter().enumerate() {
                    if ra.to == rb.from {
                        if keep[k % keep.len()] {
                            connections.push((a as u32, b as u32));
                        }
                        k += 1;
                    }
                }
            }
            Network { nodes, roads, connections }
        }
    }

    /// The router's cost of entering road `b` from road `a`: the movement
    /// cost plus `b`'s travel time.
    fn edge_s(router: &Router, a: u32, b: u32) -> f64 {
        let m = router.movement(a, b).expect("a connection");
        router.move_s[m] + router.travel_s[b as usize]
    }

    /// Cheapest route cost by Bellman–Ford, the reference for A*.
    fn reference_cost(router: &Router, net: &Network, from: u32, to: u32) -> Option<f64> {
        let mut d = vec![f64::INFINITY; net.roads.len()];
        d[from as usize] = router.travel_s[from as usize];
        for _ in 0..net.roads.len() {
            for &(a, b) in &net.connections {
                d[b as usize] = d[b as usize].min(d[a as usize] + edge_s(router, a, b));
            }
        }
        d[to as usize].is_finite().then(|| d[to as usize])
    }

    proptest! {
        /// A* with turn costs and observed delays finds the Bellman–Ford
        /// optimum, and all movement costs are non-negative.
        #[test]
        fn routes_are_valid_connected_and_cheapest(
            net in arb_net(),
            waits in prop::collection::vec((any::<prop::sample::Index>(), 0..2000u32), 0..20),
        ) {
            let mut router = Router::new(&net);
            if !net.connections.is_empty() {
                for (i, ticks) in waits {
                    let (a, b) = net.connections[i.index(net.connections.len())];
                    router.passed(a, b, ticks);
                }
                router.observe(&net, &Vehicles::new(&net), 0.1);
            }
            prop_assert!(router.move_s.iter().all(|&c| c >= 0.0));
            let mut out = Vec::new();
            let n = net.roads.len() as u32;
            for o in 0..n {
                for d in 0..n {
                    let found = router.route(&net, trip(o, d), &mut out);
                    let best = reference_cost(&router, &net, o, d);
                    prop_assert_eq!(found, best.is_some());
                    if let Some(best) = best {
                        prop_assert_eq!((out[0], *out.last().unwrap()), (o, d));
                        prop_assert!(out.windows(2).all(|w| net.connections.contains(&(w[0], w[1]))));
                        let cost = router.travel_s[o as usize]
                            + out.windows(2).map(|w| edge_s(&router, w[0], w[1])).sum::<f64>();
                        prop_assert!((cost - best).abs() < 1e-6);
                    }
                }
            }
            for &o in &router.routable().to_vec() {
                for &d in router.routable() {
                    prop_assert!(router.clone().route(&net, trip(o, d), &mut out));
                }
            }
        }
    }
}
