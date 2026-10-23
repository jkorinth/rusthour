//! Engine benchmarks (M2.8): steps per second by vehicle count, and routing
//! on its own. ADR-0004 budget at 5,000 vehicles: step <= 1 ms, routing
//! <= 0.25 ms per step (about 2.5 new trips).

// `criterion_group!` expands to an undocumented public fn.
#![allow(missing_docs)]

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rusthour_core::demand::Trip;
use rusthour_core::routing::Router;
use rusthour_core::{Config, Engine};

mod grid;

fn step(c: &mut Criterion) {
    let net = grid::grid(24);
    let mut g = c.benchmark_group("step");
    for vehicles in [500, 2_000, 5_000] {
        let config = Config {
            target_vehicles: vehicles,
            ..Config::default()
        };
        let mut e = Engine::with_network(config, 1, net.clone());
        // Fill up to the steady population.
        e.step(3_000);
        let active = u32::try_from(e.vehicles().live_count()).unwrap();
        assert!(
            active >= vehicles * 9 / 10,
            "only {active} of {vehicles} vehicles on the network"
        );
        g.throughput(Throughput::Elements(u64::from(vehicles)));
        g.bench_function(BenchmarkId::from_parameter(vehicles), |b| {
            b.iter(|| e.step(1));
        });
    }
    g.finish();
}

/// One A* query between spread-out routable roads, as demand draws them.
fn routing(c: &mut Criterion) {
    let net = grid::grid(24);
    let mut router = Router::new(&net);
    let roads = router.routable().to_vec();
    let n = roads.len();
    let mut out = Vec::new();
    let mut i = 0;
    c.bench_function("route", |b| {
        b.iter_batched(
            || {
                i = (i + 1) % n;
                Trip {
                    origin: roads[i],
                    destination: roads[(i * 7 + n / 2) % n],
                }
            },
            |trip| router.route(&net, trip, &mut out),
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, step, routing);
criterion_main!(benches);
