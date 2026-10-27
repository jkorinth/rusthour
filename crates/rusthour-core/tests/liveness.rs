//! Liveness: at moderate density, traffic keeps arriving (M2 trial).

use proptest::prelude::*;
use rusthour_core::network::{Network, Road};
use rusthour_core::{Config, Engine};

const MINUTE: u64 = 600;

/// An `n × n` grid, nodes 100 m apart, two one-lane roads between
/// neighbours, and every movement except U-turns. East–west roads take
/// their lengths from `lengths` (cycled), north–south roads are 60 m, so
/// short links sit between longer roads as at split junctions.
fn grid(n: u32, lengths: &[f64]) -> Network {
    let mut net = Network {
        nodes: (0..n * n)
            .map(|i| [f64::from(i % n) * 100.0, f64::from(i / n) * 100.0])
            .collect(),
        ..Network::default()
    };
    let add = |net: &mut Network, from: u32, to: u32| {
        let (a, b) = (net.nodes[from as usize], net.nodes[to as usize]);
        let dir = [(b[0] - a[0]) / 100.0, (b[1] - a[1]) / 100.0];
        let length_m = if from.abs_diff(to) == 1 {
            lengths[net.roads.len() % lengths.len()]
        } else {
            60.0
        };
        net.roads.push(Road {
            from,
            to,
            length_m,
            speed_limit_mps: 13.9,
            lanes: 1,
            start_dir: dir,
            end_dir: dir,
            roundabout: false,
        });
    };
    for i in 0..n * n {
        if i % n + 1 < n {
            add(&mut net, i, i + 1);
            add(&mut net, i + 1, i);
        }
        if i + n < n * n {
            add(&mut net, i, i + n);
            add(&mut net, i + n, i);
        }
    }
    for (a, ra) in net.roads.iter().enumerate() {
        for (b, rb) in net.roads.iter().enumerate() {
            if ra.to == rb.from && rb.to != ra.from {
                let id = |r: usize| u32::try_from(r).unwrap();
                net.connections.push((id(a), id(b)));
            }
        }
    }
    net
}

/// Trips completed so far: every despawn bumps its slot's generation.
fn arrivals(e: &Engine) -> u64 {
    let vs = e.vehicles();
    (0..vs.slot_count())
        .map(|s| u64::from(vs.id(s).generation))
        .sum()
}

proptest! {
    /// Short roads (down to one car) make the junction pairs the trial
    /// froze on; one vehicle per 8 roads is far from jam density.
    #[test]
    fn traffic_keeps_arriving_at_moderate_density(
        n in 2..6u32,
        lengths in prop::collection::vec(prop_oneof![6.0..15.0f64, 15.0..120.0f64], 1..12),
        seed in any::<u64>(),
    ) {
        let net = grid(n, &lengths);
        let roads = u32::try_from(net.roads.len()).unwrap();
        let config = Config { target_vehicles: roads / 8 + 1, ..Config::default() };
        let mut e = Engine::with_network(config, seed, net);
        let mut last = 0;
        for window in 1..=4 {
            e.step(2 * MINUTE);
            let now = arrivals(&e);
            prop_assert!(now > last, "no arrival in minutes {}..{}", 2 * (window - 1), 2 * window);
            last = now;
        }
    }
}
