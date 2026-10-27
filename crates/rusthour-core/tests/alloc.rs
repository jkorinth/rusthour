//! ADR-0004: no allocations in `Engine::step` after warm-up, at a steady
//! population. `dhat` (safe to use; `unsafe_code` is forbidden here) counts
//! the allocations.

#![cfg(not(target_arch = "wasm32"))]

use rusthour_core::{Config, Engine};

#[path = "../benches/grid.rs"]
mod grid;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[test]
fn no_allocations_in_steady_state() {
    let config = Config {
        target_vehicles: 1000,
        ..Config::default()
    };
    let mut e = Engine::with_network(config, 7, grid::grid(12));
    // Warm-up: columns, scratch buffers and route Vecs (sized by the longest
    // route so far) reach their peaks within about 7,500 steps; traffic still
    // flows when the window is measured. A change that shifts the trajectory
    // (demand, routing, the car) may need another warm-up.
    e.step(20_000);
    let _profiler = dhat::Profiler::builder().testing().build();
    let before = dhat::HeapStats::get().total_blocks;
    e.step(1000);
    let n = dhat::HeapStats::get().total_blocks - before;
    assert_eq!(n, 0, "{n} allocations in 1000 steps");
}
