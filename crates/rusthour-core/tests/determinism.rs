//! Determinism harness tests (M0.8).
//!
//! Every change to `rusthour-core` must keep these passing. If a change
//! intentionally alters simulation results, update `GOLDEN_HASH` in the same
//! PR and say so in the PR description.

use rand_chacha::rand_core::Rng;
use rusthour_core::network::{Network, Road};
use rusthour_core::vehicle::idm::IdmParams;
use rusthour_core::{Config, Engine, Stream, run_and_hash};

/// State hash of [`traffic`] with seed 42. Changed in M1.4: the state hash
/// now includes the network. Changed in M2.1a: one random stream per
/// subsystem, each hashed. Changed in M2.1b: the golden run is now the
/// traffic scenario below (step pipeline, random demand and routing), and
/// the hash includes the vehicle store and the waiting trips. Changed in
/// M2.5b: heads yield at nodes by the priority rule, and the hash includes
/// each node's controller state.
const GOLDEN_HASH: u64 = 5_873_231_017_195_465_414;

/// A four-road ring with a short road and a merging side road that routing
/// excludes. Random demand keeps 10 vehicles on the ring, and hand-placed
/// ones come in from the side road; they are blocked at the merge, arrive,
/// and their slots are reused.
fn traffic(seed: u64) -> u64 {
    let road = |from, to, length_m, speed_limit_mps, lanes| Road {
        from,
        to,
        length_m,
        speed_limit_mps,
        lanes,
        start_dir: [1.0, 0.0],
        end_dir: [0.0, 1.0],
        roundabout: false,
    };
    let net = Network {
        nodes: vec![
            [0.0, 0.0],
            [120.0, 0.0],
            [120.0, 80.0],
            [110.0, 80.0],
            [60.0, -50.0],
        ],
        roads: vec![
            road(0, 1, 120.0, 13.9, 1),
            road(1, 2, 80.0, 8.3, 2),
            road(2, 3, 9.5, 8.3, 1),
            road(3, 0, 160.0, 13.9, 1),
            road(4, 1, 70.0, 13.9, 1),
        ],
        connections: vec![(0, 1), (1, 2), (2, 3), (3, 0), (4, 1)],
    };
    let config = Config {
        target_vehicles: 10,
        ..Config::default()
    };
    let mut e = Engine::with_network(config, seed, net);
    for i in 0..40u32 {
        let (a, b) = (1.0 + f64::from(i % 3) * 0.4, 1.5 + f64::from(i % 2));
        let params = IdmParams {
            desired_speed_factor: 0.8 + f64::from(i % 5) * 0.1,
            time_headway_s: 1.0 + f64::from(i % 4) * 0.25,
            max_accel_mps2: a,
            comfort_decel_mps2: b,
            min_gap_m: 2.0,
            length_m: 4.5,
            sqrt_ab: f64::sqrt(a * b),
        };
        let route: Vec<u32> = if i % 3 == 0 {
            [4, 1, 2, 3, 0, 1].to_vec()
        } else {
            (0..4 + i % 7).map(|k| (i + k) % 4).collect()
        };
        let _ = e.spawn(&route, params);
        e.step(25);
    }
    e.step(1000);
    e.state_hash()
}

#[test]
fn same_seed_gives_same_hash() {
    let a = run_and_hash(Config::default(), 42, 1000);
    let b = run_and_hash(Config::default(), 42, 1000);
    assert_eq!(a, b);
}

#[test]
fn different_seeds_give_different_hashes() {
    let a = run_and_hash(Config::default(), 1, 1000);
    let b = run_and_hash(Config::default(), 2, 1000);
    assert_ne!(a, b);
}

#[test]
fn stepping_in_chunks_matches_stepping_at_once() {
    let mut chunked = Engine::new(Config::default(), 7);
    for _ in 0..10 {
        chunked.step(100);
    }
    let mut once = Engine::new(Config::default(), 7);
    once.step(1000);
    assert_eq!(chunked.state_hash(), once.state_hash());
}

#[test]
fn golden_hash_is_stable() {
    assert_eq!(traffic(42), GOLDEN_HASH);
}

#[test]
fn traffic_is_reproducible_and_seeded() {
    assert_eq!(traffic(42), traffic(42));
    assert_ne!(traffic(42), traffic(43), "demand must draw from the seed");
}

#[test]
fn streams_are_independent_and_distinct() {
    let mut a = Engine::new(Config::default(), 9);
    let mut b = Engine::new(Config::default(), 9);
    // Drawing from one stream must not shift another.
    for _ in 0..100 {
        a.rng(Stream::Demand).next_u64();
    }
    assert_eq!(
        a.rng(Stream::Behavior).next_u64(),
        b.rng(Stream::Behavior).next_u64()
    );
    // Same seed, different stream id: different sequences.
    let d = b.rng(Stream::Demand).next_u64();
    let h = b.rng(Stream::Behavior).next_u64();
    assert_ne!(d, h);
}

#[test]
fn drawing_changes_the_state_hash() {
    let mut a = Engine::new(Config::default(), 9);
    let before = a.state_hash();
    a.rng(Stream::Behavior).next_u64();
    assert_ne!(before, a.state_hash());
}
