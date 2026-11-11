//! Movements: the connections `(from_road, to_road)` at a node, and their
//! turn types, derived from road directions at load (ADR-0005 §1).

use super::Network;

/// The kind of turn a movement makes, in right-hand traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// Within about 30° of straight on.
    Straight,
    /// A right turn.
    Right,
    /// A left turn with no oncoming road at the node.
    Left,
    /// A left turn across oncoming traffic.
    LeftAcross,
    /// A turn back of more than about 155°.
    UTurn,
}

impl Turn {
    /// The turn from road `from` onto road `to`, where `incoming` are the
    /// roads ending at `from`'s end node. Uses only `dot`, `cross` and
    /// comparisons (ADR-0004 float rules).
    #[must_use]
    pub fn of(net: &Network, from: u32, to: u32, incoming: &[u32]) -> Self {
        let a = net.roads[from as usize].end_dir;
        let b = net.roads[to as usize].start_dir;
        let (ab, cross) = (dot(a, b), a[0] * b[1] - a[1] * b[0]);
        if ab < -0.9 {
            Self::UTurn
        } else if cross.abs() <= 0.5 && ab > 0.0 {
            Self::Straight
        } else if cross <= 0.0 {
            Self::Right
        } else if (incoming.iter()).any(|&r| dot(net.roads[r as usize].end_dir, a) < -0.7) {
            Self::LeftAcross
        } else {
            Self::Left
        }
    }
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// Roads grouped by end node: node `n`'s incoming roads, ascending, are
/// `roads[first[n]..first[n + 1]]`. Returns `(first, roads)`.
///
/// # Panics
///
/// If the network has more than `u32::MAX` roads.
#[must_use]
pub fn incoming(net: &Network) -> (Vec<u32>, Vec<u32>) {
    by_node(net, |r| r.to)
}

/// Roads grouped by start node, as [`incoming`] does by end node.
///
/// # Panics
///
/// If the network has more than `u32::MAX` roads.
#[must_use]
pub fn outgoing(net: &Network) -> (Vec<u32>, Vec<u32>) {
    by_node(net, |r| r.from)
}

fn by_node(net: &Network, node: impl Fn(&super::Road) -> u32) -> (Vec<u32>, Vec<u32>) {
    let mut roads: Vec<u32> = (0..net.roads.len())
        .map(|r| u32::try_from(r).expect("road id fits u32"))
        .collect();
    roads.sort_by_key(|&r| (node(&net.roads[r as usize]), r));
    let first = (0..=net.nodes.len())
        .map(|n| {
            let before = roads.partition_point(|&r| (node(&net.roads[r as usize]) as usize) < n);
            u32::try_from(before).expect("road id fits u32")
        })
        .collect();
    (first, roads)
}

#[cfg(test)]
mod tests {
    use super::Turn::*;
    use super::*;
    use crate::network::{EndControl, Road};

    #[test]
    fn four_arm_turn_types() {
        // Roads 0..4 enter node 0 heading N, W, S, E; roads 4..8 leave it
        // heading N, W, S, E.
        let dirs = [[0.0, 1.0], [-1.0, 0.0], [0.0, -1.0], [1.0, 0.0]];
        let road = |from, to, d| Road {
            from,
            to,
            length_m: 100.0,
            speed_limit_mps: 10.0,
            lanes: 1,
            start_dir: d,
            end_dir: d,
            roundabout: false,
            class: 0,
            priority_road: false,
            end_control: EndControl::None,
            turn_lanes: vec![],
        };
        let mut net = Network {
            nodes: vec![[0.0, 0.0]; 2],
            roads: (dirs.iter().map(|&d| road(1, 0, d)))
                .chain(dirs.iter().map(|&d| road(0, 1, d)))
                .collect(),
            connections: Vec::new(),
        };
        let all = [0, 1, 2, 3];
        let turns = [4, 5, 6, 7].map(|to| Turn::of(&net, 0, to, &all));
        assert_eq!(turns, [Straight, LeftAcross, UTurn, Right]);
        // Without the oncoming road (2, heading S) the left turn is plain.
        assert_eq!(Turn::of(&net, 0, 5, &[0, 1, 3]), Left);
        // A slight bend still counts as straight.
        net.roads[4].start_dir = [0.4, 0.916_515];
        assert_eq!(Turn::of(&net, 0, 4, &all), Straight);
    }
}
