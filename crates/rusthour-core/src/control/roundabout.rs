//! Roundabout entries with gap acceptance (M5.2, ADR-0009).
//!
//! A ring node with an entry passes its approaches through an inner
//! [`Priority`], as on any node: circulating traffic ranks highest, entries
//! merging into one lane take turns, and committed heads keep `go`. On top,
//! an entry head into the ring goes only if the next circulating vehicle
//! reaches this node at least the driver's critical gap after the head
//! reaches the line, and then keeps `go`. Circulating vehicles that leave
//! the ring before this node, or at it, are not in the way. The follow-up
//! time emerges from car following (ADR-0009).

use super::priority::Priority;
use super::{Approach, Controller};
use crate::StateHasher;
use crate::network::Network;
use crate::vehicle::Vehicles;

/// How far upstream along the ring circulating vehicles are looked for (m):
/// 11 s at 50 km/h, beyond any critical gap plus a head's time to its line.
const LOOK_BACK_M: f64 = 150.0;
/// No vehicle.
const NONE: u32 = u32::MAX;

/// A ring node with at least one entry.
#[derive(Debug, Clone)]
pub struct Roundabout {
    inner: Priority,
    /// Ring roads upstream of this node, nearest first, up to
    /// [`LOOK_BACK_M`].
    upstream: Vec<u32>,
    /// Roads whose vehicles may reach this node on the ring: each with the
    /// distance from its end to this node (m), how many `upstream` roads a
    /// vehicle on it takes to get here, and whether it is an entry at an
    /// upstream ring node.
    watch: Vec<(u32, f64, usize, bool)>,
    /// The ring road out of this node; entries into it are gated.
    out: u32,
    /// Per entry lane, ascending by `(road, lane)`.
    lanes: Vec<EntryLane>,
}

#[derive(Debug, Clone, Copy)]
struct EntryLane {
    road: u32,
    lane: u8,
    /// The head seen last step.
    head: u32,
    /// Whether `head` has had `go`: it accepted a gap.
    went: bool,
}

impl Roundabout {
    /// The controller of the node where ring road `ring[k]` ends, for
    /// `ring` the roads of the ring in driving order, and that node's
    /// `incoming` and `outgoing` roads, both ascending. `ring` may be closed
    /// (the last road ends where the first starts) or an open chain. A
    /// roundabout made by an edit (M5.3) passes its new ring roads here.
    /// `patience` is in ticks (see [`Priority::new`]).
    ///
    /// # Panics
    ///
    /// If `ring[k]` is not among `incoming`.
    #[must_use]
    pub fn new(
        net: &Network,
        ring: &[u32],
        k: usize,
        incoming: &[u32],
        outgoing: &[u32],
        patience: u64,
    ) -> Self {
        assert!(incoming.contains(&ring[k]), "ring[k] ends at this node");
        let roads = &net.roads;
        let closed = roads[ring[ring.len() - 1] as usize].to == roads[ring[0] as usize].from;
        let node = roads[ring[k] as usize].to;
        let (mut upstream, mut watch) = (Vec::new(), Vec::new());
        let (mut i, mut offset) = (k, 0.0);
        loop {
            let j = upstream.len();
            upstream.push(ring[i]);
            watch.push((ring[i], offset, j, false));
            offset += roads[ring[i] as usize].length_m;
            if roads[ring[i] as usize].from != node {
                let feeds = (net.connections.iter())
                    .filter(|&&(from, to)| to == ring[i] && !roads[from as usize].roundabout);
                watch.extend(feeds.map(|&(from, _)| (from, offset, j + 1, true)));
            }
            i = match i {
                0 if closed => ring.len() - 1,
                0 => break,
                _ => i - 1,
            };
            if i == k || offset >= LOOK_BACK_M {
                break;
            }
        }
        let after = if closed { (k + 1) % ring.len() } else { k + 1 };
        let out = ring.get(after).copied().filter(|r| outgoing.contains(r));
        let lanes = (incoming.iter())
            .filter(|&&r| !roads[r as usize].roundabout)
            .flat_map(|&road| {
                (0..roads[road as usize].lanes).map(move |lane| EntryLane {
                    road,
                    lane,
                    head: NONE,
                    went: false,
                })
            })
            .collect();
        Self {
            inner: Priority::new(net, incoming, outgoing, patience),
            upstream,
            watch,
            out: out.unwrap_or(NONE),
            lanes,
        }
    }

    /// Seconds until the first vehicle that circulates past this node
    /// reaches it: on the ring, or entering it upstream and unable to stop
    /// any more; infinite if none within [`LOOK_BACK_M`].
    // shortcut: every lane of a multi-lane ring counts, as if each conflicted with the entry; per-lane conflicts when a map needs them.
    fn next_circulating_s(&self, net: &Network, vs: &Vehicles) -> f64 {
        let mut first = f64::INFINITY;
        let limit = net.roads[self.upstream[0] as usize].speed_limit_mps;
        for &(road, offset, k, entry) in &self.watch {
            let length_m = net.roads[road as usize].length_m;
            for lane in 0..net.roads[road as usize].lanes {
                for &slot in vs.lane(vs.lane_id(road, lane)) {
                    let slot = slot as usize;
                    let (p, v) = (&vs.params[slot], vs.speed_mps[slot]);
                    let dist_m = length_m - vs.pos_m[slot];
                    // An entrant that can still stop may yield; those behind it too.
                    if entry && dist_m >= v * v / (2.0 * p.comfort_decel_mps2) {
                        break;
                    }
                    // Its next roads: the ring roads to this node, then `out`.
                    let ahead = &vs.route[slot][vs.route_pos[slot] as usize + 1..];
                    let stays = ahead.len() > k
                        && (self.upstream[..k].iter().rev().zip(ahead)).all(|(u, n)| u == n)
                        && ahead[k] == self.out;
                    if stays {
                        let vmax = p.v0(limit).min(p.max_speed_mps);
                        // Faster than that, it brakes for the ring.
                        let t = arrival_s(dist_m + offset, v.min(vmax), p.max_accel_mps2, vmax);
                        first = first.min(t);
                    }
                }
            }
        }
        first
    }

    fn lane(&self, a: &Approach) -> Option<usize> {
        (self.lanes)
            .binary_search_by_key(&(a.from_road, a.lane), |l| (l.road, l.lane))
            .ok()
    }
}

/// Earliest time (s) to cover `d` metres from speed `v`, accelerating at
/// `a` up to `vmax`: an upper bound on how soon a vehicle can be there.
fn arrival_s(d: f64, v: f64, a: f64, vmax: f64) -> f64 {
    if d <= 0.0 {
        return 0.0;
    }
    if v >= vmax {
        return d / v;
    }
    let t1 = (vmax - v) / a;
    let d1 = f64::midpoint(v, vmax) * t1;
    if d >= d1 {
        t1 + (d - d1) / vmax
    } else {
        ((v * v + 2.0 * a * d).sqrt() - v) / a
    }
}

impl Controller for Roundabout {
    fn decide(
        &mut self,
        net: &Network,
        vs: &Vehicles,
        tick: u64,
        aps: &[Approach],
        go: &mut [bool],
    ) {
        // Forget heads that left.
        let mut rest = aps.iter().peekable();
        for l in &mut self.lanes {
            while (rest.next_if(|a| (a.from_road, a.lane) < (l.road, l.lane))).is_some() {}
            let head = (rest.next_if(|a| (a.from_road, a.lane) == (l.road, l.lane)))
                .map_or(NONE, |a| a.slot);
            if head != l.head {
                (l.head, l.went) = (head, false);
            }
        }
        self.inner.decide(net, vs, tick, aps, go);
        let mut circulating = None;
        for (a, g) in aps.iter().zip(go.iter_mut()) {
            let Some(lane) = self.lane(a) else {
                continue;
            };
            if a.to_road == self.out {
                if self.lanes[lane].went {
                    // Accepted a gap: it goes once there is room, ahead of
                    // the circulating traffic it saw coming, and even beside
                    // another committed entry (transfer orders the two).
                    *g = !a.blocked;
                } else if *g && a.can_stop {
                    // (A head that can't stop any more is committed.)
                    let slot = a.slot as usize;
                    let p = &vs.params[slot];
                    let limit = net.roads[a.from_road as usize].speed_limit_mps;
                    let vmax = p.v0(limit).min(p.max_speed_mps);
                    // Standing within `2·s0` of the line counts as at it.
                    let d = (a.dist_m - 2.0 * a.min_gap_m).max(0.0);
                    let to_line = arrival_s(d, a.speed_mps, p.max_accel_mps2, vmax);
                    let first =
                        *circulating.get_or_insert_with(|| self.next_circulating_s(net, vs));
                    // shortcut: no impatience, so a busy ring can starve an entry until the stuck net; let `tc` shrink with the wait if that shows (ADR-0009).
                    *g = first - to_line >= vs.gap[slot].critical_s;
                }
            }
            self.lanes[lane].went |= *g;
        }
    }

    fn hash_into(&self, h: &mut StateHasher) {
        self.inner.hash_into(h);
        for l in &self.lanes {
            h.write_u64(u64::from(l.head));
            h.write_u64(u64::from(l.went));
        }
    }

    fn holds_at_stop_sign(&self, a: &Approach) -> bool {
        self.inner.holds_at_stop_sign(a)
    }
}

/// The ring through `node` in driving order, ending with the ring road into
/// it, if `node` is a ring node with an entry: one incoming road flagged
/// roundabout and at least one that is not. Followed upstream while each
/// ring node has one incoming ring road, so a ring the map breaks off
/// becomes an open chain. `in_first` and `incoming` are
/// [`crate::network::movement::incoming`].
#[must_use]
pub fn ring_into(
    net: &Network,
    in_first: &[u32],
    incoming: &[u32],
    node: usize,
) -> Option<Vec<u32>> {
    let into = |n: usize| &incoming[in_first[n] as usize..in_first[n + 1] as usize];
    let ring_road = |n: usize| {
        let mut ring = into(n)
            .iter()
            .filter(|&&r| net.roads[r as usize].roundabout);
        ring.next().copied().filter(|_| ring.next().is_none())
    };
    let first = ring_road(node)?;
    if into(node).iter().all(|&r| net.roads[r as usize].roundabout) {
        return None;
    }
    let mut ring = vec![first];
    while let Some(r) = ring_road(net.roads[ring[ring.len() - 1] as usize].from as usize) {
        if r == first || ring.len() == net.roads.len() {
            break;
        }
        ring.push(r);
    }
    ring.reverse();
    Some(ring)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behavior::profile::Profile;
    use crate::network::{EndControl, Road};
    use crate::vehicle::NewVehicle;
    use crate::vehicle::class::Class;

    /// A piece of ring, west to east: road 0 (40 m) to exit node 1, road 1
    /// (10 m) to entry node 2, road 2 (40 m) on. Road 3 leaves at node 1
    /// to the south, road 4 enters at node 2 from the south, road 5 at
    /// node 1 from the north.
    fn ring_piece() -> Network {
        let road = |from, to, length_m, d: [f64; 2], roundabout| Road {
            from,
            to,
            length_m,
            speed_limit_mps: 8.3,
            lanes: 1,
            start_dir: d,
            end_dir: d,
            roundabout,
            class: 0,
            priority_road: false,
            end_control: if roundabout {
                EndControl::None
            } else {
                EndControl::GiveWay
            },
            turn_lanes: vec![],
        };
        let (east, south, north) = ([1.0, 0.0], [0.0, -1.0], [0.0, 1.0]);
        Network {
            nodes: vec![
                [-40.0, 0.0],
                [0.0, 0.0],
                [10.0, 0.0],
                [50.0, 0.0],
                [0.0, -50.0],
                [10.0, -50.0],
                [0.0, 50.0],
            ],
            roads: vec![
                road(0, 1, 40.0, east, true),
                road(1, 2, 10.0, east, true),
                road(2, 3, 40.0, east, true),
                road(1, 4, 50.0, south, false),
                road(5, 2, 50.0, north, false),
                road(6, 1, 50.0, south, false),
            ],
            connections: vec![(0, 1), (0, 3), (1, 2), (4, 2), (5, 1)],
        }
    }

    fn put(vs: &mut Vehicles, route: &[u32], pos_m: f64, speed_mps: f64) -> u32 {
        let d = Profile::Normal.base();
        vs.spawn(NewVehicle {
            road: route[0],
            lane: 0,
            pos_m,
            speed_mps,
            params: d.idm,
            mobil: d.mobil,
            gap: d.gap,
            profile: Profile::Normal,
            class: Class::Car,
            route,
            depart_tick: 0,
        })
        .slot
    }

    /// Whether a car standing at road 4's line gets `go` while another
    /// follows `route` at `pos_m` on its first road at 8 m/s.
    fn entry_goes(route: &[u32], pos_m: f64) -> bool {
        let net = ring_piece();
        let mut vs = Vehicles::new(&net);
        put(&mut vs, route, pos_m, 8.0);
        let head = put(&mut vs, &[4, 2], 48.0, 0.0);
        let mut c = Roundabout::new(&net, &[0, 1, 2], 1, &[1, 4], &[2], 100);
        assert_eq!(c.watch.len(), 3, "roads 1, 0 and the entry 5");
        let a = Approach {
            from_road: 4,
            lane: 0,
            to_road: 2,
            to_lane: 0,
            slot: head,
            dist_m: 2.0,
            speed_mps: 0.0,
            min_gap_m: 2.0,
            can_stop: true,
            blocked: false,
        };
        let mut go = [false];
        c.decide(&net, &vs, 0, &[a], &mut go);
        go[0]
    }

    #[test]
    fn a_car_exiting_before_the_entry_does_not_block_it() {
        // 10 m before node 1.
        assert!(entry_goes(&[0, 3], 30.0), "it leaves at node 1");
        assert!(
            !entry_goes(&[0, 1, 2], 30.0),
            "2.5 s away, it stays on the ring"
        );
    }

    #[test]
    fn a_car_entering_upstream_counts_once_it_cannot_stop() {
        // 8 m/s needs 16 m to stop at 2 m/s².
        assert!(
            !entry_goes(&[5, 1, 2], 40.0),
            "10 m before node 1, committed"
        );
        assert!(
            entry_goes(&[5, 1, 2], 20.0),
            "30 m before node 1, it may yield"
        );
    }

    #[test]
    fn the_ring_of_an_entry_node_is_found_closed_or_open() {
        let net = ring_piece();
        let (first, incoming) = crate::network::movement::incoming(&net);
        assert_eq!(ring_into(&net, &first, &incoming, 2), Some(vec![0, 1]));
        assert_eq!(
            ring_into(&net, &first, &incoming, 1),
            Some(vec![0]),
            "an open chain"
        );
        assert_eq!(
            ring_into(&net, &first, &incoming, 0),
            None,
            "no ring road in"
        );
    }
}
