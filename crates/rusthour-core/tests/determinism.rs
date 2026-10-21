//! Determinism harness tests (M0.8).
//!
//! Every change to `rusthour-core` must keep these passing. If a change
//! intentionally alters simulation results, update `GOLDEN_HASH` in the same
//! PR and say so in the PR description.

use rusthour_core::{Config, Engine, run_and_hash};

/// State hash after 1000 steps with seed 42, the default config and an empty
/// network. Changed in M1.4: the state hash now includes the network.
const GOLDEN_HASH: u64 = 11_439_358_342_447_110_657;

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
    assert_eq!(run_and_hash(Config::default(), 42, 1000), GOLDEN_HASH);
}
