//! Route finding over the network: A*, later contraction hierarchies (M2, M5).
//!
//! The road graph has roads as vertices and `connections` as edges, so turn
//! restrictions are honored by construction. The cost of a route is the sum of
//! `length / speed limit` over its roads, origin and destination included.
//! M2 traffic stays inside the largest strongly connected component (SCC), so
//! every origin–destination pair taken from [`Router::routable`] has a route
//! (ADR-0004).

use std::collections::BinaryHeap;

use crate::demand::Trip;
use crate::network::Network;

const UNSEEN: u32 = u32::MAX;

/// A* router with scratch buffers reused across calls (no allocation per
/// route after warm-up). Must be used with the network it was built from.
#[derive(Debug, Clone)]
pub struct Router {
    /// CSR adjacency: successors of road `r` are `succ[first[r]..first[r + 1]]`.
    first: Vec<u32>,
    succ: Vec<u32>,
    routable: Vec<u32>,
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

        Self {
            first,
            succ,
            routable,
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
        let time = |road: u32| {
            let r = &net.roads[road as usize];
            r.length_m / r.speed_limit_mps
        };

        let g0 = time(origin);
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
                let g2 = g + time(nb);
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
            roads in prop::collection::vec((any::<u8>(), any::<u8>(), 1.0..2.0_f64, 5.0..30.0_f64), 1..20),
            keep in prop::collection::vec(any::<bool>(), 400),
        ) -> Network {
            let n = nodes.len();
            let nodes: Vec<[f64; 2]> = nodes.into_iter().map(|(x, y)| [x, y]).collect();
            let roads: Vec<Road> = roads.into_iter().map(|(f, t, stretch, v)| {
                let (f, t) = (f as usize % n, t as usize % n);
                Road {
                    speed_limit_mps: v,
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

    /// Cheapest route cost by Bellman–Ford, the reference for A*.
    fn reference_cost(net: &Network, from: u32, to: u32) -> Option<f64> {
        let t = |r: u32| net.roads[r as usize].length_m / net.roads[r as usize].speed_limit_mps;
        let mut d = vec![f64::INFINITY; net.roads.len()];
        d[from as usize] = t(from);
        for _ in 0..net.roads.len() {
            for &(a, b) in &net.connections {
                d[b as usize] = d[b as usize].min(d[a as usize] + t(b));
            }
        }
        d[to as usize].is_finite().then(|| d[to as usize])
    }

    proptest! {
        #[test]
        fn routes_are_valid_connected_and_cheapest(net in arb_net()) {
            let mut router = Router::new(&net);
            let mut out = Vec::new();
            let n = net.roads.len() as u32;
            for o in 0..n {
                for d in 0..n {
                    let found = router.route(&net, trip(o, d), &mut out);
                    let best = reference_cost(&net, o, d);
                    prop_assert_eq!(found, best.is_some());
                    if let Some(best) = best {
                        prop_assert_eq!((out[0], *out.last().unwrap()), (o, d));
                        prop_assert!(out.windows(2).all(|w| net.connections.contains(&(w[0], w[1]))));
                        let cost: f64 = out.iter().map(|&r| net.roads[r as usize].length_m / net.roads[r as usize].speed_limit_mps).sum();
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
