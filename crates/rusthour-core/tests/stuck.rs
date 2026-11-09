//! M2.10: the stuck-vehicle safety net removes vehicles that have not moved
//! for `stuck_timeout_s`, which breaks a physical gridlock.

use rusthour_core::network::{Network, Road};
use rusthour_core::vehicle::idm::IdmParams;
use rusthour_core::{Config, Engine};

fn car() -> IdmParams {
    let (a, b) = (1.5, 2.0);
    IdmParams {
        desired_speed_factor: 1.0,
        time_headway_s: 1.5,
        max_accel_mps2: a,
        comfort_decel_mps2: b,
        min_gap_m: 2.0,
        length_m: 4.5,
        sqrt_ab: f64::sqrt(a * b),
    }
}

/// Four 15 m one-lane roads in a ring, each next road in the ring.
fn ring() -> Network {
    let road = |from, to| Road {
        from,
        to,
        length_m: 15.0,
        speed_limit_mps: 10.0,
        lanes: 1,
        start_dir: [1.0, 0.0],
        end_dir: [1.0, 0.0],
        roundabout: false,
    };
    Network {
        nodes: vec![[0.0, 0.0], [15.0, 0.0], [30.0, 0.0], [45.0, 0.0]],
        roads: (0..4).map(|r| road(r, (r + 1) % 4)).collect(),
        connections: (0..4).map(|r| (r, (r + 1) % 4)).collect(),
    }
}

/// Fills the ring with cars that circle for ever until it is gridlocked:
/// every car's next road is full.
fn gridlocked(config: Config) -> Engine {
    let mut e = Engine::with_network(config, 1, ring());
    for _ in 0..600 {
        for start in 0..4u32 {
            let route: Vec<u32> = (0..100).map(|i| (start + i) % 4).collect();
            e.spawn(&route, car());
        }
        e.step(1);
    }
    e
}

fn positions(e: &Engine) -> Vec<f64> {
    let vs = e.vehicles();
    (0..vs.slot_count()).map(|s| vs.pos_m[s]).collect()
}

#[test]
fn disabled_ring_stays_frozen() {
    let mut e = gridlocked(Config {
        stuck_timeout_s: 0,
        ..Config::default()
    });
    let (live, pos) = (e.vehicles().live_count(), positions(&e));
    assert!(live >= 4);
    e.step(10_000); // 1000 s
    assert_eq!(e.removed_stuck(), 0);
    assert_eq!(e.vehicles().live_count(), live);
    assert_eq!(positions(&e), pos, "the ring should be frozen");
}

#[test]
fn stuck_vehicles_are_removed_only_after_the_timeout() {
    let mut e = gridlocked(Config::default());
    let live = e.vehicles().live_count();
    e.step(2_000); // frozen for 200 s of the 300 s
    assert_eq!(e.removed_stuck(), 0, "removed too early");
    assert_eq!(e.vehicles().live_count(), live);
    e.step(1_500);
    let t = e.removed_stuck();
    assert!(t > 0, "nobody was removed");
    assert_eq!(e.vehicles().live_count() as u64 + u64::from(t), live as u64);
    // Removals free room and the jam dissolves: a new car enters and drives.
    e.step(3_000);
    let route: Vec<u32> = (0..100).map(|i| i % 4).collect();
    let id = e.spawn(&route, car()).expect("room on road 0");
    e.step(50);
    assert!(
        e.vehicles().speed_mps[id.slot as usize] > 1.0,
        "still stuck"
    );
}

#[test]
fn removal_is_deterministic() {
    let hash = || {
        let mut e = gridlocked(Config::default());
        e.step(3_500);
        assert!(e.removed_stuck() > 0);
        e.state_hash()
    };
    assert_eq!(hash(), hash());
}

#[test]
fn removals_are_counted_per_road_and_arrivals_separately() {
    let mut e = gridlocked(Config::default());
    assert_eq!(e.arrived(), 0, "nobody reaches a destination on this ring");
    e.step(3_500);
    let per_road = e.stuck_by_road();
    assert_eq!(per_road.len(), 4);
    assert_eq!(per_road.iter().sum::<u32>(), e.removed_stuck());
    assert!(e.removed_stuck() > 0);
}

#[test]
fn arrivals_are_counted() {
    let mut e = Engine::with_network(Config::default(), 1, ring());
    e.spawn(&[0, 1], car()).expect("room");
    e.step(600);
    assert_eq!((e.arrived(), e.removed_stuck()), (1, 0));
    assert_eq!(e.vehicles().live_count(), 0);
}
