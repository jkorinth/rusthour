//! Road offset to world position (ADR-0004 §3).

use std::collections::HashMap;

use crate::{Network, Point, RoadId};

/// Turns `(road, lane, offset)` into a world position and heading. Built once
/// from a [`Network`]; uses only api geometry, so it runs in wasm frontends.
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
    twin: bool,
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
                let twin = by_ends.get(&(r.to_node.0, r.from_node.0)).is_some_and(|c| {
                    c.iter().any(|&j| {
                        j != i && net.roads[j].geometry.iter().rev().eq(r.geometry.iter())
                    })
                });
                PlacedRoad {
                    points: r.geometry.clone(),
                    cum,
                    lanes: r.lanes,
                    twin,
                }
            })
            .collect();
        Self { roads }
    }

    /// World position and unit heading `[x, y]` of a vehicle's front bumper at
    /// `offset_m` on `lane` of `road`. The offset is clamped to the road. Lane 0 is rightmost; the
    /// lanes are `lane_width_m` wide.
    ///
    /// # Panics
    ///
    /// Panics if `road` is not in the network this was built from.
    #[must_use]
    pub fn place(
        &self,
        road: RoadId,
        lane: u8,
        offset_m: f32,
        lane_width_m: f32,
    ) -> (Point, [f32; 2]) {
        let pr = &self.roads[road.0 as usize];
        let length = pr.cum[pr.cum.len() - 1];
        let dist = f64::from(offset_m).clamp(0.0, length);
        // Segment i covers cum[i]..cum[i+1]; the last one also takes dist == length.
        let i = pr
            .cum
            .partition_point(|&c| c <= dist)
            .saturating_sub(1)
            .min(pr.points.len() - 2);
        let (p0, p1) = (pr.points[i], pr.points[i + 1]);
        let seg = pr.cum[i + 1] - pr.cum[i];
        let frac = (dist - pr.cum[i]) / seg;
        let (dx, dy) = ((p1.x - p0.x) / seg, (p1.y - p0.y) / seg);
        let lanes = f64::from(pr.lanes);
        let lane = f64::from(lane);
        let right = if pr.twin {
            lanes - lane - 0.5
        } else {
            (lanes - 1.0) / 2.0 - lane
        } * f64::from(lane_width_m);
        let pos = Point {
            x: p0.x + (p1.x - p0.x) * frac + dy * right,
            y: p0.y + (p1.y - p0.y) * frac - dx * right,
        };
        #[expect(clippy::cast_possible_truncation, reason = "heading for drawing")]
        (pos, [dx as f32, dy as f32])
    }
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
            let at = |o| p.place(RoadId(0), 0, o, 3.5).0;
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
        assert!((y(0) + 1.75).abs() < 1e-9 && (y(1) - 1.75).abs() < 1e-9);
        // With a twin, each direction keeps to its own side of the shared line.
        let twin = Placement::new(&net(vec![
            road(0, 1, vec![a, b], 1),
            road(1, 0, vec![b, a], 1),
        ]));
        let (east, _) = twin.place(RoadId(0), 0, 10.0, 3.5);
        let (west, head) = twin.place(RoadId(1), 0, 90.0, 3.5);
        assert!((east.y + 1.75).abs() < 1e-9 && (west.y - 1.75).abs() < 1e-9);
        assert!((west.x - east.x).abs() < 1e-9 && head[0] < 0.0);
    }

    #[test]
    fn offset_clamps_to_the_road() {
        let g = vec![pt(0.0, 0.0), pt(100.0, 0.0)];
        let p = Placement::new(&net(vec![road(0, 1, g, 1)]));
        assert!((p.place(RoadId(0), 0, 25.0, 3.5).0.x - 25.0).abs() < 1e-9);
        assert!((p.place(RoadId(0), 0, 999.0, 3.5).0.x - 100.0).abs() < 1e-9);
    }
}
