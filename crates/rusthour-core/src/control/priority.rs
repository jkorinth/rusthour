//! Priority rule v2 (M3.3, ADR-0005 §4): a road's rank comes from its
//! signs, `priority_road`, class, speed limit and lanes; among equals a left
//! turn yields to oncoming traffic, then right before left; first arrival
//! and a rotating pointer break ties. Only conflicting movements yield, and
//! a stop sign needs a full stop first.
//!
//! Known limit: a priority road that bends at a junction ("abknickende
//! Vorfahrt") is treated as two equal roads.

use super::{Approach, Controller};
use crate::StateHasher;
use crate::network::movement::Turn;
use crate::network::{EndControl, Network};

/// No slot, or no arrival yet.
const NONE: u32 = u32::MAX;
const NEVER: u64 = u64::MAX;
/// Below this speed a stop head has stood (m/s).
const STOOD_MPS: f64 = 0.1;

/// One incoming lane's state.
#[derive(Debug, Clone, Copy)]
struct Arm {
    road: u32,
    lane: u8,
    /// Position of the road's end on the node's circle (see [`Priority::new`]).
    point: u16,
    /// The road ends at a stop sign.
    stop_sign: bool,
    /// Lane head seen last step.
    head: u32,
    /// Tick `head` first was arriving, or `NEVER`.
    since: u64,
    /// Slot that may pass whatever arrives, or `NONE`.
    released: u32,
    /// `head` has stood at the line.
    stood: bool,
}

/// Priority-rule state of one node.
#[derive(Debug, Clone)]
pub struct Priority {
    /// Ascending by `(road, lane)`.
    arms: Vec<Arm>,
    /// `(road, point)` of the outgoing roads, ascending by road.
    exits: Vec<(u32, u16)>,
    /// Ticks after which a head waiting among equals is served first-come.
    patience: u64,
    /// Arm that wins the next tie in a deadlock.
    pointer: u32,
    /// Slot that won the last deadlock; it wins again while it is a
    /// candidate, so the pointer turns per vehicle served, not per tick.
    winner: u32,
}

impl Priority {
    /// A controller for a node whose incoming and outgoing roads are
    /// `incoming` and `outgoing`, both ascending.
    ///
    /// Road ends sit on a circle around the node (ADR-0005 §1): roads are
    /// grouped into arms by their neighbour node, arms are sorted
    /// counter-clockwise, and inside an arm the out-point comes first. Two
    /// movements conflict if their chords cross or they share a target.
    ///
    /// # Panics
    ///
    /// If the node has more than `u16::MAX` road ends.
    #[must_use]
    pub fn new(net: &Network, incoming: &[u32], outgoing: &[u32], patience: u64) -> Self {
        let roads = &net.roads;
        // An arm's angle: its outgoing road's start, else its incoming road's end reversed.
        let arm_angle = |neighbour: u32| {
            let out = outgoing
                .iter()
                .find(|&&r| roads[r as usize].to == neighbour);
            let d = if let Some(&r) = out {
                roads[r as usize].start_dir
            } else {
                let r = incoming
                    .iter()
                    .find(|&&r| roads[r as usize].from == neighbour);
                let d = roads[*r.expect("the neighbour has a road") as usize].end_dir;
                [-d[0], -d[1]]
            };
            pseudo_angle(d)
        };
        // `(angle, is_in, road)`; arms with equal angles also put out-points first.
        let mut points: Vec<(f64, bool, u32)> = (outgoing.iter())
            .map(|&r| (arm_angle(roads[r as usize].to), false, r))
            .chain(
                incoming
                    .iter()
                    .map(|&r| (arm_angle(roads[r as usize].from), true, r)),
            )
            .collect();
        points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let at = |is_in: bool, road: u32| {
            let i = points.iter().position(|&(_, i, r)| i == is_in && r == road);
            u16::try_from(i.expect("every road end is a point")).expect("node degree fits u16")
        };
        let mut arms = Vec::new();
        for &road in incoming {
            let r = &roads[road as usize];
            arms.extend((0..r.lanes).map(|lane| Arm {
                road,
                lane,
                point: at(true, road),
                stop_sign: r.end_control == EndControl::Stop,
                head: NONE,
                since: NEVER,
                released: NONE,
                stood: false,
            }));
        }
        Self {
            arms,
            exits: outgoing.iter().map(|&r| (r, at(false, r))).collect(),
            patience,
            pointer: 0,
            winner: NONE,
        }
    }

    fn arm(&self, a: &Approach) -> usize {
        (self.arms).partition_point(|arm| (arm.road, arm.lane) < (a.from_road, a.lane))
    }

    /// Whether `a` is held only because it must stand at a stop sign first.
    /// Valid after `decide` for the same approaches; for diagnostics.
    #[must_use]
    pub fn holds_at_stop_sign(&self, a: &Approach) -> bool {
        let arm = &self.arms[self.arm(a)];
        arm.stop_sign && !arm.stood
    }

    /// The movements of `a` and `b` cross or merge, that is, enter the same
    /// lane of one road (ADR-0005 §5). An unknown target conflicts with
    /// everything.
    fn conflict(&self, a: &Approach, b: &Approach) -> bool {
        let exit =
            |to| (self.exits.binary_search_by_key(&to, |&(r, _)| r).ok()).map(|i| self.exits[i].1);
        let (Some(a2), Some(b2)) = (exit(a.to_road), exit(b.to_road)) else {
            return true;
        };
        let (a1, b1) = (self.arms[self.arm(a)].point, self.arms[self.arm(b)].point);
        let (lo, hi) = (a1.min(a2), a1.max(a2));
        let inside = |p| lo < p && p < hi;
        (a2 == b2 && a.to_lane == b.to_lane) || a1 != b1 && inside(b1) != inside(b2)
    }
}

/// A monotone stand-in for the angle of unit vector `d`, in `[0, 4)`,
/// counter-clockwise from +x; no trigonometry (ADR-0004 float rules).
fn pseudo_angle(d: [f64; 2]) -> f64 {
    let p = d[1] / (d[0].abs() + d[1].abs());
    if d[0] < 0.0 {
        2.0 - p
    } else if d[1] < 0.0 {
        4.0 + p
    } else {
        p
    }
}

/// A road's rank at its end node, compared lexicographically, highest
/// first: circulating > no give-way or stop sign > priority road > class >
/// speed limit > lanes. Routing uses it for the yield prior of a movement
/// (ADR-0005 §2).
#[must_use]
pub fn rank(net: &Network, road: u32) -> (bool, bool, bool, u8, f64, u8) {
    let r = &net.roads[road as usize];
    let unsigned = !matches!(r.end_control, EndControl::GiveWay | EndControl::Stop);
    (
        r.roundabout,
        unsigned,
        r.priority_road,
        r.class,
        r.speed_limit_mps,
        r.lanes,
    )
}

fn class(net: &Network, a: &Approach) -> (bool, bool, bool, u8, f64, u8) {
    rank(net, a.from_road)
}

/// `a` turns left across `b`, which comes from the opposite direction.
fn left_across(net: &Network, a: &Approach, b: &Approach) -> bool {
    Turn::of(net, a.from_road, a.to_road, &[b.from_road]) == Turn::LeftAcross
}

/// `a` goes straight on or turns right.
fn through(net: &Network, a: &Approach) -> bool {
    matches!(
        Turn::of(net, a.from_road, a.to_road, &[]),
        Turn::Straight | Turn::Right
    )
}

/// Time-to-line test: `dist ≤ 3 s · v + 2·s0`.
fn arriving(a: &Approach) -> bool {
    a.dist_m <= 3.0 * a.speed_mps + 2.0 * a.min_gap_m
}

impl Controller for Priority {
    fn decide(&mut self, net: &Network, tick: u64, aps: &[Approach], go: &mut [bool]) {
        // Forget heads that left; note new arrivals and stop-sign stands.
        let mut rest = aps.iter().peekable();
        for arm in &mut self.arms {
            let Some(a) = rest.next_if(|a| (a.from_road, a.lane) == (arm.road, arm.lane)) else {
                (arm.head, arm.since, arm.released, arm.stood) = (NONE, NEVER, NONE, false);
                continue;
            };
            if a.slot != arm.head {
                (arm.head, arm.since, arm.released, arm.stood) = (a.slot, NEVER, NONE, false);
            }
            if arm.since == NEVER && arriving(a) {
                arm.since = tick;
            }
            arm.stood |= a.speed_mps < STOOD_MPS && a.dist_m <= 2.0 * a.min_gap_m;
        }
        debug_assert!(
            rest.next().is_none(),
            "approaches from lanes not ending here"
        );

        let arm = |i: usize| &self.arms[self.arm(&aps[i])];
        let since = |i: usize| arm(i).since;
        // The heads others yield to: arriving, able to enter, and past any stop sign.
        let competing = |i: usize| {
            let a = &aps[i];
            arriving(a) && !a.blocked && (!arm(i).stop_sign || arm(i).stood)
        };
        // `i` yields to competing `j` with a conflicting movement: a higher
        // rank; or the same rank and `i` turns left across `j` coming straight
        // or turning right; or `j` is on the right, or opposite/merging
        // (|cross| ≤ 0.5) and there first.
        let yields = |i: usize, j: usize| {
            let (a, b) = (&aps[i], &aps[j]);
            if i == j || !competing(j) || !self.conflict(a, b) {
                return false;
            }
            match class(net, b).partial_cmp(&class(net, a)) {
                Some(std::cmp::Ordering::Greater) => true,
                Some(std::cmp::Ordering::Equal) => {
                    // Impatience: past the limit, the earlier arrival goes.
                    let overdue = |k: usize| since(k) != NEVER && tick - since(k) >= self.patience;
                    if (overdue(i) || overdue(j)) && since(i) != since(j) {
                        return since(j) < since(i);
                    }
                    if left_across(net, a, b) && through(net, b) {
                        return true;
                    }
                    if left_across(net, b, a) && through(net, a) {
                        return false;
                    }
                    let (da, db) = (
                        net.roads[a.from_road as usize].end_dir,
                        net.roads[b.from_road as usize].end_dir,
                    );
                    let cross = da[0] * db[1] - da[1] * db[0];
                    cross > 0.5 || (cross >= -0.5 && since(j) <= since(i))
                }
                _ => false,
            }
        };
        for (i, g) in go.iter_mut().enumerate() {
            *g = arm(i).released == aps[i].slot
                || !aps[i].blocked && !(0..aps.len()).any(|j| yields(i, j));
        }

        // Deadlock: every competing head yields. The top class's earliest
        // arrival goes, ties to the first arm from the rotating pointer.
        let stuck = (0..aps.len()).all(|i| !(go[i] && competing(i)));
        let top = (0..aps.len()).filter(|&i| competing(i)).filter(|&i| {
            !(0..aps.len()).any(|j| competing(j) && class(net, &aps[j]) > class(net, &aps[i]))
        });
        let n = self.arms.len();
        let turn = |i: usize| (self.arm(&aps[i]) + n - self.pointer as usize) % n;
        let pick = top.min_by_key(|&i| (aps[i].slot != self.winner, since(i), turn(i)));
        if let Some(i) = pick.filter(|_| stuck) {
            go[i] = true;
            if aps[i].slot != self.winner {
                self.winner = aps[i].slot;
                self.pointer =
                    u32::try_from((self.arm(&aps[i]) + 1) % n).expect("arm count fits u32");
            }
        }

        // Stop gate, last: a stop head goes only once it has stood.
        for (a, g) in aps.iter().zip(go.iter_mut()) {
            *g &= !self.holds_at_stop_sign(a);
        }

        // Commitment: `go` that can no longer stop holds until it leaves.
        for (a, &g) in aps.iter().zip(go.iter()) {
            if g && !a.can_stop {
                let k = self.arm(a);
                self.arms[k].released = a.slot;
            }
        }
    }

    fn hash_into(&self, h: &mut StateHasher) {
        for a in &self.arms {
            h.write_u64(u64::from(a.road));
            h.write_u64(u64::from(a.lane));
            h.write_u64(u64::from(a.head));
            h.write_u64(a.since);
            h.write_u64(u64::from(a.released));
            h.write_u64(u64::from(a.stood));
        }
        h.write_u64(u64::from(self.pointer));
        h.write_u64(u64::from(self.winner));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{EndControl, Road};
    use proptest::prelude::*;

    /// A road's ranking fields: `(roundabout, speed, lanes)`.
    type Class = (bool, f64, u8);

    /// Roads 0..4 end at node 0 heading N, W, S, E, so road `i + 1` (mod 4)
    /// comes from road `i`'s right; roads 4..8 leave it heading N, W, S, E,
    /// each to the neighbour of its twin. Every movement is connected.
    fn node(classes: [Class; 4]) -> Network {
        let dirs = [[0.0, 1.0], [-1.0, 0.0], [0.0, -1.0], [1.0, 0.0]];
        // Neighbours 1..=4 lie S, E, N, W: where roads 0..4 come from.
        let road = |from, to, d: [f64; 2], c: Class| Road {
            from,
            to,
            length_m: 100.0,
            speed_limit_mps: c.1,
            lanes: c.2,
            start_dir: d,
            end_dir: d,
            roundabout: c.0,
            class: 0,
            priority_road: false,
            end_control: EndControl::None,
            turn_lanes: vec![],
        };
        let mut roads: Vec<Road> = (0..4)
            .map(|i| road(i + 1, 0, dirs[i as usize], classes[i as usize]))
            .collect();
        roads.extend((0..4).map(|i| road(0, (i + 2) % 4 + 1, dirs[i as usize], MINOR)));
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

    fn priority(net: &Network) -> Priority {
        Priority::new(net, &[0, 1, 2, 3], &[4, 5, 6, 7], 100)
    }

    const MINOR: Class = (false, 8.3, 1);
    const MAJOR: Class = (false, 13.9, 1);

    /// The road leaving node 0 in the direction road `i` arrives.
    fn straight(i: u32) -> u32 {
        4 + i
    }
    fn left(i: u32) -> u32 {
        4 + (i + 1) % 4
    }
    fn right(i: u32) -> u32 {
        4 + (i + 3) % 4
    }

    /// A head on `road` going straight, with `s0 = 2` and `b = 2`.
    fn ap(road: u32, slot: u32, dist_m: f64, speed_mps: f64) -> Approach {
        Approach {
            from_road: road,
            lane: 0,
            to_road: straight(road),
            to_lane: 0,
            slot,
            dist_m,
            speed_mps,
            min_gap_m: 2.0,
            can_stop: dist_m >= speed_mps * speed_mps / 4.0,
            blocked: false,
        }
    }

    fn to(a: Approach, to_road: u32) -> Approach {
        Approach { to_road, ..a }
    }

    fn decide(p: &mut Priority, net: &Network, tick: u64, aps: &[Approach]) -> Vec<bool> {
        let mut go = vec![false; aps.len()];
        p.decide(net, tick, aps, &mut go);
        go
    }

    #[test]
    fn equal_roads_yield_to_the_right_and_opposites_both_go() {
        let net = node([MINOR; 4]);
        let mut p = priority(&net);
        let go = decide(&mut p, &net, 0, &[ap(0, 10, 2.0, 0.0), ap(1, 11, 2.0, 0.0)]);
        assert_eq!(go, [false, true], "road 1 is on road 0's right");

        // Opposite straight movements don't conflict.
        let mut p = priority(&net);
        let go = decide(&mut p, &net, 0, &[ap(0, 10, 2.0, 0.0), ap(2, 12, 2.0, 0.0)]);
        assert_eq!(go, [true, true]);
    }

    #[test]
    fn heads_of_one_road_conflict_only_when_they_enter_the_same_lane() {
        let net = node([(false, 8.3, 2), MINOR, MINOR, MINOR]);
        let head = |lane, to_lane| Approach {
            lane,
            to_lane,
            ..ap(0, 10 + u32::from(lane), 2.0, 0.0)
        };
        // Side by side into different lanes: both go.
        let mut p = priority(&net);
        assert_eq!(
            decide(&mut p, &net, 0, &[head(0, 0), head(1, 1)]),
            [true, true]
        );
        // A lane drop: both enter lane 0, so one waits for the other, and
        // the one that goes keeps going until it has left.
        let mut p = priority(&net);
        let first = decide(&mut p, &net, 0, &[head(0, 0), head(1, 0)]);
        assert_eq!(first.iter().filter(|&&g| g).count(), 1, "{first:?}");
        assert_eq!(decide(&mut p, &net, 1, &[head(0, 0), head(1, 0)]), first);
    }

    #[test]
    fn a_left_turn_yields_to_oncoming_traffic_and_non_conflicting_movements_both_go() {
        let net = node([MINOR; 4]);
        // Road 0 was there first and turns left; road 2 comes straight on.
        let mut p = priority(&net);
        assert_eq!(
            decide(&mut p, &net, 0, &[to(ap(0, 10, 2.0, 0.0), left(0))]),
            [true]
        );
        let aps = [to(ap(0, 10, 2.0, 0.0), left(0)), ap(2, 12, 2.0, 0.0)];
        assert_eq!(decide(&mut p, &net, 1, &aps), [false, true]);
        // And to an oncoming right turn into the same road.
        let mut p = priority(&net);
        let aps = [
            to(ap(0, 10, 2.0, 0.0), left(0)),
            to(ap(2, 12, 2.0, 0.0), right(2)),
        ];
        assert_eq!(decide(&mut p, &net, 0, &aps), [false, true]);
        // A right turn crosses nothing; opposite left turns pass each other.
        let mut p = priority(&net);
        let aps = [to(ap(0, 10, 2.0, 0.0), right(0)), ap(2, 12, 2.0, 0.0)];
        assert_eq!(decide(&mut p, &net, 0, &aps), [true, true]);
        let aps = [
            to(ap(0, 10, 2.0, 0.0), left(0)),
            to(ap(2, 12, 2.0, 0.0), left(2)),
        ];
        assert_eq!(decide(&mut p, &net, 1, &aps), [true, true]);
    }

    #[test]
    fn signs_beat_priority_roads_which_beat_class_which_beats_speed() {
        // Road 1 is on road 0's right and faster.
        let mut net = node([MINOR, MAJOR, MINOR, MINOR]);
        let aps = [ap(0, 10, 2.0, 0.0), ap(1, 11, 2.0, 0.0)];
        net.roads[0].class = 5;
        net.roads[1].class = 3;
        assert_eq!(
            decide(&mut priority(&net), &net, 0, &aps),
            [true, false],
            "class beats speed"
        );
        net.roads[1].priority_road = true;
        assert_eq!(
            decide(&mut priority(&net), &net, 0, &aps),
            [false, true],
            "priority road beats class"
        );
        net.roads[1].end_control = EndControl::GiveWay;
        assert_eq!(
            decide(&mut priority(&net), &net, 0, &aps),
            [true, false],
            "a give-way sign beats all"
        );
    }

    #[test]
    fn a_stop_head_goes_only_after_standing_even_at_an_empty_node() {
        let mut net = node([MINOR; 4]);
        net.roads[0].end_control = EndControl::Stop;
        let mut p = priority(&net);
        // Rolling up to the line: no go, even though nobody else is there.
        for (tick, d) in (0..).zip([20.0, 10.0, 4.0]) {
            assert_eq!(
                decide(&mut p, &net, tick, &[ap(0, 10, d, 3.0)]),
                [false],
                "{d} m"
            );
        }
        assert_eq!(
            decide(&mut p, &net, 3, &[ap(0, 10, 3.0, 0.0)]),
            [true],
            "stood within 2·s0"
        );
        // Rolling on keeps go; the next head must stop again.
        assert_eq!(decide(&mut p, &net, 4, &[ap(0, 10, 2.5, 1.0)]), [true]);
        assert_eq!(decide(&mut p, &net, 5, &[ap(0, 11, 3.0, 2.0)]), [false]);
    }

    #[test]
    fn the_deadlock_rule_never_forces_an_unstood_stop_head() {
        // An all-way stop: four heads roll up together.
        let mut net = node([MINOR; 4]);
        for r in 0..4 {
            net.roads[r].end_control = EndControl::Stop;
        }
        let mut p = priority(&net);
        let heads = |d, v| [0, 1, 2, 3].map(|r| ap(r, 10 + r, d, v));
        for tick in 0..5 {
            assert_eq!(decide(&mut p, &net, tick, &heads(5.0, 2.0)), [false; 4]);
        }
        // Once all have stood, the deadlock rule lets exactly one go.
        let go = decide(&mut p, &net, 5, &heads(2.0, 0.0));
        assert_eq!(go.iter().filter(|&&g| g).count(), 1, "{go:?}");
    }

    #[test]
    fn a_deadlock_winner_keeps_go_until_it_leaves() {
        // Four equal heads arrive together and all yield to their right.
        let net = node([MINOR; 4]);
        let mut p = priority(&net);
        let heads = |d| [0, 1, 2, 3].map(|r| ap(r, 10 + r, d, 0.0));
        let first = decide(&mut p, &net, 0, &heads(2.0));
        assert_eq!(first.iter().filter(|&&g| g).count(), 1, "{first:?}");
        for tick in 1..50 {
            assert_eq!(
                decide(&mut p, &net, tick, &heads(2.0)),
                first,
                "tick {tick}"
            );
        }
        // Once the winner has left, another arm is served.
        let w = first.iter().position(|&g| g).unwrap();
        let rest: Vec<Approach> = (0..4).filter(|&i| i != w).map(|i| heads(2.0)[i]).collect();
        let go = decide(&mut p, &net, 50, &rest);
        assert_eq!(go.iter().filter(|&&g| g).count(), 1, "{go:?}");
    }

    #[test]
    fn a_released_vehicle_keeps_go_when_a_major_arrives() {
        // Road 3 (major) comes from road 0's left, so only class matters.
        let net = node([MINOR, MINOR, MINOR, MAJOR]);
        let mut p = priority(&net);
        // 8 m/s at 12 m: can't stop (needs 16 m), so `go` releases it.
        assert_eq!(decide(&mut p, &net, 0, &[ap(0, 1, 12.0, 8.0)]), [true]);
        let go = decide(&mut p, &net, 1, &[ap(0, 1, 11.2, 8.0), ap(3, 2, 6.0, 10.0)]);
        assert_eq!(go, [true, true]);

        // Still able to stop at 20 m: the major arrival takes `go` away.
        let mut p = priority(&net);
        assert_eq!(decide(&mut p, &net, 0, &[ap(0, 1, 20.0, 8.0)]), [true]);
        let go = decide(&mut p, &net, 1, &[ap(0, 1, 19.2, 8.0), ap(3, 2, 6.0, 10.0)]);
        assert_eq!(go, [false, true]);
    }

    fn class(net: &Network, a: &Approach) -> Class {
        let r = &net.roads[a.from_road as usize];
        (r.roundabout, r.speed_limit_mps, r.lanes)
    }

    fn arriving(a: &Approach) -> bool {
        a.dist_m <= 3.0 * a.speed_mps + 2.0 * a.min_gap_m && !a.blocked
    }

    proptest! {
        /// On every tick: if any unblocked head is arriving, one of them
        /// gets `go` (no deadlock), and an unblocked head told to stop
        /// always has such a head of at least its class (no major waits for
        /// a minor, nor for a head that can't enter).
        #[test]
        fn some_arrival_goes_and_no_head_waits_for_a_lower_class(
            classes in prop::array::uniform4((any::<bool>(), prop::sample::select(vec![8.3, 13.9]), 1..=2u8)),
            ticks in prop::collection::vec(
                prop::array::uniform4(prop::option::of((0..3u32, 0.0..60.0f64, 0.0..14.0f64, prop::bool::weighted(0.2), 0..3u32))),
                1..12,
            ),
        ) {
            let net = node(classes);
            let mut p = priority(&net);
            for (tick, heads) in (0u64..).zip(&ticks) {
                let aps: Vec<Approach> = (0..4u32)
                    .zip(heads)
                    .filter_map(|(r, h)| h.map(|(s, d, v, blocked, turn)| Approach {
                        blocked,
                        to_road: [straight(r), left(r), right(r)][turn as usize],
                        ..ap(r, r * 4 + s, d, v)
                    }))
                    .collect();
                let go = decide(&mut p, &net, tick, &aps);
                if aps.iter().any(arriving) {
                    prop_assert!(aps.iter().zip(&go).any(|(a, &g)| g && arriving(a)), "deadlock: {aps:?}");
                }
                for (a, _) in aps.iter().zip(&go).filter(|&(a, &g)| !g && !a.blocked) {
                    let held_by_equal_or_higher = aps.iter().any(|b| {
                        b.slot != a.slot && arriving(b) && class(&net, b) >= class(&net, a)
                    });
                    prop_assert!(held_by_equal_or_higher, "{a:?} held for a lower class: {aps:?} {go:?}");
                }
            }
        }
    }
}
