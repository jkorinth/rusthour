//! Priority rule (M2.5, ADR-0004 §2): circulating > higher speed limit >
//! more lanes, then yield to the right; first arrival and a rotating
//! pointer break ties.

use super::{Approach, Controller};
use crate::StateHasher;
use crate::network::Network;

/// No slot, or no arrival yet.
const NONE: u32 = u32::MAX;
const NEVER: u64 = u64::MAX;

/// One incoming road's state.
#[derive(Debug, Clone, Copy)]
struct Arm {
    road: u32,
    /// Lane head seen last step.
    head: u32,
    /// Tick `head` first was arriving, or `NEVER`.
    since: u64,
    /// Slot that may pass whatever arrives, or `NONE`.
    released: u32,
}

/// Priority-rule state of one node.
#[derive(Debug, Clone)]
pub struct Priority {
    /// Ascending by road.
    arms: Vec<Arm>,
    /// Arm that wins the next tie in a deadlock.
    pointer: u32,
    /// Slot that won the last deadlock; it wins again while it is a
    /// candidate, so the pointer turns per vehicle served, not per tick.
    winner: u32,
}

impl Priority {
    /// A controller for a node whose incoming roads are `incoming`, ascending.
    #[must_use]
    pub fn new(incoming: &[u32]) -> Self {
        let arm = |&road| Arm {
            road,
            head: NONE,
            since: NEVER,
            released: NONE,
        };
        Self {
            arms: incoming.iter().map(arm).collect(),
            pointer: 0,
            winner: NONE,
        }
    }

    fn arm(&self, a: &Approach) -> usize {
        self.arms.partition_point(|arm| arm.road < a.from_road)
    }
}

/// Circulating > higher speed limit > more lanes.
fn class(net: &Network, a: &Approach) -> (bool, f64, u8) {
    let r = &net.roads[a.from_road as usize];
    (r.roundabout, r.speed_limit_mps, r.lanes)
}

/// Time-to-line test: `dist ≤ 3 s · v + 2·s0`.
fn arriving(a: &Approach) -> bool {
    a.dist_m <= 3.0 * a.speed_mps + 2.0 * a.min_gap_m
}

/// Arriving and able to enter: the heads others yield to.
fn competing(a: &Approach) -> bool {
    arriving(a) && !a.blocked
}

impl Controller for Priority {
    fn decide(&mut self, net: &Network, tick: u64, aps: &[Approach], go: &mut [bool]) {
        // Forget heads that left; note new arrivals.
        let mut rest = aps.iter().peekable();
        for arm in &mut self.arms {
            let Some(a) = rest.next_if(|a| a.from_road == arm.road) else {
                (arm.head, arm.since, arm.released) = (NONE, NEVER, NONE);
                continue;
            };
            if a.slot != arm.head {
                (arm.head, arm.since, arm.released) = (a.slot, NEVER, NONE);
            }
            if arm.since == NEVER && arriving(a) {
                arm.since = tick;
            }
        }
        debug_assert!(
            rest.next().is_none(),
            "approaches from roads not ending here"
        );

        let since = |i: usize| self.arms[self.arm(&aps[i])].since;
        // `i` yields to arriving `j`: a higher class, or the same class from
        // the right, or opposite/merging (|cross| ≤ 0.5) and there first.
        let yields = |i: usize, j: usize| {
            let (a, b) = (&aps[i], &aps[j]);
            if i == j || !competing(b) {
                return false;
            }
            match class(net, b).partial_cmp(&class(net, a)) {
                Some(std::cmp::Ordering::Greater) => true,
                Some(std::cmp::Ordering::Equal) => {
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
            *g = self.arms[self.arm(&aps[i])].released == aps[i].slot
                || !aps[i].blocked && !(0..aps.len()).any(|j| yields(i, j));
        }

        // Deadlock: every competing head yields. The top class's earliest
        // arrival goes, ties to the first arm from the rotating pointer.
        let stuck = (0..aps.len()).all(|i| !(go[i] && competing(&aps[i])));
        let top = (0..aps.len()).filter(|&i| competing(&aps[i])).filter(|&i| {
            !(0..aps.len()).any(|j| competing(&aps[j]) && class(net, &aps[j]) > class(net, &aps[i]))
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
            h.write_u64(u64::from(a.head));
            h.write_u64(a.since);
            h.write_u64(u64::from(a.released));
        }
        h.write_u64(u64::from(self.pointer));
        h.write_u64(u64::from(self.winner));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::Road;
    use proptest::prelude::*;

    /// Road `i` ends at node 0 heading N, W, S, E: road `i + 1` (mod 4)
    /// comes from road `i`'s right. Classes are `(roundabout, speed, lanes)`.
    fn node(classes: [(bool, f64, u8); 4]) -> Network {
        let dirs = [[0.0, 1.0], [-1.0, 0.0], [0.0, -1.0], [1.0, 0.0]];
        Network {
            nodes: vec![[0.0, 0.0]; 5],
            roads: (0..4)
                .map(|i| Road {
                    from: u32::try_from(i + 1).unwrap(),
                    to: 0,
                    length_m: 100.0,
                    speed_limit_mps: classes[i].1,
                    lanes: classes[i].2,
                    start_dir: dirs[i],
                    end_dir: dirs[i],
                    roundabout: classes[i].0,
                })
                .collect(),
            connections: Vec::new(),
        }
    }

    const MINOR: (bool, f64, u8) = (false, 8.3, 1);
    const MAJOR: (bool, f64, u8) = (false, 13.9, 1);

    /// A head on `road` with `s0 = 2` and `b = 2`.
    fn ap(road: u32, slot: u32, dist_m: f64, speed_mps: f64) -> Approach {
        Approach {
            from_road: road,
            to_road: 9,
            slot,
            dist_m,
            speed_mps,
            min_gap_m: 2.0,
            can_stop: dist_m >= speed_mps * speed_mps / 4.0,
            blocked: false,
        }
    }

    fn decide(p: &mut Priority, net: &Network, tick: u64, aps: &[Approach]) -> Vec<bool> {
        let mut go = vec![false; aps.len()];
        p.decide(net, tick, aps, &mut go);
        go
    }

    #[test]
    fn equal_roads_yield_to_the_right_and_opposites_to_the_first_arrival() {
        let net = node([MINOR; 4]);
        let mut p = Priority::new(&[0, 1, 2, 3]);
        let go = decide(&mut p, &net, 0, &[ap(0, 10, 2.0, 0.0), ap(1, 11, 2.0, 0.0)]);
        assert_eq!(go, [false, true], "road 1 is on road 0's right");

        let mut p = Priority::new(&[0, 1, 2, 3]);
        assert_eq!(decide(&mut p, &net, 0, &[ap(0, 10, 2.0, 0.0)]), [true]);
        let go = decide(&mut p, &net, 1, &[ap(0, 10, 2.0, 0.0), ap(2, 12, 2.0, 0.0)]);
        assert_eq!(go, [true, false], "road 0 arrived first");
    }

    #[test]
    fn a_deadlock_winner_keeps_go_until_it_leaves() {
        // Four equal heads arrive together and all yield to their right.
        let net = node([MINOR; 4]);
        let mut p = Priority::new(&[0, 1, 2, 3]);
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
        let mut p = Priority::new(&[0, 1, 2, 3]);
        // 8 m/s at 12 m: can't stop (needs 16 m), so `go` releases it.
        assert_eq!(decide(&mut p, &net, 0, &[ap(0, 1, 12.0, 8.0)]), [true]);
        let go = decide(&mut p, &net, 1, &[ap(0, 1, 11.2, 8.0), ap(3, 2, 6.0, 10.0)]);
        assert_eq!(go, [true, true]);

        // Still able to stop at 20 m: the major arrival takes `go` away.
        let mut p = Priority::new(&[0, 1, 2, 3]);
        assert_eq!(decide(&mut p, &net, 0, &[ap(0, 1, 20.0, 8.0)]), [true]);
        let go = decide(&mut p, &net, 1, &[ap(0, 1, 19.2, 8.0), ap(3, 2, 6.0, 10.0)]);
        assert_eq!(go, [false, true]);
    }

    fn class(net: &Network, a: &Approach) -> (bool, f64, u8) {
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
                prop::array::uniform4(prop::option::of((0..3u32, 0.0..60.0f64, 0.0..14.0f64, prop::bool::weighted(0.2)))),
                1..12,
            ),
        ) {
            let net = node(classes);
            let mut p = Priority::new(&[0, 1, 2, 3]);
            for (tick, heads) in (0u64..).zip(&ticks) {
                let aps: Vec<Approach> = (0..4u32)
                    .zip(heads)
                    .filter_map(|(r, h)| h.map(|(s, d, v, blocked)| Approach { blocked, ..ap(r, r * 4 + s, d, v) }))
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
