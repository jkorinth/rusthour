//! Route finding over the network: A*, later contraction hierarchies (M2, M6).
//!
//! The road graph has roads as vertices and `connections` (movements) as
//! edges, so turn restrictions are honored by construction. The cost of a
//! route is the sum of each road's current travel-time estimate, origin and
//! destination included: `length / speed limit` on an empty road,
//! `length / mean speed` of its vehicles otherwise ([`Router::observe`],
//! M2.9); plus, per movement, a static turn penalty and the observed delay
//! at the stop line (ADR-0005 §2).
//! The A* bound comes from landmarks (ALT, M4.5): the exact costs to and
//! from a few roads, recomputed whenever the costs change ([`Router::observe`]),
//! so the bound is admissible and consistent (ADR-0005, addendum M4.5).
//! M2 traffic stays inside the largest strongly connected component (SCC), so
//! every origin–destination pair taken from [`Router::routable`] has a route
//! (ADR-0004).

use std::collections::BinaryHeap;

use crate::StateHasher;
use crate::control::priority::rank;
use crate::control::signal::Plan;
use crate::demand::Trip;
use crate::network::movement::{self, Turn};
use crate::network::{EndControl, Network};
use crate::vehicle::Vehicles;

const UNSEEN: u32 = u32::MAX;
/// `prev` of a road entered straight from the origin in `route_from_any`.
const SOURCE: u32 = u32::MAX - 1;
/// Floor of the mean speed in the travel-time estimate (m/s). Low on
/// purpose: a road whose cars all stand gets a near-prohibitive cost, which
/// keeps routes out of forming gridlocks (ADR-0004, addendum M2.9).
const MIN_SPEED_MPS: f64 = 0.01;
/// Weight of a new observation in the travel-time and delay estimates (EMAs).
const SMOOTHING: f64 = 0.2;
/// Delay prior of a movement whose road ranks below another at the node (s).
const YIELD_PRIOR_S: f64 = 4.0;
/// Delay prior of a movement at a stop sign (s).
const STOP_PRIOR_S: f64 = 6.0;

/// Number of landmarks for the A* bound. Each costs two Dijkstra runs per
/// `observe`; 4 ran faster than 8 on both benchmarks (ADR-0005, addendum M4.5).
const LANDMARKS: usize = 4;

/// Static cost of a turn (s).
// shortcut: guessed values from ADR-0005, and the M3 trial passed on them; calibrate with M4.7.
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
    /// Per movement: its turn type, for inspection.
    kind: Vec<Turn>,
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
    /// Per road: smoothed current travel time (s), never below `free_s`.
    travel_s: Vec<f64>,
    /// Reversed CSR: the movements into road `r` are
    /// `pred_move[pred_first[r]..pred_first[r + 1]]` (indices into `succ`), from the
    /// roads `pred[..]` at the same positions.
    pred_first: Vec<u32>,
    pred: Vec<u32>,
    pred_move: Vec<u32>,
    /// Landmark roads, chosen at load.
    landmarks: Vec<u32>,
    /// Per road: current cost from each landmark to it and from it to each
    /// landmark (s), `INFINITY` if unreachable; unused landmarks are all 0.
    from_lm: Vec<[f64; LANDMARKS]>,
    to_lm: Vec<[f64; LANDMARKS]>,
    /// `from_lm` and `to_lm` of the current search's destination.
    goal_from: [f64; LANDMARKS],
    goal_to: [f64; LANDMARKS],
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
        // Turn penalties and yield, stop and signal priors, per movement.
        let (in_first, incoming) = movement::incoming(net);
        let plans: Vec<Option<Plan>> = (0..net.nodes.len())
            .map(|node| Plan::new(net, &in_first, &incoming, node))
            .collect();
        let mut turn = vec![0.0; succ.len()];
        let mut kind = vec![Turn::Straight; succ.len()];
        let mut prior = vec![0.0; succ.len()];
        for from in 0..n {
            let node = net.roads[from].to as usize;
            let arms = &incoming[in_first[node] as usize..in_first[node + 1] as usize];
            let from = u32::try_from(from).expect("road count fits u32");
            let mine = rank(net, from);
            let prior_s = if let Some(plan) = &plans[node] {
                // A non-signal arm at a signal node yields to the signal arms.
                plan.phase_of(from)
                    .map_or(YIELD_PRIOR_S, |_| plan.delay_prior_s())
            } else if net.roads[from as usize].end_control == EndControl::Stop {
                STOP_PRIOR_S
            } else if arms.iter().any(|&r| rank(net, r) > mine) {
                YIELD_PRIOR_S
            } else {
                0.0
            };
            for i in first[from as usize] as usize..first[from as usize + 1] as usize {
                kind[i] = Turn::of(net, from, succ[i], arms);
                turn[i] = turn_s(kind[i]);
                prior[i] = prior_s;
            }
        }
        let in_scc = largest_scc(&first, &succ);
        let routable: Vec<u32> = (0..n)
            .filter(|&r| in_scc[r])
            .map(|r| u32::try_from(r).expect("road count fits u32"))
            .collect();

        let free_s: Vec<f64> = (net.roads.iter())
            .map(|r| r.length_m / r.speed_limit_mps)
            .collect();
        let m = succ.len();
        let mut pred_first = vec![0u32; n + 1];
        for &to in &succ {
            pred_first[to as usize + 1] += 1;
        }
        for i in 0..n {
            pred_first[i + 1] += pred_first[i];
        }
        let mut fill = pred_first.clone();
        let (mut pred, mut pred_move) = (vec![0u32; m], vec![0u32; m]);
        for from in 0..n {
            for i in first[from]..first[from + 1] {
                let k = &mut fill[succ[i as usize] as usize];
                pred[*k as usize] = u32::try_from(from).expect("road count fits u32");
                pred_move[*k as usize] = i;
                *k += 1;
            }
        }

        let mut router = Self {
            move_s: turn.iter().zip(&prior).map(|(t, p)| t + p).collect(),
            kind,
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
            pred_first,
            pred,
            pred_move,
            landmarks: Vec::with_capacity(LANDMARKS),
            from_lm: vec![[0.0; LANDMARKS]; n],
            to_lm: vec![[0.0; LANDMARKS]; n],
            goal_from: [0.0; LANDMARKS],
            goal_to: [0.0; LANDMARKS],
            cost: vec![0.0; n],
            prev: vec![0; n],
            stamp: vec![0; n],
            epoch: 0,
            heap: BinaryHeap::new(),
        };
        router.pick_landmarks();
        router
    }

    /// Chooses up to [`LANDMARKS`] routable roads farthest-first on the
    /// costs at load: each next one maximizes the smallest round trip to
    /// those chosen so far (ties to the lowest id), the first one the round
    /// trip to the lowest routable road. Leaves their tables filled.
    fn pick_landmarks(&mut self) {
        let Some(&start) = self.routable.first() else {
            return;
        };
        self.fill_column(0, start); // a probe, overwritten below
        let round = |r: &Self, i: usize| r.from_lm[i][0] + r.to_lm[i][0];
        let mut near: Vec<f64> = (0..self.free_s.len()).map(|i| round(self, i)).collect();
        for k in 0..LANDMARKS.min(self.routable.len()) {
            let lm = (self.routable.iter().copied())
                .max_by(|&a, &b| {
                    near[a as usize]
                        .total_cmp(&near[b as usize])
                        .then(b.cmp(&a))
                })
                .expect("routable is not empty");
            self.landmarks.push(lm);
            self.fill_column(k, lm);
            for (i, d) in near.iter_mut().enumerate() {
                let r = self.from_lm[i][k] + self.to_lm[i][k];
                *d = if k == 0 { r } else { d.min(r) };
            }
        }
    }

    /// Recomputes the landmark tables on the current costs.
    fn refresh_landmarks(&mut self) {
        for k in 0..self.landmarks.len() {
            self.fill_column(k, self.landmarks[k]);
        }
    }

    /// Fills column `k` of `from_lm` and `to_lm` with the current costs from
    /// and to road `lm`: Dijkstra forward, then on the reversed graph.
    fn fill_column(&mut self, k: usize, lm: u32) {
        let Self {
            first,
            succ,
            pred_first,
            pred,
            pred_move,
            move_s,
            travel_s,
            heap,
            from_lm,
            to_lm,
            ..
        } = self;
        for forward in [true, false] {
            let dist = if forward { &mut *from_lm } else { &mut *to_lm };
            for row in dist.iter_mut() {
                row[k] = f64::INFINITY;
            }
            dist[lm as usize][k] = 0.0;
            heap.clear();
            heap.push(Entry {
                f: 0.0,
                g: 0.0,
                road: lm,
            });
            while let Some(Entry { g, road, .. }) = heap.pop() {
                let r = road as usize;
                if g > dist[r][k] {
                    continue;
                }
                let (lo, hi) = if forward {
                    (first[r], first[r + 1])
                } else {
                    (pred_first[r], pred_first[r + 1])
                };
                for i in lo as usize..hi as usize {
                    // Entering `succ[i]` from `road`, or `road` from `pred[i]`.
                    let (nb, c) = if forward {
                        (succ[i], move_s[i] + travel_s[succ[i] as usize])
                    } else {
                        (pred[i], move_s[pred_move[i] as usize] + travel_s[r])
                    };
                    let g2 = g + c;
                    if g2 < dist[nb as usize][k] {
                        dist[nb as usize][k] = g2;
                        heap.push(Entry {
                            f: g2,
                            g: g2,
                            road: nb,
                        });
                    }
                }
            }
        }
    }

    /// The movements out of road `from` as `(to, turn, delay_s, prior_s)`:
    /// the smoothed stop-line delay now and what it decays to.
    pub fn movement_delays(&self, from: u32) -> impl Iterator<Item = (u32, Turn, f64, f64)> + '_ {
        let (a, b) = (self.first[from as usize], self.first[from as usize + 1]);
        (a as usize..b as usize)
            .map(|i| (self.succ[i], self.kind[i], self.delay_s[i], self.prior_s[i]))
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
    pub fn route(&mut self, _net: &Network, trip: Trip, out: &mut Vec<u32>) -> bool {
        self.epoch += 1;
        self.heap.clear();
        self.set_goal(trip.destination);
        let g0 = self.travel_s[trip.origin as usize];
        self.push(trip.origin, g0, UNSEEN);
        self.search(trip.origin, trip.destination, out)
    }

    /// Like [`Self::route`], but leaves `origin` only by a movement onto a
    /// road for which `first(to)` holds: a multi-source A* from those
    /// movements, each with its edge cost (ADR-0005 §7). The route may pass
    /// `origin` again, e.g. once round the block.
    pub fn route_from_any(
        &mut self,
        _net: &Network,
        origin: u32,
        first: impl Fn(u32) -> bool,
        dest: u32,
        out: &mut Vec<u32>,
    ) -> bool {
        self.epoch += 1;
        self.heap.clear();
        self.set_goal(dest);
        let g0 = self.travel_s[origin as usize];
        let (a, b) = (self.first[origin as usize], self.first[origin as usize + 1]);
        for i in a as usize..b as usize {
            let nb = self.succ[i];
            let g = g0 + self.move_s[i] + self.travel_s[nb as usize];
            if first(nb) && (self.stamp[nb as usize] != self.epoch || g < self.cost[nb as usize]) {
                self.push(nb, g, SOURCE);
            }
        }
        self.search(origin, dest, out)
    }

    fn set_goal(&mut self, dest: u32) {
        self.goal_from = self.from_lm[dest as usize];
        self.goal_to = self.to_lm[dest as usize];
    }

    /// Lower bound on the cost after `road` to the goal: the best landmark
    /// triangle bound, at least 0. Comparisons skip the `NaN` of
    /// `INFINITY − INFINITY` (neither road reaches or is reached by that
    /// landmark); `+INFINITY` means the goal is out of reach.
    fn bound_s(&self, road: u32) -> f64 {
        let (from, to) = (&self.from_lm[road as usize], &self.to_lm[road as usize]);
        let mut h = 0.0;
        for k in 0..LANDMARKS {
            for b in [self.goal_from[k] - from[k], to[k] - self.goal_to[k]] {
                if b > h {
                    h = b;
                }
            }
        }
        h
    }

    /// Records `road` reached at cost `g` from `prev` and queues it.
    fn push(&mut self, road: u32, g: f64, prev: u32) {
        let f = g + self.bound_s(road);
        self.set(road, g, prev);
        self.heap.push(Entry { f, g, road });
    }

    /// Runs A* from the queued roads to `dest` and writes the route into
    /// `out`; a road whose `prev` is `SOURCE` follows `origin`.
    fn search(&mut self, origin: u32, dest: u32, out: &mut Vec<u32>) -> bool {
        out.clear();
        while let Some(Entry { g, road, .. }) = self.heap.pop() {
            if g > self.cost[road as usize] {
                continue; // stale entry
            }
            if road == dest {
                let mut r = dest;
                while r != UNSEEN {
                    if r == SOURCE {
                        out.push(origin);
                        break;
                    }
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
                    self.push(nb, g2, road);
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
        self.refresh_landmarks();
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
    use crate::network::{EndControl, Road};
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
            class: 0,
            priority_road: false,
            end_control: EndControl::None,
            turn_lanes: vec![],
        }
    }

    fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
        let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
        (dx * dx + dy * dy).sqrt()
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
    fn route_from_any_starts_with_an_allowed_movement_and_may_pass_the_origin_again() {
        // A loop 0 → 1 → 2 → 0 with an exit 3 off road 0's end.
        let net = Network {
            nodes: vec![[0.0, 0.0], [100.0, 0.0], [50.0, 80.0], [200.0, 0.0]],
            roads: vec![
                road(0, 1, 100.0),
                road(1, 2, 100.0),
                road(2, 0, 100.0),
                road(1, 3, 100.0),
            ],
            connections: vec![(0, 1), (0, 3), (1, 2), (2, 0)],
        };
        let mut router = Router::new(&net);
        let mut out = Vec::new();
        assert!(router.route_from_any(&net, 0, |to| to == 1, 3, &mut out));
        assert_eq!(out, [0, 1, 2, 0, 3], "once round the loop");
        assert!(router.route_from_any(&net, 0, |to| to == 3, 3, &mut out));
        assert_eq!(out, [0, 3]);
        assert!(!router.route_from_any(&net, 3, |_| true, 0, &mut out));
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
    fn signs_set_the_delay_prior() {
        let priors = |end_control| {
            let mut net = left_or_around();
            net.roads[0].end_control = end_control;
            let router = Router::new(&net);
            router.movement_delays(0).map(|m| m.3).collect::<Vec<_>>()
        };
        assert_eq!(priors(EndControl::None), [0.0, 0.0]);
        assert_eq!(priors(EndControl::GiveWay), [4.0, 4.0]);
        assert_eq!(priors(EndControl::Stop), [6.0, 6.0]);
        // One phase: red 5 s of a 30 s cycle.
        let signal = 5.0 * 5.0 / 60.0;
        assert_eq!(priors(EndControl::Signals), [signal, signal]);
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
            mobil: crate::behavior::mobil::MobilParams::default(),
            profile: crate::behavior::profile::Profile::Normal,
            class: crate::vehicle::class::Class::Car,
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

    /// Cheapest remaining cost from every road to `to` by Bellman–Ford,
    /// the reference for A*: the movements and roads after the road itself.
    fn remaining_s(router: &Router, net: &Network, to: u32) -> Vec<f64> {
        let mut d = vec![f64::INFINITY; net.roads.len()];
        d[to as usize] = 0.0;
        for _ in 0..net.roads.len() {
            for &(a, b) in &net.connections {
                d[a as usize] = d[a as usize].min(edge_s(router, a, b) + d[b as usize]);
            }
        }
        d
    }

    /// Congests the router: travel times up to 20× free flow, and passages
    /// held up to 200 s.
    fn congest(
        router: &mut Router,
        net: &Network,
        slow: &[f64],
        waits: &[(prop::sample::Index, u32)],
    ) {
        if net.connections.is_empty() {
            return;
        }
        for &(i, ticks) in waits {
            let (a, b) = net.connections[i.index(net.connections.len())];
            router.passed(a, b, ticks);
        }
        router.observe(net, &Vehicles::new(net), 0.1);
        for (t, k) in router.travel_s.iter_mut().zip(slow.iter().cycle()) {
            *t *= k;
        }
        router.refresh_landmarks();
    }

    /// Checks every origin–destination pair: A* finds a connected route of
    /// the reference cost, and the landmark bound never exceeds the true
    /// remaining cost of any road, and meets it towards a landmark.
    fn check_all_pairs(router: &mut Router, net: &Network) -> Result<(), TestCaseError> {
        let mut out = Vec::new();
        let n = net.roads.len() as u32;
        for d in 0..n {
            let rem = remaining_s(router, net, d);
            router.set_goal(d);
            for v in 0..n {
                let (bound, rem) = (router.bound_s(v), rem[v as usize]);
                prop_assert!(bound <= rem + 1e-9, "bound {bound} > {rem} from {v} to {d}");
                if router.landmarks.contains(&d) && rem.is_finite() {
                    prop_assert!(
                        (bound - rem).abs() < 1e-9,
                        "bound {bound} < {rem} from {v} to landmark {d}"
                    );
                }
            }
            for o in 0..n {
                let found = router.route(net, trip(o, d), &mut out);
                prop_assert_eq!(found, rem[o as usize].is_finite());
                if found {
                    prop_assert_eq!((out[0], *out.last().unwrap()), (o, d));
                    prop_assert!(
                        out.windows(2)
                            .all(|w| net.connections.contains(&(w[0], w[1])))
                    );
                    let cost: f64 = out.windows(2).map(|w| edge_s(router, w[0], w[1])).sum();
                    prop_assert!((cost - rem[o as usize]).abs() < 1e-6);
                }
            }
        }
        Ok(())
    }

    /// A `k × k` grid of nodes 100 m apart, a road each way between
    /// neighbours, at `speeds` (cycled), and the movements `keep` (cycled)
    /// allows, U-turns included.
    fn grid(k: u32, speeds: &[f64], keep: &[bool]) -> Network {
        let mut net = Network {
            nodes: (0..k * k)
                .map(|i| [f64::from(i % k) * 100.0, f64::from(i / k) * 100.0])
                .collect(),
            ..Network::default()
        };
        for i in 0..k * k {
            for j in [i + 1, i + k] {
                if j < k * k && (j == i + k || j % k != 0) {
                    for (f, t) in [(i, j), (j, i)] {
                        let (a, b) = (net.nodes[f as usize], net.nodes[t as usize]);
                        let dir = [(b[0] - a[0]) / 100.0, (b[1] - a[1]) / 100.0];
                        net.roads.push(Road {
                            speed_limit_mps: speeds[net.roads.len() % speeds.len()],
                            start_dir: dir,
                            end_dir: dir,
                            ..road(f, t, 100.0)
                        });
                    }
                }
            }
        }
        let mut c = 0;
        for (a, ra) in net.roads.iter().enumerate() {
            for (b, rb) in net.roads.iter().enumerate() {
                if ra.to == rb.from {
                    if keep[c % keep.len()] {
                        net.connections.push((a as u32, b as u32));
                    }
                    c += 1;
                }
            }
        }
        net
    }

    proptest! {
        /// On random networks, congested and with observed delays, A* finds
        /// the Bellman–Ford optimum, all movement costs are non-negative,
        /// and the landmark bound is admissible.
        #[test]
        fn routes_are_valid_connected_and_cheapest(
            net in arb_net(),
            waits in prop::collection::vec((any::<prop::sample::Index>(), 0..2000u32), 0..20),
            slow in prop::collection::vec(1.0..20.0_f64, 1..8),
        ) {
            let mut router = Router::new(&net);
            congest(&mut router, &net, &slow, &waits);
            prop_assert!(router.move_s.iter().all(|&c| c >= 0.0));
            check_all_pairs(&mut router, &net)?;
            let mut out = Vec::new();
            for &o in &router.routable().to_vec() {
                for &d in router.routable() {
                    prop_assert!(router.clone().route(&net, trip(o, d), &mut out));
                }
            }
        }

        /// The same on random grids with missing movements.
        #[test]
        fn landmark_routes_on_congested_grids_are_cheapest(
            k in 2..5u32,
            speeds in prop::collection::vec(5.0..30.0_f64, 1..6),
            keep in prop::collection::vec(prop::bool::weighted(0.85), 1..64),
            waits in prop::collection::vec((any::<prop::sample::Index>(), 0..2000u32), 0..30),
            slow in prop::collection::vec(1.0..20.0_f64, 1..8),
        ) {
            let net = grid(k, &speeds, &keep);
            let mut router = Router::new(&net);
            congest(&mut router, &net, &slow, &waits);
            check_all_pairs(&mut router, &net)?;
        }
    }
}
