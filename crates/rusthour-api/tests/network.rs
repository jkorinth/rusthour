//! Network loading and geometry query (M1.4), as property tests.

use proptest::prelude::*;
use proptest::sample::{Index, subsequence};
use rusthour_api::{
    Config, Connection, InProcess, Network, NetworkError, Node, NodeId, Point, Road, RoadId,
    Simulation,
};

/// Node `i` sits in its own 250 m grid cell (jitter < half a cell), so node
/// positions are always distinct.
fn node(i: usize, (jx, jy): (f64, f64)) -> Node {
    #[allow(clippy::cast_precision_loss)]
    let (col, row) = ((i % 8) as f64, (i / 8) as f64);
    Node {
        position: Point {
            x: col * 250.0 + jx,
            y: row * 250.0 + jy,
        },
        osm_node: None,
    }
}

/// Road polyline from `a` to `b`: points at evenly spaced steps along a→b,
/// each pushed sideways by an offset. Distinct steps along the direction mean
/// no two consecutive points coincide.
fn polyline(a: Point, b: Point, offsets: &[f64]) -> Vec<Point> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    let (nx, ny) = (-dy / len, dx / len);
    #[allow(clippy::cast_precision_loss)]
    let steps = (offsets.len() - 1) as f64;
    offsets
        .iter()
        .enumerate()
        .map(|(k, o)| {
            #[allow(clippy::cast_precision_loss)]
            let t = k as f64 / steps;
            Point {
                x: a.x + dx * t + nx * o,
                y: a.y + dy * t + ny * o,
            }
        })
        .collect()
}

/// A valid network: 2–30 nodes, 1–40 roads between distinct nodes, and a
/// subset of the movements between roads that meet at a node.
fn valid_network() -> impl Strategy<Value = Network> {
    let jitter = (-100.0..100.0, -100.0..100.0);
    prop::collection::vec(jitter, 2..30)
        .prop_flat_map(|jitters| {
            let n = jitters.len();
            let road = (
                0..n,
                1..n,
                prop::collection::vec(-30.0..30.0, 2..6),
                0.5..40.0,
                1u8..5,
            );
            (Just(jitters), prop::collection::vec(road, 1..40))
        })
        .prop_map(|(jitters, roads)| {
            let n = jitters.len();
            let nodes: Vec<Node> = jitters
                .into_iter()
                .enumerate()
                .map(|(i, j)| node(i, j))
                .collect();
            let roads = roads
                .into_iter()
                .map(|(from, step, offsets, speed_limit_mps, lanes)| {
                    let to = (from + step) % n;
                    Road {
                        from_node: NodeId(id(from)),
                        to_node: NodeId(id(to)),
                        geometry: polyline(nodes[from].position, nodes[to].position, &offsets),
                        speed_limit_mps,
                        lanes,
                        name: None,
                        osm: None,
                        roundabout: false,
                    }
                })
                .collect();
            Network {
                nodes,
                roads,
                connections: vec![],
            }
        })
        .prop_flat_map(|network| {
            let mut movements = vec![];
            for (i, a) in network.roads.iter().enumerate() {
                for (j, b) in network.roads.iter().enumerate() {
                    if a.to_node == b.from_node {
                        movements.push(Connection {
                            from_road: RoadId(id(i)),
                            to_road: RoadId(id(j)),
                        });
                    }
                }
            }
            let count = 0..=movements.len();
            (Just(network), subsequence(movements, count))
        })
        .prop_map(|(network, connections)| Network {
            connections,
            ..network
        })
}

/// Anything at all, mostly invalid: arbitrary floats (NaN, infinities, huge),
/// small ids that are sometimes in range, arbitrary point and lane counts.
fn arbitrary_network() -> impl Strategy<Value = Network> {
    let point = (any::<f64>(), any::<f64>()).prop_map(|(x, y)| Point { x, y });
    let road = (
        0u32..8,
        0u32..8,
        prop::collection::vec(point.clone(), 0..5),
        any::<f64>(),
        any::<u8>(),
    )
        .prop_map(|(from, to, geometry, speed_limit_mps, lanes)| Road {
            from_node: NodeId(from),
            to_node: NodeId(to),
            geometry,
            speed_limit_mps,
            lanes,
            name: None,
            osm: None,
            roundabout: false,
        });
    let connection = (0u32..8, 0u32..8).prop_map(|(a, b)| Connection {
        from_road: RoadId(a),
        to_road: RoadId(b),
    });
    (
        prop::collection::vec(
            point.prop_map(|position| Node {
                position,
                osm_node: None,
            }),
            0..6,
        ),
        prop::collection::vec(road, 0..6),
        prop::collection::vec(connection, 0..6),
    )
        .prop_map(|(nodes, roads, connections)| Network {
            nodes,
            roads,
            connections,
        })
}

fn id(i: usize) -> u32 {
    u32::try_from(i).unwrap()
}

fn load(network: Network) -> Result<InProcess, NetworkError> {
    InProcess::with_network(Config::default(), 1, network)
}

fn hash_after(network: &Network, seed: u64) -> u64 {
    let mut sim = InProcess::with_network(Config::default(), seed, network.clone()).unwrap();
    sim.step(100);
    sim.state_hash()
}

proptest! {
    #[test]
    fn the_roundabout_flag_is_part_of_the_state_hash(network in valid_network()) {
        let mut flipped = network.clone();
        flipped.roads[0].roundabout = !flipped.roads[0].roundabout;
        prop_assert_ne!(hash_after(&network, 1), hash_after(&flipped, 1));
    }

    #[test]
    fn valid_networks_load_and_come_back_unchanged(network in valid_network()) {
        let sim = load(network.clone()).unwrap();
        prop_assert_eq!(sim.network(), &network);
    }

    #[test]
    fn networks_round_trip_through_json(network in valid_network()) {
        let json = serde_json::to_string(&network).unwrap();
        prop_assert_eq!(serde_json::from_str::<Network>(&json).unwrap(), network);
    }

    #[test]
    fn validation_never_panics(network in arbitrary_network()) {
        let _ = load(network);
    }

    #[test]
    fn a_planted_defect_is_reported(
        mut network in valid_network(),
        defect in 0..11u8,
        item in any::<Index>(),
        point in any::<Index>(),
        non_finite in prop_oneof![Just(f64::NAN), Just(f64::INFINITY), Just(f64::NEG_INFINITY)],
        bad_speed in prop_oneof![Just(0.0), Just(-1.0), Just(f64::NAN), Just(f64::INFINITY)],
    ) {
        let r = item.index(network.roads.len());
        let road = RoadId(id(r));
        let geometry = &mut network.roads[r].geometry;
        let p = point.index(geometry.len());
        match defect {
            0 => {
                let missing = NodeId(id(network.nodes.len()));
                network.roads[r].to_node = missing;
                prop_assert_eq!(load(network).unwrap_err(), NetworkError::UnknownNode { road, node: missing });
            }
            1 => {
                let missing = RoadId(id(network.roads.len()));
                network.connections.push(Connection { from_road: missing, to_road: road });
                prop_assert_eq!(load(network).unwrap_err(), NetworkError::UnknownRoad { from_road: missing, to_road: road, missing });
            }
            2 => {
                // A reversed copy of road r always continues it, so r → copy is valid.
                let mut back = network.roads[r].clone();
                (back.from_node, back.to_node) = (back.to_node, back.from_node);
                back.geometry.reverse();
                let copy = RoadId(id(network.roads.len()));
                network.roads.push(back);
                let c = Connection { from_road: road, to_road: copy };
                network.connections.extend([c, c]);
                prop_assert_eq!(load(network).unwrap_err(), NetworkError::DuplicateConnection { from_road: road, to_road: copy });
            }
            3 => {
                geometry.insert(p, geometry[p]);
                prop_assert_eq!(load(network).unwrap_err(), NetworkError::ZeroLengthSegment { road, point: p });
            }
            4 => {
                geometry[p].y = non_finite;
                prop_assert_eq!(load(network).unwrap_err(), NetworkError::NonFiniteGeometry { road, point: p });
            }
            5 => {
                let n = item.index(network.nodes.len());
                network.nodes[n].position.x = non_finite;
                prop_assert_eq!(load(network).unwrap_err(), NetworkError::NonFiniteNode { node: NodeId(id(n)) });
            }
            6 => {
                network.roads[r].speed_limit_mps = bad_speed;
                let err = load(network).unwrap_err();
                prop_assert!(matches!(err, NetworkError::BadSpeedLimit { road: e, .. } if e == road), "{err:?}");
            }
            7 => {
                network.roads[r].lanes = 0;
                prop_assert_eq!(load(network).unwrap_err(), NetworkError::NoLanes { road });
            }
            9 => {
                geometry.truncate(1);
                prop_assert_eq!(load(network).unwrap_err(), NetworkError::TooFewPoints { road, points: 1 });
            }
            10 => {
                // Finite, but the segment length squared overflows to infinity.
                geometry[p].x = f64::MAX;
                prop_assert_eq!(load(network).unwrap_err(), NetworkError::LengthOverflow { road });
            }
            _ => {
                // A road never ends where it starts, so it cannot continue onto itself.
                let (ends_at, starts_at) = (network.roads[r].to_node, network.roads[r].from_node);
                network.connections.push(Connection { from_road: road, to_road: road });
                prop_assert_eq!(
                    load(network).unwrap_err(),
                    NetworkError::Disconnected { from_road: road, ends_at, to_road: road, starts_at }
                );
            }
        }
    }

    #[test]
    fn state_hash_depends_on_seed_and_network(
        network in valid_network(),
        seed in any::<u64>(),
        item in any::<Index>(),
    ) {
        let hash = hash_after(&network, seed);
        prop_assert_eq!(hash, hash_after(&network, seed));
        prop_assert_ne!(hash, hash_after(&network, seed.wrapping_add(1)));

        let mut faster = network.clone();
        faster.roads[item.index(network.roads.len())].speed_limit_mps += 1.0;
        prop_assert_ne!(hash, hash_after(&faster, seed));
    }
}

#[test]
fn error_messages_name_the_offending_item_and_value() {
    let messages = [
        (
            NetworkError::UnknownNode {
                road: RoadId(1),
                node: NodeId(7),
            },
            "road 1 references node 7, which does not exist",
        ),
        (
            NetworkError::UnknownRoad {
                from_road: RoadId(3),
                to_road: RoadId(9),
                missing: RoadId(9),
            },
            "the connection from road 3 to road 9 refers to road 9, which does not exist",
        ),
        (
            NetworkError::Disconnected {
                from_road: RoadId(2),
                ends_at: NodeId(5),
                to_road: RoadId(3),
                starts_at: NodeId(8),
            },
            "the connection from road 2 to road 3 is impossible: road 2 ends at node 5 but road 3 starts at node 8",
        ),
        (
            NetworkError::DuplicateConnection {
                from_road: RoadId(3),
                to_road: RoadId(5),
            },
            "the connection from road 3 to road 5 appears twice",
        ),
        (
            NetworkError::BadSpeedLimit {
                road: RoadId(3),
                speed_limit_mps: -1.0,
            },
            "road 3 needs a positive, finite speed limit, got -1 m/s",
        ),
    ];
    for (err, message) in messages {
        assert_eq!(err.to_string(), message);
    }
}
