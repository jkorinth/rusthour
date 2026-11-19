//! ADR-0004: no allocations in `Engine::step` after warm-up, at a steady
//! population. `dhat` (safe to use; `unsafe_code` is forbidden here) counts
//! the allocations.
//!
//! `dhat` counts every thread's allocations, so this binary has no libtest
//! harness (`harness = false`): libtest watches tests from a second thread and
//! allocates its "running for over 60 seconds" warning there, which made this
//! test flaky on slow runners (M4.10). Here `main` is the only thread. It
//! answers the `--list` calls of cargo-nextest itself.

#![cfg(not(target_arch = "wasm32"))]

use rusthour_core::{Config, Engine};

#[path = "../benches/grid.rs"]
mod grid;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const NAME: &str = "no_allocations_in_steady_state";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        // cargo-nextest lists the tests in libtest's terse format; none is ignored.
        if !args.iter().any(|a| a == "--ignored") {
            println!("{NAME}: test");
        }
        return;
    }
    // A name filter (`cargo test <filter>`, nextest's `--exact <name>`) that
    // doesn't match, or a matching `--skip`, skips the test; the values of
    // libtest's other options are not filters.
    let mut args = args.iter();
    while let Some(a) = args.next() {
        let skip = match a.as_str() {
            "--skip" => args.next().is_some_and(|f| NAME.contains(f.as_str())),
            "--test-threads" | "--format" | "--color" | "--logfile" | "-Z" => {
                args.next();
                false
            }
            f => !f.starts_with('-') && !NAME.contains(f),
        };
        if skip {
            return;
        }
    }
    no_allocations_in_steady_state();
    println!("test {NAME} ... ok");
}

fn no_allocations_in_steady_state() {
    let config = Config {
        target_vehicles: 1000,
        ..Config::default()
    };
    // One profiler for both: the one-lane grid and the multi-lane one (M3.6).
    let mut engines =
        [grid::grid(12), grid::multilane_grid(12)].map(|net| Engine::with_network(config, 7, net));
    // Warm-up: columns, scratch buffers and route Vecs (sized by the longest
    // route so far) reach their peaks within about 7,500 steps; traffic still
    // flows when the window is measured. A change that shifts the trajectory
    // (demand, routing, the car) may need another warm-up.
    for e in &mut engines {
        e.step(20_000);
    }
    let _profiler = dhat::Profiler::builder().testing().build();
    for e in &mut engines {
        let before = dhat::HeapStats::get().total_blocks;
        e.step(1000);
        let n = dhat::HeapStats::get().total_blocks - before;
        assert_eq!(n, 0, "{n} allocations in 1000 steps");
        assert!(e.vehicles().live_count() > 0);
    }
}
