//! Network loading and geometry query (M1.4).

use rusthour_api::{
    Config, Connection, InProcess, Network, NetworkError, Node, NodeId, Point, Road, RoadId,
    Simulation,
};

/// Three intersections in a row: a straight two-lane road A→B, then a bent
/// one-lane road B→C, with traffic allowed to continue from the first onto
/// the second.
fn sample_network() -> Network {
    let p = |x, y| Point { x, y };
    Network {
        nodes: vec![
            Node {
                position: p(0.0, 0.0),
            },
            Node {
                position: p(100.0, 0.0),
            },
            Node {
                position: p(200.0, 50.0),
            },
        ],
        roads: vec![
            Road {
                from_node: NodeId(0),
                to_node: NodeId(1),
                geometry: vec![p(0.0, 0.0), p(100.0, 0.0)],
                speed_limit_mps: 13.9,
                lanes: 2,
            },
            Road {
                from_node: NodeId(1),
                to_node: NodeId(2),
                geometry: vec![p(100.0, 0.0), p(150.0, 0.0), p(200.0, 50.0)],
                speed_limit_mps: 8.3,
                lanes: 1,
            },
        ],
        connections: vec![Connection {
            from_road: RoadId(0),
            to_road: RoadId(1),
        }],
    }
}

fn hash_after(network: Network, seed: u64) -> u64 {
    let mut sim = InProcess::with_network(Config::default(), seed, network).unwrap();
    sim.step(100);
    sim.status().state_hash
}

#[test]
fn network_round_trips_through_json() {
    let network = sample_network();
    let json = serde_json::to_string(&network).unwrap();
    assert_eq!(serde_json::from_str::<Network>(&json).unwrap(), network);
}

#[test]
fn loading_rejects_dangling_and_duplicate_references() {
    let mut network = sample_network();
    network.roads[1].to_node = NodeId(7);
    let err = InProcess::with_network(Config::default(), 1, network).unwrap_err();
    assert_eq!(
        err,
        NetworkError::UnknownNode {
            road: RoadId(1),
            node: NodeId(7)
        }
    );
    assert_eq!(
        err.to_string(),
        "road 1 references node 7, which does not exist"
    );

    let mut network = sample_network();
    network.connections.push(network.connections[0]);
    let err = InProcess::with_network(Config::default(), 1, network).unwrap_err();
    assert_eq!(
        err,
        NetworkError::DuplicateConnection {
            from_road: RoadId(0),
            to_road: RoadId(1)
        }
    );
}

#[test]
fn loading_rejects_zero_length_segments() {
    let mut network = sample_network();
    let bend = network.roads[1].geometry[1];
    network.roads[1].geometry.insert(1, bend);
    let err = InProcess::with_network(Config::default(), 1, network).unwrap_err();
    assert_eq!(
        err,
        NetworkError::ZeroLengthSegment {
            road: RoadId(1),
            point: 1
        }
    );
}

#[test]
fn simulation_returns_the_loaded_geometry() {
    let sim = InProcess::with_network(Config::default(), 1, sample_network()).unwrap();
    assert_eq!(sim.network(), &sample_network());
}

#[test]
fn state_hash_depends_on_seed_and_network() {
    assert_eq!(
        hash_after(sample_network(), 42),
        hash_after(sample_network(), 42)
    );

    let mut faster = sample_network();
    faster.roads[0].speed_limit_mps = 22.2;
    assert_ne!(hash_after(sample_network(), 42), hash_after(faster, 42));
}
