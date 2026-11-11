//! Intersection passage in the step pipeline (M2.5b, ADR-0004 §2).

use super::*;
use crate::network::{EndControl, Road};

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
        class: 0,
        priority_road: false,
        end_control: EndControl::None,
        turn_lanes: vec![],
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

/// Congestion-aware routing (M2.9). From X, road 1 (100 m) and roads 2, 3
/// (80 m + 80 m via Z) both reach Y; road 0 feeds X, 4 leaves Y, 5 closes
/// the loop. The detour costs 4.3 s more at free flow.
fn two_routes() -> Network {
    let dir = |a: [f64; 2], b: [f64; 2]| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let d = (dx * dx + dy * dy).sqrt();
        [dx / d, dy / d]
    };
    let nodes = vec![
        [-200.0, 0.0], // P
        [0.0, 0.0],    // X
        [100.0, 0.0],  // Y
        [50.0, -60.0], // Z: road 1 yields to road 3 at Y
        [300.0, 0.0],  // Q
    ];
    let mut roads = Vec::new();
    for (from, to, length_m) in [
        (0, 1, 200.0),
        (1, 2, 100.0),
        (1, 3, 80.0),
        (3, 2, 80.0),
        (2, 4, 200.0),
        (4, 0, 600.0),
    ] {
        let d = dir(nodes[from as usize], nodes[to as usize]);
        roads.push(road(from, to, length_m, 14.0, [d, d]));
    }
    Network {
        nodes,
        roads,
        connections: vec![(0, 1), (0, 2), (2, 3), (1, 4), (3, 4), (4, 5), (5, 0)],
    }
}

/// Runs random demand on [`two_routes`] for 20 minutes and counts the
/// vehicles that entered road 1 and road 2 from road 0 on their way past Y:
/// the trips with a choice.
fn through_counts(target_vehicles: u32) -> [u32; 2] {
    let config = Config {
        target_vehicles,
        ..Config::default()
    };
    let mut e = Engine::with_network(config, 5, two_routes());
    let mut counts = [0; 2];
    let mut was_on = Vec::new();
    for _ in 0..20 * MINUTE {
        e.step(1);
        let vs = &e.vehicles;
        was_on.resize(vs.slot_count(), u32::MAX);
        for (s, was) in was_on.iter_mut().enumerate() {
            let road = vs.road(s);
            let past_y = vs.route[s].last().is_some_and(|d| ![1, 2, 3].contains(d));
            if road != *was && *was == 0 && past_y && (road == 1 || road == 2) {
                counts[road as usize - 1] += 1;
            }
            *was = road;
        }
    }
    counts
}

#[test]
fn congestion_spreads_traffic_over_both_routes() {
    let [short, detour] = through_counts(5);
    assert!(
        short > 0 && detour == 0,
        "free flow: {short} short, {detour} detour"
    );
    let [short, detour] = through_counts(40);
    assert!(
        short > 0 && detour * 10 >= short,
        "congested: {short} short, {detour} detour"
    );
}

#[test]
fn a_vehicle_under_way_avoids_a_jam_ahead_and_keeps_its_road() {
    let mut e = Engine::with_network(Config::default(), 1, two_routes());
    // Block road 4's entry with a crawler, then queue cars on road 1.
    let mut crawler = car();
    crawler.desired_speed_factor = 0.001;
    e.spawn(&[4], crawler).unwrap();
    let mut queued = 0;
    while queued < 10 {
        queued += u32::from(e.spawn(&[1, 4], car()).is_some());
        e.step(1);
    }
    // Let the estimate build, then send a car towards road 1 so that its
    // turn to re-plan comes while it is still on road 0.
    e.step(MINUTE);
    let period = REROUTE_PERIOD_MS / u64::from(e.config.dt_ms);
    let slot = e.vehicles.slot_count() as u64;
    while !(e.tick + slot + 20).is_multiple_of(period) {
        e.step(1);
    }
    let id = e.spawn(&[0, 1, 4], car()).unwrap();
    e.step(40);
    let s = id.slot as usize;
    assert_eq!(e.vehicles.road(s), 0, "still on its first road");
    assert_eq!(e.vehicles.route[s], [0, 2, 3, 4]);
}

#[test]
fn rerouting_is_staggered_by_slot() {
    let config = Config {
        target_vehicles: 150,
        ..Config::default()
    };
    let mut e = Engine::with_network(config, 2, two_routes());
    let period = REROUTE_PERIOD_MS / u64::from(e.config.dt_ms);
    let mut total = 0;
    for _ in 0..2 * period {
        let n = u64::from(e.clone().update_routes());
        let bound = (e.vehicles.slot_count() as u64).div_ceil(period);
        assert!(n <= bound, "{n} re-plans at tick {}, bound {bound}", e.tick);
        total += n;
        e.step(1);
    }
    assert!(total > 0, "nobody re-planned");
}

/// ADR-0005 §4: a ring head is not held for "no room" on the next ring road.
#[test]
fn ring_heads_are_exempt_from_no_room() {
    let stopped = |roundabout: bool| {
        let (mut a, mut b) = (
            road(0, 1, 100.0, 10.0, [[1.0, 0.0]; 2]),
            road(1, 0, 100.0, 10.0, [[-1.0, 0.0]; 2]),
        );
        a.roundabout = roundabout;
        b.roundabout = roundabout;
        let net = Network {
            nodes: vec![[0.0, 0.0], [100.0, 0.0]],
            roads: vec![a, b],
            connections: vec![(0, 1), (1, 0)],
        };
        let mut e = Engine::with_network(Config::default(), 1, net);
        let head = e.spawn(&[0, 1], car()).unwrap();
        e.spawn(&[1, 0], car()).unwrap(); // sits at the start of road 1: no room
        e.vehicles.pos_m[head.slot as usize] = 95.0;
        e.decide();
        e.stop[e.vehicles.lane_id(0, 0)]
    };
    assert!(stopped(false), "a plain node keeps the box clear");
    assert!(!stopped(true), "the ring keeps circulating");
}

/// Lowest speed of a lone car within 10 m of road 0's end, and whether it
/// reached road 1, at an otherwise empty node.
fn pass_alone(end_control: EndControl) -> (f64, bool) {
    let mut first = road(0, 1, 200.0, 13.9, [[1.0, 0.0]; 2]);
    first.end_control = end_control;
    let net = Network {
        nodes: vec![[0.0, 0.0], [200.0, 0.0], [400.0, 0.0]],
        roads: vec![first, road(1, 2, 200.0, 13.9, [[1.0, 0.0]; 2])],
        connections: vec![(0, 1)],
    };
    let mut e = Engine::with_network(Config::default(), 1, net);
    let id = e.spawn(&[0, 1], car()).unwrap();
    let mut min_v = f64::INFINITY;
    for _ in 0..MINUTE {
        let s = id.slot as usize;
        if e.vehicles.road(s) == 1 {
            return (min_v, true);
        }
        if e.vehicles.pos_m[s] > 190.0 {
            min_v = min_v.min(e.vehicles.speed_mps[s]);
        }
        e.step(1);
    }
    (min_v, false)
}

#[test]
fn a_stop_sign_needs_a_full_stop_even_at_an_empty_node() {
    let (min_v, passed) = pass_alone(EndControl::Stop);
    assert!(passed && min_v < STUCK_SPEED_MPS, "{min_v} {passed}");
    let (min_v, passed) = pass_alone(EndControl::GiveWay);
    assert!(
        passed && min_v > 5.0,
        "a give-way sign alone doesn't stop it: {min_v}"
    );
}
