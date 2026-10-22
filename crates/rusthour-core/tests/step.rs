//! Step pipeline (M2.1b, ADR-0004 §2): IDM, integration, transfers, arrivals.

use proptest::prelude::*;
use rusthour_core::network::{Network, Road};
use rusthour_core::vehicle::idm::IdmParams;
use rusthour_core::{Config, Engine};

fn road(from: u32, to: u32, length_m: f64) -> Road {
    Road {
        from,
        to,
        length_m,
        speed_limit_mps: 14.0,
        lanes: 1,
        start_dir: [1.0, 0.0],
        end_dir: [1.0, 0.0],
        roundabout: false,
    }
}

/// Roads `0 → 1 → … → n-1`, one per length, end to end.
fn chain(lengths: &[f64]) -> Network {
    let n = u32::try_from(lengths.len()).unwrap();
    Network {
        nodes: (0..=n).map(|i| [f64::from(i), 0.0]).collect(),
        roads: (0..n)
            .map(|i| road(i, i + 1, lengths[i as usize]))
            .collect(),
        connections: (1..n).map(|i| (i - 1, i)).collect(),
    }
}

fn car(desired_speed_factor: f64) -> IdmParams {
    let (a, b) = (1.5, 2.0);
    IdmParams {
        desired_speed_factor,
        time_headway_s: 1.5,
        max_accel_mps2: a,
        comfort_decel_mps2: b,
        min_gap_m: 2.0,
        length_m: 4.5,
        sqrt_ab: f64::sqrt(a * b),
    }
}

/// The vehicles' (road, pos, speed) in lane order.
fn state(e: &Engine) -> Vec<(u32, f64, f64)> {
    let vs = e.vehicles();
    (0..vs.lane_count())
        .flat_map(|q| vs.lane(q).iter())
        .map(|&s| {
            let s = s as usize;
            (vs.road(s), vs.pos_m[s], vs.speed_mps[s])
        })
        .collect()
}

#[test]
fn free_vehicle_reaches_v0_transfers_and_arrives() {
    let mut e = Engine::with_network(Config::default(), 1, chain(&[600.0, 600.0]));
    let id = e.spawn(&[0, 1], car(1.0)).expect("empty road");
    let mut max_v: f64 = 0.0;
    let mut seen_road_1 = false;
    for _ in 0..1200 {
        e.step(1);
        if let [(r, _, v)] = state(&e)[..] {
            max_v = max_v.max(v);
            seen_road_1 |= r == 1;
        }
    }
    assert!(
        (max_v - 14.0).abs() < 0.1 && max_v <= 14.0,
        "max v = {max_v}"
    );
    assert!(seen_road_1, "never transferred");
    assert_eq!(e.vehicles().live_count(), 0, "did not arrive");
    assert!(!e.vehicles().is_alive(id));
}

#[test]
fn follower_never_overlaps_its_leader_across_road_ends() {
    // A fast car enters a 6 m road 1.5 m behind a crawling car on the next
    // road: no braking can stop it in time, so the position clamp against
    // the leader on the later road must hold it back.
    let lengths = [6.0, 300.0];
    let offset = [0.0, 6.0];
    let mut e = Engine::with_network(Config::default(), 1, chain(&lengths));
    let slow = e.spawn(&[1], car(0.02)).unwrap();
    let fast = e.spawn(&[0, 1], car(3.0)).unwrap();
    let x = |e: &Engine, s: u32| {
        let s = s as usize;
        offset[e.vehicles().road(s) as usize] + e.vehicles().pos_m[s]
    };
    for _ in 0..600 {
        e.step(1);
        let gap = x(&e, slow.slot) - 4.5 - x(&e, fast.slot);
        assert!(gap >= -1e-9, "overlap: gap = {gap} at tick {}", e.tick());
    }
    assert_eq!(
        e.vehicles().road(fast.slot as usize),
        1,
        "follower should follow"
    );
}

#[test]
#[allow(clippy::float_cmp)] // the hold sets exactly `pos = length, v = 0`
fn blocked_transfer_holds_the_vehicle_at_the_road_end() {
    // Roads 0 and 1 merge into road 2; two identical cars reach the
    // merge in the same step, so one of them is blocked.
    let mut net = chain(&[80.0]);
    net.nodes = vec![[0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [2.0, 0.0]];
    net.roads = vec![road(0, 2, 80.0), road(1, 2, 80.0), road(2, 3, 200.0)];
    net.connections = vec![(0, 2), (1, 2)];
    let mut e = Engine::with_network(Config::default(), 1, net);
    e.spawn(&[0, 2], car(1.0)).unwrap();
    e.spawn(&[1, 2], car(1.0)).unwrap();
    let mut held = false;
    for _ in 0..400 {
        e.step(1);
        for (r, pos, v) in state(&e) {
            assert!(pos >= 0.0 && v >= 0.0);
            held |= r < 2 && pos == 80.0 && v == 0.0;
        }
    }
    assert!(held, "no vehicle was held at the merge");
    assert_eq!(e.vehicles().live_count(), 0, "both should arrive");
}

proptest! {
    /// A ring of roads; each spawn `(origin, hops, speed factor, steps to
    /// run after it)` sends a vehicle once or more around it.
    #[test]
    fn lanes_stay_sorted_without_overlap_or_negative_positions(
        lengths in prop::collection::vec(3.0..120.0f64, 2..6),
        spawns in prop::collection::vec((0..6u32, 1..15u32, 0.3..1.6f64, 1..40u8), 1..25),
    ) {
        let n = u32::try_from(lengths.len()).unwrap();
        let mut net = chain(&lengths);
        net.roads[(n - 1) as usize].to = 0;
        net.nodes.pop();
        net.connections.push((n - 1, 0));
        let mut e = Engine::with_network(Config::default(), 1, net);
        for (origin, hops, factor, wait) in spawns {
            let route: Vec<u32> = (0..hops).map(|k| (origin + k) % n).collect();
            let _ = e.spawn(&route, car(factor));
            for _ in 0..wait {
                e.step(1);
                let vs = e.vehicles();
                for q in 0..vs.lane_count() {
                    let lane = vs.lane(q);
                    for (i, &s) in lane.iter().enumerate() {
                        let s = s as usize;
                        let (pos, v) = (vs.pos_m[s], vs.speed_mps[s]);
                        let len = lengths[vs.road(s) as usize];
                        prop_assert!((0.0..=len).contains(&pos), "pos {pos} on road of {len} m");
                        prop_assert!(v.is_finite() && v >= 0.0, "v = {v}");
                        if i > 0 {
                            let l = lane[i - 1] as usize;
                            let gap = vs.pos_m[l] - vs.params[l].length_m - pos;
                            prop_assert!(gap >= -1e-9, "overlap in lane {q}: gap {gap}");
                        }
                    }
                }
            }
        }
    }
}
