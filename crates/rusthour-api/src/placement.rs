//! Road offset to world position (ADR-0004 §3).

use std::collections::HashMap;

use crate::{Network, Point, RoadId, Turns};

/// A median that left-turn pockets need is kept this far into the roads
/// that continue a twin pair straight on…
const HOLD_M: f64 = 10.0;
/// …and then tapers off over this length. Shorter roads taper end to end.
const TAPER_M: f64 = 30.0;
/// Most a road may turn from the one before it and still continue it.
const STRAIGHT_COS: f64 = 0.866; // cos 30°

/// Turns `(road, lane, offset)` into a world position and heading. Built once
/// from a [`Network`]; uses only api geometry, so it runs in wasm frontends.
///
/// A road with a twin (the other direction on the same geometry) keeps its
/// lanes right of the shared line. Its left-turn pockets, the leftmost lanes
/// that allow neither through nor right turns, take a median between the two
/// directions instead of pushing the through lanes outward, so the through
/// lanes run on straight across a street's alternating pockets. Roads
/// continuing the pair straight on keep that median for a while, then taper
/// it off.
#[derive(Debug, Clone)]
pub struct Placement {
    roads: Vec<PlacedRoad>,
}

#[derive(Debug, Clone)]
struct PlacedRoad {
    points: Vec<Point>,
    /// Cumulative length at each point; `cum[0] == 0`.
    cum: Vec<f64>,
    lanes: u8,
    /// How far the middle of the lanes lies right of `points`, in lanes, as
    /// knots `(offset_m, lanes)` interpolated linearly; one knot if constant.
    center: Vec<(f64, f64)>,
}

impl PlacedRoad {
    /// Position and unit direction at `dist` along the road.
    fn at(&self, dist: f64) -> (Point, (f64, f64)) {
        // Segment i covers cum[i]..cum[i+1]; the last one also takes dist == length.
        let i = self
            .cum
            .partition_point(|&c| c <= dist)
            .saturating_sub(1)
            .min(self.points.len() - 2);
        let (p0, p1) = (self.points[i], self.points[i + 1]);
        let seg = self.cum[i + 1] - self.cum[i];
        let frac = (dist - self.cum[i]) / seg;
        let pos = Point {
            x: p0.x + (p1.x - p0.x) * frac,
            y: p0.y + (p1.y - p0.y) * frac,
        };
        (pos, ((p1.x - p0.x) / seg, (p1.y - p0.y) / seg))
    }

    /// [`Self::center`] at `dist`.
    fn center_at(&self, dist: f64) -> f64 {
        let k = self.center.partition_point(|&(s, _)| s <= dist);
        match (k.checked_sub(1).map(|j| self.center[j]), self.center.get(k)) {
            (Some((s0, c0)), Some(&(s1, c1))) => c0 + (c1 - c0) * (dist - s0) / (s1 - s0),
            (Some((_, c)), None) | (None, Some(&(_, c))) => c,
            (None, None) => 0.0,
        }
    }

    fn length(&self) -> f64 {
        self.cum[self.cum.len() - 1]
    }
}

/// Unit direction from `a` to `b`.
fn unit(a: Point, b: Point) -> (f64, f64) {
    let len = (b.x - a.x).hypot(b.y - a.y);
    ((b.x - a.x) / len, (b.y - a.y) / len)
}

impl Placement {
    /// Builds the lookup. Offsets are measured along each road's polyline.
    ///
    /// The network must have passed validation (`to_core`): every road needs
    /// at least two distinct points, or [`place`](Self::place) can panic or
    /// return NaN.
    #[must_use]
    pub fn new(net: &Network) -> Self {
        // Twin: another road with the same ends swapped and reversed geometry.
        let mut by_ends: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, r) in net.roads.iter().enumerate() {
            by_ends
                .entry((r.from_node.0, r.to_node.0))
                .or_default()
                .push(i);
        }
        let twin: Vec<Option<usize>> = net
            .roads
            .iter()
            .enumerate()
            .map(|(i, r)| {
                by_ends
                    .get(&(r.to_node.0, r.from_node.0))?
                    .iter()
                    .copied()
                    .find(|&j| j != i && net.roads[j].geometry.iter().rev().eq(r.geometry.iter()))
            })
            .collect();
        // Left-turn pockets: leftmost lanes with neither through nor right turns.
        let pockets: Vec<f64> = net
            .roads
            .iter()
            .zip(&twin)
            .map(|(r, t)| {
                let pocket = |l: &&Turns| l.0 & (Turns::THROUGH.0 | Turns::RIGHT.0) == 0;
                let n = r.turn_lanes.iter().rev().take_while(pocket).count();
                let n = u8::try_from(n).unwrap_or(u8::MAX).min(r.lanes - 1);
                if t.is_some() { f64::from(n) } else { 0.0 }
            })
            .collect();
        let median: Vec<f64> = (0..net.roads.len())
            .map(|i| twin[i].map_or(0.0, |t| pockets[i] + pockets[t]))
            .collect();
        let (before, after) = straight_on(net, &twin);

        let roads = net
            .roads
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let mut cum = vec![0.0];
                for w in r.geometry.windows(2) {
                    let last = cum[cum.len() - 1];
                    cum.push(last + (w[1].x - w[0].x).hypot(w[1].y - w[0].y));
                }
                let len = cum[cum.len() - 1];
                let center = if twin[i].is_some() {
                    let m = median[i];
                    let ends = |n: Option<usize>| n.map_or(m, |n| m.max(median[n]));
                    let (start, end) = (ends(before[i]), ends(after[i]));
                    let c = |median: f64| f64::from(r.lanes) / 2.0 - pockets[i] + median / 2.0;
                    if start.max(end) <= m {
                        vec![(0.0, c(m))]
                    } else if len >= 2.0 * (HOLD_M + TAPER_M) {
                        vec![
                            (0.0, c(start)),
                            (HOLD_M, c(start)),
                            (HOLD_M + TAPER_M, c(m)),
                            (len - HOLD_M - TAPER_M, c(m)),
                            (len - HOLD_M, c(end)),
                            (len, c(end)),
                        ]
                    } else {
                        vec![(0.0, c(start)), (len, c(end))]
                    }
                } else {
                    vec![(0.0, 0.0)]
                };
                PlacedRoad {
                    points: r.geometry.clone(),
                    cum,
                    lanes: r.lanes,
                    center,
                }
            })
            .collect();
        Self { roads }
    }

    /// The points of `road`'s geometry, plus where its lanes start or stop
    /// moving sideways, each with how far the middle of its lanes lies right
    /// of it in meters. Frontends draw the road along these, as
    /// [`place`](Self::place) puts its lanes.
    ///
    /// # Panics
    ///
    /// Panics if `road` is not in the network this was built from.
    #[must_use]
    pub fn center_line(&self, road: RoadId, lane_width_m: f32) -> Vec<(Point, f64)> {
        let pr = &self.roads[road.0 as usize];
        let len = pr.length();
        let mut offsets = pr.cum.clone();
        offsets.extend(
            pr.center
                .iter()
                .map(|&(s, _)| s)
                .filter(|&s| s > 0.0 && s < len),
        );
        offsets.sort_by(f64::total_cmp);
        offsets.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        offsets
            .into_iter()
            .map(|s| (pr.at(s).0, pr.center_at(s) * f64::from(lane_width_m)))
            .collect()
    }

    /// World position and unit heading `[x, y]` of a vehicle's front bumper at
    /// `offset_m` on `lane` of `road`. The offset is clamped to the road. Lane 0 is rightmost; the
    /// lanes are `lane_width_m` wide. `lane` may be fractional, as in
    /// `lane + lane_shift` during a lane change.
    ///
    /// # Panics
    ///
    /// Panics if `road` is not in the network this was built from.
    #[must_use]
    pub fn place(
        &self,
        road: RoadId,
        lane: f32,
        offset_m: f32,
        lane_width_m: f32,
    ) -> (Point, [f32; 2]) {
        let pr = &self.roads[road.0 as usize];
        let dist = f64::from(offset_m).clamp(0.0, pr.length());
        let (p, (dx, dy)) = pr.at(dist);
        let right = (pr.center_at(dist) + (f64::from(pr.lanes) - 1.0) / 2.0 - f64::from(lane))
            * f64::from(lane_width_m);
        let pos = Point {
            x: p.x + dy * right,
            y: p.y - dx * right,
        };
        #[expect(clippy::cast_possible_truncation, reason = "heading for drawing")]
        (pos, [dx as f32, dy as f32])
    }
}

/// For each road with a twin, the twin road it continues straight on from at
/// its start and to at its end, if both are each other's straightest choice.
fn straight_on(net: &Network, twin: &[Option<usize>]) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let n = net.roads.len();
    let mut ins = vec![Vec::new(); net.nodes.len()];
    let mut outs = vec![Vec::new(); net.nodes.len()];
    for (i, r) in net.roads.iter().enumerate() {
        if twin[i].is_some() {
            ins[r.to_node.0 as usize].push(i);
            outs[r.from_node.0 as usize].push(i);
        }
    }
    let end_dir = |i: usize| {
        let g = &net.roads[i].geometry;
        unit(g[g.len() - 2], g[g.len() - 1])
    };
    let start_dir = |i: usize| {
        let g = &net.roads[i].geometry;
        unit(g[0], g[1])
    };
    let cos = |a: usize, b: usize| {
        let (u, v) = (end_dir(a), start_dir(b));
        u.0 * v.0 + u.1 * v.1
    };
    // The straightest candidate, if straight enough; never the road's own twin.
    let best = |from: usize, candidates: &[usize], score: &dyn Fn(usize) -> f64| {
        candidates
            .iter()
            .copied()
            .filter(|&c| twin[from] != Some(c) && score(c) >= STRAIGHT_COS)
            .max_by(|&a, &b| score(a).total_cmp(&score(b)))
    };
    let (mut before, mut after) = (vec![None; n], vec![None; n]);
    for (ins, outs) in ins.iter().zip(&outs) {
        for &a in ins {
            let Some(b) = best(a, outs, &|b| cos(a, b)) else {
                continue;
            };
            if best(b, ins, &|a| cos(a, b)) == Some(a) {
                after[a] = Some(b);
                before[b] = Some(a);
            }
        }
    }
    (before, after)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::{Node, NodeId, Road};

    fn road(from: u32, to: u32, geometry: Vec<Point>, lanes: u8) -> Road {
        Road {
            from_node: NodeId(from),
            to_node: NodeId(to),
            geometry,
            speed_limit_mps: 13.9,
            lanes,
            name: None,
            osm: None,
            roundabout: false,
            class: crate::RoadClass::Unknown,
            priority_road: false,
            end_control: crate::EndControl::None,
            turn_lanes: vec![],
        }
    }

    fn net(roads: Vec<Road>) -> Network {
        let node = Node {
            position: Point { x: 0.0, y: 0.0 },
            osm_node: None,
        };
        Network {
            nodes: vec![node.clone(), node],
            roads,
            connections: vec![],
        }
    }

    fn pt(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    proptest! {
        // x grows along the polyline, so positions must too.
        #[test]
        fn ends_and_monotone(steps in prop::collection::vec((1.0f64..50.0, -50.0f64..50.0), 1..8)) {
            let mut g = vec![pt(0.0, 0.0)];
            for (dx, dy) in &steps {
                let l = g[g.len() - 1];
                g.push(pt(l.x + dx, l.y + dy));
            }
            let p = Placement::new(&net(vec![road(0, 1, g.clone(), 1)]));
            #[expect(clippy::cast_possible_truncation)]
            let len = p.roads[0].cum[g.len() - 1] as f32;
            let at = |o| p.place(RoadId(0), 0.0, o, 3.5).0;
            prop_assert!((at(0.0).x - g[0].x).abs() < 1e-3 && (at(0.0).y - g[0].y).abs() < 1e-3);
            let end = g[g.len() - 1];
            prop_assert!((at(len).x - end.x).abs() < 1e-2 && (at(len).y - end.y).abs() < 1e-2);
            let mut last = f64::MIN;
            for k in 0..=20u8 {
                let x = at(len * f32::from(k) / 20.0).x;
                prop_assert!(x >= last - 1e-6);
                last = x;
            }
        }
    }

    #[test]
    fn lanes_lie_right_and_twins_do_not_overlap() {
        let (a, b) = (pt(0.0, 0.0), pt(100.0, 0.0));
        // Alone, heading east, two lanes: lane 0 south (right) of lane 1.
        let solo = Placement::new(&net(vec![road(0, 1, vec![a, b], 2)]));
        let y = |l| solo.place(RoadId(0), l, 50.0, 3.5).0.y;
        assert!((y(0.0) + 1.75).abs() < 1e-9 && (y(1.0) - 1.75).abs() < 1e-9);
        assert!(y(0.5).abs() < 1e-9, "halfway through a lane change");
        // With a twin, each direction keeps to its own side of the shared line.
        let twin = Placement::new(&net(vec![
            road(0, 1, vec![a, b], 1),
            road(1, 0, vec![b, a], 1),
        ]));
        let (east, _) = twin.place(RoadId(0), 0.0, 10.0, 3.5);
        let (west, head) = twin.place(RoadId(1), 0.0, 90.0, 3.5);
        assert!((east.y + 1.75).abs() < 1e-9 && (west.y - 1.75).abs() < 1e-9);
        assert!((west.x - east.x).abs() < 1e-9 && head[0] < 0.0);
        // Drawn as placed: the middle of a twin's one lane, half a lane right.
        assert_eq!(twin.center_line(RoadId(1), 3.5), [(b, 1.75), (a, 1.75)]);
        assert_eq!(solo.center_line(RoadId(0), 3.5), [(a, 0.0), (b, 0.0)]);
    }

    /// An east-west street of twin pairs between nodes at `xs` (`y` = 0),
    /// each direction with `(lanes, turn_lanes)` per pair, west to east, plus
    /// a one-lane pair going north from the last node.
    fn street(xs: &[f64], east: &[(u8, Vec<Turns>)], west: &[(u8, Vec<Turns>)]) -> Network {
        let mut roads = Vec::new();
        for (k, w) in xs.windows(2).enumerate() {
            let (from, to) = (pt(w[0], 0.0), pt(w[1], 0.0));
            let node = u32::try_from(k).unwrap();
            let mut east_road = road(node, node + 1, vec![from, to], east[k].0);
            east_road.turn_lanes.clone_from(&east[k].1);
            let mut west_road = road(node + 1, node, vec![to, from], west[k].0);
            west_road.turn_lanes.clone_from(&west[k].1);
            roads.extend([east_road, west_road]);
        }
        let last = u32::try_from(xs.len() - 1).unwrap();
        let (end, north) = (pt(xs[xs.len() - 1], 0.0), pt(xs[xs.len() - 1], 100.0));
        roads.push(road(last, last + 1, vec![end, north], 1));
        roads.push(road(last + 1, last, vec![north, end], 1));
        let node = |x, y| Node {
            position: pt(x, y),
            osm_node: None,
        };
        let mut nodes: Vec<_> = xs.iter().map(|&x| node(x, 0.0)).collect();
        nodes.push(node(xs[xs.len() - 1], 100.0));
        Network {
            nodes,
            roads,
            connections: vec![],
        }
    }

    #[test]
    fn through_lanes_run_straight_across_left_turn_pockets() {
        let pocket = (2, vec![Turns::THROUGH, Turns::LEFT]);
        let one = (1, vec![]);
        // Pockets alternate: east-bound on 0..40, west-bound on 40..80; 80..200
        // carries on, then a turn north.
        let net = street(
            &[0.0, 40.0, 80.0, 200.0],
            &[pocket.clone(), one.clone(), one.clone()],
            &[one.clone(), pocket, one],
        );
        let p = Placement::new(&net);
        let y = |road, lane, offset| p.place(RoadId(road), lane, offset, 3.5).0.y;
        // East-bound through lane (lane 0): one lane south of the shared line
        // throughout, half a lane further out than without a median.
        for (road, offset) in [(0, 0.0), (0, 40.0), (2, 0.0), (2, 40.0), (4, 10.0)] {
            assert!(
                (y(road, 0.0, offset) + 3.5).abs() < 1e-9,
                "road {road} at {offset}"
            );
        }
        // Its pocket sits on the shared line, the west-bound one likewise.
        assert!(y(0, 1.0, 20.0).abs() < 1e-9);
        assert!(y(3, 1.0, 20.0).abs() < 1e-9 && (y(1, 0.0, 20.0) - 3.5).abs() < 1e-9);
        // The median tapers off along the long road, ending as without pockets.
        assert!(
            (y(4, 0.0, 25.0) + 2.625).abs() < 1e-9,
            "halfway down the taper"
        );
        assert!((y(4, 0.0, 120.0) + 1.75).abs() < 1e-9);
        // The road north is no straight continuation: no median there.
        let (north, _) = p.place(RoadId(6), 0.0, 0.0, 3.5);
        assert!((north.x - 201.75).abs() < 1e-9, "{north:?}");
        // Drawn as placed, with points where the taper bends.
        let line = p.center_line(RoadId(4), 3.5);
        let xs: Vec<f64> = line.iter().map(|(q, _)| q.x).collect();
        assert!(xs.contains(&90.0) && xs.contains(&120.0), "{xs:?}");
        for (q, d) in line {
            #[expect(clippy::cast_possible_truncation)]
            let o = (q.x - 80.0) as f32;
            assert!((y(4, 0.0, o) + d).abs() < 1e-6, "at {q:?}");
        }
        // A right-turn pocket at the kerb widens the road outward instead.
        let kerb = street(
            &[0.0, 40.0],
            &[(2, vec![Turns::RIGHT, Turns::THROUGH])],
            &[(1, vec![])],
        );
        let p = Placement::new(&kerb);
        let through = p.place(RoadId(0), 1.0, 20.0, 3.5).0.y;
        assert!((through + 1.75).abs() < 1e-9, "{through}");
    }

    #[test]
    fn offset_clamps_to_the_road() {
        let g = vec![pt(0.0, 0.0), pt(100.0, 0.0)];
        let p = Placement::new(&net(vec![road(0, 1, g, 1)]));
        assert!((p.place(RoadId(0), 0.0, 25.0, 3.5).0.x - 25.0).abs() < 1e-9);
        assert!((p.place(RoadId(0), 0.0, 999.0, 3.5).0.x - 100.0).abs() < 1e-9);
    }
}
