//! Fixed-time traffic signals (M3.4, ADR-0005 §4).
//!
//! A node with a signal on any approach is signalled on all of them, except
//! short links from another signalled node (split junctions): those flow on
//! and yield. Signal arms are grouped into phases, and green arms pass
//! through an inner [`Priority`], which handles permissive left turns.

use super::priority::Priority;
use super::{Approach, Controller};
use crate::StateHasher;
use crate::network::{EndControl, Network};

// shortcut: one default plan for every node, offset 0, no green wave; plans from the map or tuned per node later.
const GREEN_MS: u64 = 25_000;
const AMBER_MS: u64 = 3_000;
const ALL_RED_MS: u64 = 2_000;
const PHASE_MS: u64 = GREEN_MS + AMBER_MS + ALL_RED_MS;
/// A road shorter than this between two signalled nodes links a split
/// junction and gets no signal, as in the importer (ADR-0005 §3).
const SPLIT_LINK_M: f64 = 30.0;
/// The phase of a non-signal arm.
const NONE: u8 = u8::MAX;

/// What a signal shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Light {
    /// Go.
    Green,
    /// Stop unless it can't stop any more.
    Amber,
    /// Stop.
    Red,
}

/// A node's fixed-time plan: the phase each incoming road is green in.
#[derive(Debug, Clone)]
pub struct Plan {
    /// `(road, phase)` per incoming road, ascending; `NONE` for a non-signal arm.
    phase: Vec<(u32, u8)>,
    phases: u8,
}

impl Plan {
    /// The plan of `node`, or `None` if no road into it ends at signals.
    /// `in_first` and `incoming` are [`crate::network::movement::incoming`].
    ///
    /// Signal arms are grouped greedily in road-id order: a road joins the
    /// first phase whose roads all come from the opposite direction.
    ///
    /// # Panics
    ///
    /// If the node needs more than 254 phases.
    #[must_use]
    pub fn new(net: &Network, in_first: &[u32], incoming: &[u32], node: usize) -> Option<Self> {
        let into = |n: usize| &incoming[in_first[n] as usize..in_first[n + 1] as usize];
        let signalled = |n: usize| {
            (into(n).iter()).any(|&r| net.roads[r as usize].end_control == EndControl::Signals)
        };
        if !signalled(node) {
            return None;
        }
        let mut groups: Vec<Vec<[f64; 2]>> = Vec::new();
        let phase = (into(node).iter())
            .map(|&r| {
                let road = &net.roads[r as usize];
                if road.length_m < SPLIT_LINK_M && signalled(road.from as usize) {
                    return (r, NONE);
                }
                let d = road.end_dir;
                let opposite = |o: &[f64; 2]| d[0] * o[0] + d[1] * o[1] < -0.7;
                let k = if let Some(k) = groups.iter().position(|g| g.iter().all(opposite)) {
                    k
                } else {
                    groups.push(Vec::new());
                    groups.len() - 1
                };
                groups[k].push(d);
                (
                    r,
                    u8::try_from(k)
                        .ok()
                        .filter(|&k| k != NONE)
                        .expect("< 255 phases"),
                )
            })
            .collect();
        let phases = u8::try_from(groups.len()).expect("< 255 phases");
        (phases > 0).then_some(Self { phase, phases })
    }

    /// The phase `road` is green in; `None` for a non-signal arm.
    #[must_use]
    pub fn phase_of(&self, road: u32) -> Option<u8> {
        let i = self.phase.binary_search_by_key(&road, |&(r, _)| r).ok()?;
        Some(self.phase[i].1).filter(|&k| k != NONE)
    }

    /// The routing delay prior of a signal arm, `red²/(2·cycle)` (s): the
    /// mean wait of a vehicle arriving at a random time (ADR-0005 §2).
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // a few minutes in ms
    pub fn delay_prior_s(&self) -> f64 {
        let cycle = (u64::from(self.phases) * PHASE_MS) as f64 / 1000.0;
        let red = cycle - GREEN_MS as f64 / 1000.0;
        red * red / (2.0 * cycle)
    }

    /// What phase `k` shows at `time_ms`, and the ms until that changes.
    fn light(&self, k: u8, time_ms: u64) -> (Light, u64) {
        let cycle = u64::from(self.phases) * PHASE_MS;
        let into = (time_ms + cycle - u64::from(k) * PHASE_MS % cycle) % cycle;
        if into < GREEN_MS {
            (Light::Green, GREEN_MS - into)
        } else if into < GREEN_MS + AMBER_MS {
            (Light::Amber, GREEN_MS + AMBER_MS - into)
        } else {
            (Light::Red, cycle - into)
        }
    }
}

/// A signal-controlled node.
#[derive(Debug, Clone)]
pub struct Signal {
    plan: Plan,
    /// Decides among green and non-signal arms.
    inner: Priority,
    dt_ms: u64,
    /// `decide` scratch, sized for every incoming lane: the approaches passed
    /// to `inner`, their index in the caller's slice, and `inner`'s answer.
    aps: Vec<Approach>,
    index: Vec<usize>,
    go: Vec<bool>,
}

impl Signal {
    /// A controller for a node with `plan` and the given roads, both
    /// ascending; `patience` is in ticks (see [`Priority::new`]).
    #[must_use]
    pub fn new(
        net: &Network,
        plan: Plan,
        incoming: &[u32],
        outgoing: &[u32],
        patience: u64,
        dt_ms: u32,
    ) -> Self {
        let mut inner = Priority::new(net, incoming, outgoing, patience);
        for &(r, k) in &plan.phase {
            inner.set_signal(r, k != NONE);
        }
        let lanes = (incoming.iter())
            .map(|&r| usize::from(net.roads[r as usize].lanes))
            .sum();
        Self {
            plan,
            inner,
            dt_ms: u64::from(dt_ms),
            aps: Vec::with_capacity(lanes),
            index: Vec::with_capacity(lanes),
            go: Vec::with_capacity(lanes),
        }
    }

    /// What `road`'s signal shows at `tick` and the ms until that changes;
    /// `None` for a non-signal arm.
    fn light(&self, road: u32, tick: u64) -> Option<(Light, u64)> {
        let k = self.plan.phase_of(road)?;
        Some(self.plan.light(k, tick * self.dt_ms))
    }

    /// Each signal arm's `(road, light, seconds to the next change)` at `tick`.
    pub fn lights(&self, tick: u64) -> impl Iterator<Item = (u32, Light, f64)> + '_ {
        (self.plan.phase.iter()).filter_map(move |&(r, _)| {
            #[allow(clippy::cast_precision_loss)] // under a few minutes in ms
            self.light(r, tick)
                .map(|(l, ms)| (r, l, ms as f64 / 1000.0))
        })
    }

    /// Whether `a` faces amber or red at `tick`.
    #[must_use]
    pub fn holds_at_red(&self, a: &Approach, tick: u64) -> bool {
        self.light(a.from_road, tick)
            .is_some_and(|(l, _)| l != Light::Green)
    }

    /// See [`Priority::holds_at_stop_sign`].
    #[must_use]
    pub fn holds_at_stop_sign(&self, a: &Approach) -> bool {
        self.inner.holds_at_stop_sign(a)
    }
}

impl Controller for Signal {
    fn decide(&mut self, net: &Network, tick: u64, aps: &[Approach], go: &mut [bool]) {
        // Green and non-signal arms go to `inner`; the others don't compete.
        self.aps.clear();
        self.index.clear();
        for (i, a) in aps.iter().enumerate() {
            match self.light(a.from_road, tick) {
                None | Some((Light::Green, _)) => {
                    self.aps.push(*a);
                    self.index.push(i);
                }
                Some((light, _)) => {
                    go[i] = light == Light::Amber && !a.can_stop && !a.blocked;
                }
            }
        }
        self.go.clear();
        self.go.resize(self.aps.len(), false);
        self.inner.decide(net, tick, &self.aps, &mut self.go);
        for (&i, &g) in self.index.iter().zip(&self.go) {
            go[i] = g;
        }
    }

    fn hash_into(&self, h: &mut StateHasher) {
        // The lights are a function of the tick.
        self.inner.hash_into(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::Road;
    use crate::network::movement::incoming;

    /// Roads 0..4 end at node 0 heading N, W, S, E, from neighbours 1..=4;
    /// roads 4..8 leave it heading N, W, S, E. Only road 0 has signals
    /// tagged, and road `short`, if any, is 20 m long.
    fn node(short: Option<u32>) -> Network {
        let dirs = [[0.0, 1.0], [-1.0, 0.0], [0.0, -1.0], [1.0, 0.0]];
        let road = |from, to, d: [f64; 2]| Road {
            from,
            to,
            length_m: 100.0,
            speed_limit_mps: 13.9,
            lanes: 1,
            start_dir: d,
            end_dir: d,
            roundabout: false,
            class: 0,
            priority_road: false,
            end_control: EndControl::None,
            turn_lanes: vec![],
        };
        let mut roads: Vec<Road> = (0..4).map(|i| road(i + 1, 0, dirs[i as usize])).collect();
        roads.extend((0..4).map(|i| road(0, (i + 2) % 4 + 1, dirs[i as usize])));
        roads[0].end_control = EndControl::Signals;
        if let Some(r) = short {
            // Its start, the neighbour, is signalled too.
            roads[r as usize].length_m = 20.0;
            let mut back = road(0, roads[r as usize].from, [0.0, 1.0]);
            back.end_control = EndControl::Signals;
            roads.push(back);
        }
        Network {
            nodes: vec![
                [0.0, 0.0],
                [0.0, -100.0],
                [100.0, 0.0],
                [0.0, 100.0],
                [-100.0, 0.0],
            ],
            roads,
            connections: (0..4).flat_map(|i| (4..8).map(move |o| (i, o))).collect(),
        }
    }

    fn signal(net: &Network) -> Signal {
        let (first, inc) = incoming(net);
        let plan = Plan::new(net, &first, &inc, 0).expect("signalled");
        Signal::new(net, plan, &[0, 1, 2, 3], &[4, 5, 6, 7], 100, 100)
    }

    /// A head on `road` standing at the line, going to `to`.
    fn ap(road: u32, to_road: u32, slot: u32) -> Approach {
        Approach {
            from_road: road,
            lane: 0,
            to_road,
            to_lane: 0,
            slot,
            dist_m: 2.0,
            speed_mps: 0.0,
            min_gap_m: 2.0,
            can_stop: true,
            blocked: false,
        }
    }

    fn decide(s: &mut Signal, net: &Network, tick: u64, aps: &[Approach]) -> Vec<bool> {
        let mut go = vec![false; aps.len()];
        s.decide(net, tick, aps, &mut go);
        go
    }

    #[test]
    fn a_four_arm_signal_cycles_and_never_lets_crossing_arms_go_together() {
        let net = node(None);
        let mut s = signal(&net);
        // One head per arm, each going straight on.
        let aps: Vec<Approach> = (0..4).map(|r| ap(r, 4 + r, 10 + r)).collect();
        let mut served = [0u32; 4];
        for tick in 0..1200 {
            let go = decide(&mut s, &net, tick, &aps);
            let green: Vec<bool> = (0..4)
                .map(|r| s.light(r, tick).unwrap().0 == Light::Green)
                .collect();
            for r in 0..4 {
                assert!(!(go[r] && go[(r + 1) % 4]), "tick {tick}: {go:?}");
                assert!(!(green[r] && green[(r + 1) % 4]), "tick {tick}: {green:?}");
                assert_eq!(
                    go[r], green[r],
                    "tick {tick}: a lone straight head goes on green"
                );
                served[r] += u32::from(go[r]);
            }
        }
        // Two phases of 30 s, 25 s green each, over two minutes.
        assert_eq!(served, [500; 4]);
        assert!((s.plan.delay_prior_s() - 35.0 * 35.0 / 120.0).abs() < 1e-9);
    }

    #[test]
    fn a_permissive_left_turn_on_green_yields_to_oncoming_traffic() {
        // Even from the higher-class road: signals override class.
        let mut net = node(None);
        net.roads[0].class = 5;
        let mut s = signal(&net);
        // Roads 0 (N) and 2 (S) are green first; 0 turns left into 5 (W).
        let left = ap(0, 5, 10);
        assert_eq!(
            decide(&mut s, &net, 0, &[left, ap(2, 6, 12)]),
            [false, true]
        );
        assert_eq!(decide(&mut s, &net, 1, &[left]), [true], "a gap");
        // On red it waits even with nobody else there.
        assert_eq!(decide(&mut s, &net, 300, &[ap(0, 5, 11)]), [false]);
        assert!(s.holds_at_red(&ap(0, 5, 11), 300));
    }

    #[test]
    fn a_partly_tagged_node_is_signalled_on_every_approach_but_split_links() {
        let net = node(None);
        let (first, inc) = incoming(&net);
        let plan = Plan::new(&net, &first, &inc, 0).unwrap();
        let phases: Vec<_> = (0..4).map(|r| plan.phase_of(r)).collect();
        assert_eq!(phases, [Some(0), Some(1), Some(0), Some(1)]);
        // Road 1 is a 20 m link from signalled node 2: no signal, and it
        // ranks below the signal arms.
        let net = node(Some(1));
        let (first, inc) = incoming(&net);
        let plan = Plan::new(&net, &first, &inc, 0).unwrap();
        let phases: Vec<_> = (0..4).map(|r| plan.phase_of(r)).collect();
        assert_eq!(phases, [Some(0), None, Some(0), Some(1)]);
        let mut s = signal(&net);
        // Road 1 (W-bound) straight crosses road 0 (N-bound) on green.
        assert_eq!(
            decide(&mut s, &net, 0, &[ap(0, 4, 10), ap(1, 5, 11)]),
            [true, false]
        );
        // An unsignalled node has no plan.
        let mut net = node(None);
        net.roads[0].end_control = EndControl::None;
        let (first, inc) = incoming(&net);
        assert!(Plan::new(&net, &first, &inc, 0).is_none());
    }

    #[test]
    fn lights_report_seconds_to_the_next_change() {
        let net = node(None);
        let s = signal(&net);
        let at = |tick| s.lights(tick).collect::<Vec<_>>();
        assert_eq!(
            at(0),
            [
                (0, Light::Green, 25.0),
                (1, Light::Red, 30.0),
                (2, Light::Green, 25.0),
                (3, Light::Red, 30.0)
            ]
        );
        assert_eq!(at(260)[0], (0, Light::Amber, 2.0));
        assert_eq!(at(390)[0], (0, Light::Red, 21.0));
        assert_eq!(at(390)[1], (1, Light::Green, 16.0));
    }
}
