//! Road network: lanes, intersections, geometry, speed limits (M1).
//!
//! The static network the simulation runs on, built once at load time.
//! Items are addressed by their index (`u32`) in the respective list.
//! Callers (`rusthour-api`) validate the data first: the core assumes every
//! id is in range and every number is finite.

use crate::StateHasher;

/// A directed road between two intersections, all lanes in travel direction.
#[derive(Debug, Clone, PartialEq)]
pub struct Road {
    /// Intersection the road starts at.
    pub from: u32,
    /// Intersection the road ends at.
    pub to: u32,
    /// Length along the road's centerline, in meters.
    pub length_m: f64,
    /// Speed limit in meters per second.
    pub speed_limit_mps: f64,
    /// Number of lanes.
    pub lanes: u8,
    /// Unit vector `[x, y]` of the first geometry segment, the heading at the
    /// road's start.
    pub start_dir: [f64; 2],
    /// Unit vector `[x, y]` of the last geometry segment, the heading at the
    /// road's end.
    pub end_dir: [f64; 2],
    /// Part of a roundabout: traffic on it has priority over entering roads.
    pub roundabout: bool,
}

/// The static road network.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Network {
    /// Intersection positions `[x, y]` in meters.
    pub nodes: Vec<[f64; 2]>,
    /// Roads.
    pub roads: Vec<Road>,
    /// Allowed movements `(from_road, to_road)` at the intersection between them.
    pub connections: Vec<(u32, u32)>,
}

impl Network {
    /// Feeds the whole network into `h`.
    pub fn hash_into(&self, h: &mut StateHasher) {
        h.write_u64(self.nodes.len() as u64);
        for &[x, y] in &self.nodes {
            h.write_f64(x);
            h.write_f64(y);
        }
        h.write_u64(self.roads.len() as u64);
        for r in &self.roads {
            h.write_u64(u64::from(r.from));
            h.write_u64(u64::from(r.to));
            h.write_f64(r.length_m);
            h.write_f64(r.speed_limit_mps);
            h.write_u64(u64::from(r.lanes));
            for d in [r.start_dir, r.end_dir] {
                h.write_f64(d[0]);
                h.write_f64(d[1]);
            }
            h.write_u64(u64::from(r.roundabout));
        }
        h.write_u64(self.connections.len() as u64);
        for &(from, to) in &self.connections {
            h.write_u64(u64::from(from));
            h.write_u64(u64::from(to));
        }
    }
}
