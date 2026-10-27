//! Intersection passage in the step pipeline (M2.5b, ADR-0004 §2).

use super::*;
use crate::network::Road;

const MINUTE: u64 = 600;

fn road(from: u32, to: u32, length_m: f64, speed: f64, dirs: [[f64; 2]; 2]) -> Road {
    Road {
        from,
        to,
        length_m,
        speed_limit_mps: speed,
        lanes: 1,
        start_dir: dirs[0],
        end_dir: dirs[1],
        roundabout: false,
    }
}

/// What a watched road saw from tick 0.
#[derive(Debug, Default)]
struct Watch {
    /// Longest run of ticks without its head having `go`.
    max_wait: u64,
    /// Longest run of ticks without its head leaving.
    max_no_discharge: u64,
    /// Whether its head was ever told to stop while its next road had room.
    stopped: bool,
}

/// Runs `net` for ten simulated minutes, offering a vehicle on every road
/// of `entries` each step (`route(entry index, trip number)`).
fn saturate(
    net: Network,
    entries: &[u32],
    route: impl Fn(usize, u32) -> Vec<u32>,
    watch: &[u32],
) -> Vec<Watch> {
    let mut e = Engine::with_network(Config::default(), 1, net);
    let mut trips = 0;
    let mut out: Vec<Watch> = watch.iter().map(|_| Watch::default()).collect();
    let mut last_go = vec![0; watch.len()];
    let mut last_leave = vec![0; watch.len()];
    let mut head = vec![None; watch.len()];
    for _ in 0..10 * MINUTE {
        for (k, _) in entries.iter().enumerate() {
            if e.spawn(&route(k, trips), car()).is_some() {
                trips += 1;
            }
        }
        // Whether each watched head waits for room, as phase 3 will see it.
        let full: Vec<bool> = (watch.iter())
            .map(|&r| e.vehicles.lane(e.vehicles.lane_id(r, 0)).front())
            .map(|f| f.is_some_and(|&s| no_room(&e.vehicles, s as usize)))
            .collect();
        e.step(1);
        let t = e.tick();
        for (i, &r) in watch.iter().enumerate() {
            let q = e.vehicles.lane_id(r, 0);
            let front = e
                .vehicles
                .lane(q)
                .front()
                .map(|&s| e.vehicles.id(s as usize));
            if front != head[i] {
                head[i] = front;
                last_leave[i] = last_leave[i].max(t);
            }
            if front.is_some() && !e.stop[q] {
                last_go[i] = last_go[i].max(t);
            }
            let w = &mut out[i];
            w.stopped |= e.stop[q] && !full[i];
            w.max_wait = w.max_wait.max(t.saturating_sub(last_go[i]));
            w.max_no_discharge = w.max_no_discharge.max(t.saturating_sub(last_leave[i]));
        }
    }
    out
}

#[test]
fn saturated_four_way_junction_serves_every_approach_within_20_s() {
    // Node 0 in the middle; arm k comes in on road k and leaves on 4 + k.
    let arms = [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]];
    let mut nodes = vec![[0.0, 0.0]];
    nodes.extend(arms.iter().map(|d| [100.0 * d[0], 100.0 * d[1]]));
    let inward = |d: [f64; 2]| [[-d[0], -d[1]]; 2];
    let mut roads: Vec<Road> = (1..=4)
        .map(|k| road(k, 0, 100.0, 13.9, inward(arms[k as usize - 1])))
        .collect();
    roads.extend((1..=4).map(|k| road(0, k, 100.0, 13.9, [arms[k as usize - 1]; 2])));
    let net = Network {
        nodes,
        roads,
        connections: (0..4)
            .flat_map(|i| (0..4).filter(move |&j| j != i).map(move |j| (i, 4 + j)))
            .collect(),
    };
    let route = |k: usize, n: u32| {
        let k = u32::try_from(k).unwrap();
        vec![k, 4 + (k + 1 + n % 3) % 4]
    };
    for (k, w) in saturate(net, &[0, 1, 2, 3], route, &[0, 1, 2, 3])
        .iter()
        .enumerate()
    {
        assert!(w.stopped, "arm {k} was never stopped: {w:?}");
        assert!(
            w.max_wait <= 200 && w.max_no_discharge <= 200,
            "arm {k} starved: {w:?}"
        );
    }
}

#[test]
fn saturated_roundabout_keeps_circulating_priority_and_serves_every_entry() {
    // Ring nodes 0..4 counterclockwise at 20 m; ring road k runs node k → k+1,
    // entry 4 + k (faster than the ring) ends at node k, exit 8 + k starts there.
    let radial = [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]];
    let tangent = |k: usize| [-radial[k % 4][1], radial[k % 4][0]];
    let at = |r: f64| radial.iter().map(move |d| [r * d[0], r * d[1]]);
    let nodes = at(20.0).chain(at(120.0)).chain(at(121.0)).collect();
    let mut roads = Vec::new();
    for k in 0..4u32 {
        let next = (k + 1) % 4;
        let mut ring = road(
            k,
            next,
            31.4,
            8.3,
            [tangent(k as usize), tangent(next as usize)],
        );
        ring.roundabout = true;
        roads.push(ring);
    }
    let d = |k: u32| radial[k as usize];
    roads.extend((0..4).map(|k| road(4 + k, k, 100.0, 13.9, [[-d(k)[0], -d(k)[1]]; 2])));
    roads.extend((0..4).map(|k| road(k, 8 + k, 100.0, 13.9, [d(k); 2])));
    let mut connections = Vec::new();
    for k in 0..4 {
        connections.extend([(4 + k, k), ((k + 3) % 4, k), ((k + 3) % 4, 8 + k)]);
    }
    let net = Network {
        nodes,
        roads,
        connections,
    };
    // Entry k exits after 1 to 3 ring roads.
    let route = |k: usize, n: u32| {
        let (k, m) = (u32::try_from(k).unwrap(), 1 + n % 3);
        let mut r = vec![4 + k];
        r.extend((0..m).map(|i| (k + i) % 4));
        r.push(8 + (k + m) % 4);
        r
    };
    let watch = saturate(net, &[4, 5, 6, 7], route, &[0, 1, 2, 3, 4, 5, 6, 7]);
    for (r, w) in watch.iter().enumerate() {
        if r < 4 {
            assert!(!w.stopped, "circulating road {r} was stopped: {w:?}");
        } else {
            assert!(w.stopped, "entry {r} never yielded: {w:?}");
        }
        // Entries also wait for room on the full ring: ADR-0004's minute.
        assert!(
            w.max_wait <= MINUTE && w.max_no_discharge <= MINUTE,
            "road {r} starved: {w:?}"
        );
    }
}
