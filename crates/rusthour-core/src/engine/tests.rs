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
            .map(|f| f.is_some_and(|&s| no_room(&e.network, &e.vehicles, s as usize, 0)))
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

/// Roads 0 (into the node) and 1, 2 (out of it), all `lanes` wide. A vehicle
/// from road 0 turns left onto 1 (heading W) or goes straight onto 2.
fn lane_node(lanes: [u8; 3]) -> Network {
    let mut roads = vec![
        road(1, 0, 100.0, 10.0, [[0.0, 1.0]; 2]),
        road(0, 2, 100.0, 10.0, [[-1.0, 0.0]; 2]),
        road(0, 3, 100.0, 10.0, [[0.0, 1.0]; 2]),
    ];
    for (r, l) in roads.iter_mut().zip(lanes) {
        r.lanes = l;
    }
    Network {
        nodes: vec![[0.0; 2]; 4],
        roads,
        connections: vec![(0, 1), (0, 2)],
    }
}

/// Puts a car on `lane` of road 0, `pos_m` from its start, at rest.
fn park(e: &mut Engine, lane: u8, pos_m: f64, route: &[u32]) -> usize {
    put_on(e, lane, pos_m, route, car())
}

/// Like [`park`], with the given driver.
fn put_on(e: &mut Engine, lane: u8, pos_m: f64, route: &[u32], params: IdmParams) -> usize {
    let id = e.vehicles.spawn(crate::vehicle::NewVehicle {
        road: 0,
        lane,
        pos_m,
        speed_mps: 0.0,
        params,
        mobil: crate::behavior::mobil::MobilParams::default(),
        profile: crate::behavior::profile::Profile::Normal,
        class: crate::vehicle::class::Class::Car,
        route,
        depart_tick: 0,
    });
    id.slot as usize
}

#[test]
fn a_transfer_enters_the_lane_of_the_movement() {
    // Left onto road 1 enters its leftmost lane, straight keeps the lane.
    // From lane 0 the left turn waits 10 s, finds no other route and goes.
    for (route, from, to) in [([0, 1], 0, 1), ([0, 2], 0, 0), ([0, 2], 1, 1)] {
        let mut e = Engine::with_network(Config::default(), 1, lane_node([2, 2, 2]));
        let s = park(&mut e, from, 90.0, &route);
        // Keep-right would move it to lane 0 first (MOBIL, M3.6).
        e.vehicles.mobil[s].threshold_mps2 = f64::INFINITY;
        while e.vehicles.road(s) == 0 {
            e.step(1);
            assert!(e.tick() < 600, "never left road 0");
        }
        assert_eq!(e.vehicles.lane_of(s), to, "{route:?} from lane {from}");
    }
}

#[test]
fn keep_right_never_leaves_the_lane_of_the_next_movement() {
    // Left onto road 1 is served by lane 1 only; straight by both.
    for (route, lane) in [([0, 1], 1), ([0, 2], 0)] {
        let mut e = Engine::with_network(Config::default(), 1, lane_node([2, 2, 2]));
        let s = park(&mut e, 1, 20.0, &route);
        e.step(50);
        assert_eq!(e.vehicles.road(s), 0);
        assert_eq!(e.vehicles.lane_of(s), lane, "{route:?}");
    }
}

#[test]
fn a_lane_drop_zippers_without_overlap_or_gridlock() {
    let mut e = Engine::with_network(Config::default(), 1, lane_node([2, 2, 1]));
    for i in 0..6 {
        for lane in 0..2 {
            park(&mut e, lane, 95.0 - 10.0 * f64::from(i), &[0, 2]);
        }
    }
    for _ in 0..2 * MINUTE {
        e.step(1);
        let vs = &e.vehicles;
        let lane: Vec<usize> = vs
            .lane(vs.lane_id(2, 0))
            .iter()
            .map(|&s| s as usize)
            .collect();
        for w in lane.windows(2) {
            assert!(
                vs.pos_m[w[0]] - vs.params[w[0]].length_m >= vs.pos_m[w[1]] - 1e-9,
                "overlap"
            );
        }
    }
    assert_eq!((e.arrived(), e.removed_stuck()), (12, 0));
}

#[test]
fn red_holds_a_car_at_the_line_and_green_releases_it() {
    // Only road 0 is tagged, but the cross road 2 gets a signal too, so the
    // node runs two 30 s phases: road 0 is red from 28 s to 60 s.
    let mut first = road(0, 1, 200.0, 13.9, [[1.0, 0.0]; 2]);
    first.end_control = EndControl::Signals;
    let net = Network {
        nodes: vec![[0.0, 0.0], [200.0, 0.0], [400.0, 0.0], [200.0, -200.0]],
        roads: vec![
            first,
            road(1, 2, 200.0, 13.9, [[1.0, 0.0]; 2]),
            road(3, 1, 200.0, 13.9, [[0.0, 1.0]; 2]),
        ],
        connections: vec![(0, 1), (2, 1)],
    };
    let mut e = Engine::with_network(Config::default(), 1, net);
    e.step(300);
    let id = e.spawn(&[0, 1], car()).unwrap();
    let s = id.slot as usize;
    e.step(250);
    assert_eq!(e.vehicles.road(s), 0);
    assert!(e.vehicles.speed_mps[s] < STUCK_SPEED_MPS && e.vehicles.pos_m[s] > 190.0);
    let red = WaitReason::StopLine {
        cause: StopCause::Red,
    };
    assert_eq!(e.wait_reason(s), Some(red));
    let lights: Vec<_> = e.signals().collect();
    assert_eq!(lights, [(0, Light::Red, 5.0), (2, Light::Amber, 3.0)]);
    e.step(49);
    assert_eq!(e.vehicles.road(s), 0, "still red");
    e.step(100);
    assert_eq!(e.vehicles.road(s), 1, "green releases it");
}

/// `copies` disjoint forks, five roads each (`5c..5c + 5`). Road `5c`
/// (300 m, two lanes, heading E) turns left onto `5c + 1` (N) or goes
/// straight onto `5c + 2`; both reach `5c + 4`, the left turn directly and
/// straight on via `5c + 3`. Only lane 1 serves the left turn.
fn fork(copies: u32) -> Network {
    let (e, n, nw) = ([1.0, 0.0], [0.0, 1.0], [-0.6, 0.8]);
    let mut net = Network::default();
    for c in 0..copies {
        let (k, y) = (5 * c, 1000.0 * f64::from(c));
        // Nodes k..k + 5: start, junction, north, east, end.
        net.nodes.extend([
            [-300.0, y],
            [0.0, y],
            [0.0, y + 100.0],
            [75.0, y],
            [0.0, y + 200.0],
        ]);
        let mut first = road(k, k + 1, 300.0, 13.9, [e; 2]);
        first.lanes = 2;
        net.roads.extend([
            first,
            road(k + 1, k + 2, 100.0, 13.9, [n; 2]),
            road(k + 1, k + 3, 75.0, 13.9, [e; 2]),
            road(k + 3, k + 2, 125.0, 13.9, [nw; 2]),
            road(k + 2, k + 4, 100.0, 13.9, [n; 2]),
        ]);
        net.connections.extend([
            (k, k + 1),
            (k, k + 2),
            (k + 2, k + 3),
            (k + 1, k + 4),
            (k + 3, k + 4),
        ]);
    }
    net
}

#[test]
fn a_left_turner_moves_to_the_left_lane_before_the_line() {
    let mut e = Engine::with_network(Config::default(), 1, fork(1));
    let s = e.spawn(&[0, 1, 4], car()).unwrap().slot as usize;
    let mut lane = 0;
    while e.vehicles.road(s) == 0 {
        lane = e.vehicles.lane_of(s);
        e.step(1);
        assert!(e.tick() < 600, "never left road 0");
    }
    assert_eq!(
        (lane, e.vehicles.road(s)),
        (1, 1),
        "left from the left lane"
    );
    assert_eq!(e.fallback_replans(), 0);
}

#[test]
fn a_car_stuck_in_the_wrong_lane_waits_then_replans_from_its_lane() {
    let mut e = Engine::with_network(Config::default(), 1, fork(1));
    let s = e.spawn(&[0, 1, 4], car()).unwrap().slot as usize;
    // It finds no gap, ever.
    e.vehicles.mobil[s].threshold_mps2 = f64::INFINITY;
    let wrong = WaitReason::StopLine {
        cause: StopCause::WrongLane,
    };
    let mut waited = 0;
    while e.vehicles.road(s) == 0 {
        e.step(1);
        waited += u32::from(e.wait_reason(s) == Some(wrong));
        assert!(e.tick() < 1200, "never left road 0");
    }
    assert!(waited >= 100, "held {waited} steps");
    assert_eq!(e.vehicles.road(s), 2, "straight on, which lane 0 serves");
    assert_eq!(e.vehicles.route[s], [0, 2, 3, 4]);
    assert_eq!(e.fallback_replans(), 1);
    e.step(MINUTE);
    assert_eq!(e.arrived(), 1);
}

#[test]
fn at_most_four_fallback_replans_run_per_step() {
    let mut e = Engine::with_network(Config::default(), 1, fork(6));
    for c in 0..6 {
        let s = e.spawn(&[5 * c, 5 * c + 1, 5 * c + 4], car()).unwrap().slot as usize;
        e.vehicles.mobil[s].threshold_mps2 = f64::INFINITY;
    }
    let mut most = 0;
    for _ in 0..MINUTE {
        let before = e.fallback_replans();
        e.step(1);
        most = most.max(e.fallback_replans() - before);
    }
    assert_eq!(
        (most, e.fallback_replans()),
        (4, 6),
        "the rest retry next step"
    );
}

/// M3 trial finding 1: a left turner beside a slow, dense queue in the left
/// lane got no gap, waited at the line in the wrong lane and re-planned.
/// The car behind it in the queue now lets it in.
#[test]
fn a_turner_merges_into_a_slow_queue() {
    let mut e = Engine::with_network(Config::default(), 1, fork(1));
    let mut crawler = car();
    crawler.desired_speed_factor = 0.1;
    // A queue in lane 1 crawling at 1.4 m/s behind its slow head.
    for i in 0..30 {
        let params = if i == 0 { crawler } else { car() };
        let q = put_on(&mut e, 1, 290.0 - 8.0 * f64::from(i), &[0, 2, 3, 4], params);
        e.vehicles.mobil[q].threshold_mps2 = f64::INFINITY; // stays in lane 1
    }
    e.step(100);
    let s = park(&mut e, 0, 150.0, &[0, 1, 4]);
    while e.vehicles.road(s) == 0 {
        e.step(1);
        assert!(e.tick() < 3000, "never left road 0");
    }
    assert_eq!(e.vehicles.road(s), 1, "left, from the left lane");
    assert_eq!(e.fallback_replans(), 0);
}

/// A periodic re-plan near the line keeps to the movements of the
/// vehicle's lane; further back it may take any (finding 1: re-plans in a
/// turn lane sent cars to the line in the wrong lane).
#[test]
fn a_replan_near_the_line_keeps_to_the_lanes_movements() {
    for (pos_m, route) in [(250.0, vec![0, 2, 3, 4]), (50.0, vec![0, 1, 4])] {
        let mut e = Engine::with_network(Config::default(), 1, fork(1));
        // Straight on from lane 0; left (lane 1 only) is shorter.
        let s = park(&mut e, 0, pos_m, &[0, 2, 3, 4]);
        e.vehicles.mobil[s].threshold_mps2 = f64::INFINITY;
        let period = REROUTE_PERIOD_MS / u64::from(e.config.dt_ms);
        e.tick = period - s as u64 % period;
        assert_eq!(e.update_routes(), 1);
        assert_eq!(e.vehicles.route[s], route, "at {pos_m} m");
    }
}

#[test]
fn demand_spawns_every_profile_and_only_erratic_drivers_get_noise() {
    let config = Config {
        target_vehicles: 20,
        ..Config::default()
    };
    let mut e = Engine::with_network(config, 3, two_routes());
    let (mut seen, mut noisy) = ([false; 4], false);
    for _ in 0..20 * MINUTE {
        e.step(1);
        let vs = &e.vehicles;
        for s in (0..vs.slot_count()).filter(|&s| vs.is_live(s)) {
            let (p, n) = (vs.profile[s], vs.noise_mps2[s]);
            seen[Profile::ALL.iter().position(|&q| q == p).unwrap()] = true;
            assert!(n.abs() <= 0.5, "noise {n}");
            if p == Profile::Erratic {
                noisy |= n.to_bits() != 0;
            } else {
                let d = vs.class[s].apply(p.base());
                assert!(n.to_bits() == 0 && vs.params[s] == d.idm && vs.mobil[s] == d.mobil);
            }
        }
    }
    assert!(seen == [true; 4] && noisy, "seen {seen:?}, noisy {noisy}");
}

#[test]
fn spawned_drivers_take_their_profiles_tuning() {
    let mut config = Config {
        target_vehicles: 20,
        profile_mix: [0, 0, 1, 1],
        class_mix: [1, 0, 0, 0],
        ..Config::default()
    };
    let cautious = &mut config.tuning[2];
    cautious.time_headway_s = 2.5;
    cautious.politeness = 0.9;
    config.tuning[3].politeness = 0.0; // erratic: U(0, 2 × 0) = 0
    let mut e = Engine::with_network(config, 3, two_routes());
    e.step(2 * MINUTE);
    let vs = &e.vehicles;
    let live: Vec<_> = (0..vs.slot_count()).filter(|&s| vs.is_live(s)).collect();
    assert!(!live.is_empty());
    for s in live {
        match vs.profile[s] {
            Profile::Cautious => {
                assert_eq!(vs.params[s].time_headway_s.to_bits(), 2.5f64.to_bits());
                assert_eq!(vs.mobil[s].politeness.to_bits(), 0.9f64.to_bits());
            }
            p => assert_eq!((p, vs.mobil[s].politeness.to_bits()), (Profile::Erratic, 0)),
        }
    }
}

/// One straight one-lane road of `length_m` at `speed` (m/s).
fn straight(length_m: f64, speed: f64) -> Network {
    Network {
        nodes: vec![[0.0, 0.0], [length_m, 0.0]],
        roads: vec![road(0, 1, length_m, speed, [[1.0, 0.0]; 2])],
        connections: vec![],
    }
}

/// A normal driver in a vehicle of `class`, at `pos_m` on lane 0 of road 0.
fn put_class(e: &mut Engine, pos_m: f64, speed_mps: f64, route: &[u32], class: Class) -> usize {
    put_driver(e, pos_m, speed_mps, route, Profile::Normal, class)
}

fn put_driver(
    e: &mut Engine,
    pos_m: f64,
    speed_mps: f64,
    route: &[u32],
    profile: Profile,
    class: Class,
) -> usize {
    let d = class.apply(profile.driver(e.rng(Stream::Behavior)));
    let id = e.vehicles.spawn(crate::vehicle::NewVehicle {
        road: route[0],
        lane: 0,
        pos_m,
        speed_mps,
        params: d.idm,
        mobil: d.mobil,
        profile,
        class,
        route,
        depart_tick: 0,
    });
    id.slot as usize
}

/// ADR-0007 §6: six trucks queue 45 m (6 × 7.5 m of extra length) or
/// more further back than six cars behind the same standing vehicle.
#[test]
fn a_truck_takes_more_room_in_a_queue() {
    let queue = |class: Class| {
        let mut e = Engine::with_network(Config::default(), 1, straight(400.0, 13.9));
        let blocker = put_class(&mut e, 390.0, 0.0, &[0], Class::Car);
        e.vehicles.params[blocker].desired_speed_factor = 1e-4;
        let mut n = 0;
        while n < 6 {
            let p = class.apply(Profile::Normal.base()).idm;
            if let Some(id) = e.spawn(&[0], p) {
                e.vehicles.class[id.slot as usize] = class;
                n += 1;
            }
            e.step(1);
        }
        e.step(2 * MINUTE);
        let vs = &e.vehicles;
        let tail = *vs.lane(0).back().unwrap() as usize;
        assert!(vs.speed_mps[tail] < 0.1, "the queue stands");
        vs.pos_m[blocker] - (vs.pos_m[tail] - vs.params[tail].length_m)
    };
    let (cars, trucks) = (queue(Class::Car), queue(Class::Truck));
    assert!(trucks >= cars + 45.0, "cars {cars} m, trucks {trucks} m");
}

/// "Don't block the box" asks for room for the head's own length.
#[test]
fn the_room_check_uses_the_vehicles_own_length() {
    let net = Network {
        nodes: vec![[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]],
        roads: vec![
            road(0, 1, 100.0, 13.9, [[1.0, 0.0]; 2]),
            road(1, 2, 100.0, 13.9, [[1.0, 0.0]; 2]),
        ],
        connections: vec![(0, 1)],
    };
    for (class, room) in [(Class::Car, true), (Class::Truck, false)] {
        let mut e = Engine::with_network(Config::default(), 1, net.clone());
        // A car on road 1 whose rear is 5.5 m from its start: room for 4.5 m.
        put_class(&mut e, 10.0, 0.0, &[1], Class::Car);
        let head = put_class(&mut e, 99.0, 0.0, &[0, 1], class);
        assert_eq!(
            !no_room(&e.network, &e.vehicles, head, 0),
            room,
            "{class:?}"
        );
    }
}

proptest::proptest! {
    /// An erratic truck driver on a 130 km/h road never exceeds 80 km/h,
    /// though the noise alone would carry it about 14% past its `v0`.
    #[test]
    fn a_truck_never_exceeds_its_speed_cap(seed: u64) {
        let mut e = Engine::with_network(Config::default(), seed, straight(4000.0, 36.1));
        let s = put_driver(&mut e, 0.0, 20.0, &[0], Profile::Erratic, Class::Truck);
        let cap = Class::Truck.spec().max_speed_mps;
        let mut top: f64 = 0.0;
        while e.vehicles.is_live(s) {
            e.step(1);
            top = top.max(e.vehicles.speed_mps[s]);
            proptest::prop_assert!(e.vehicles.speed_mps[s] <= cap + 1e-9);
        }
        proptest::prop_assert!(top > 0.9 * cap, "reached {} of {}", top, cap);
    }
}

fn two_roads() -> Network {
    Network {
        nodes: vec![[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]],
        roads: vec![
            road(0, 1, 100.0, 13.9, [[1.0, 0.0]; 2]),
            road(1, 2, 100.0, 13.9, [[1.0, 0.0]; 2]),
        ],
        connections: vec![(0, 1)],
    }
}

/// A trip starting on a road waits for a vehicle about to enter it from the
/// node behind (M4 trial, finding 4: trips starting on road 2136 kept a
/// truck on road 2134 waiting for room until it was removed).
#[test]
fn a_trip_yields_to_a_vehicle_about_to_enter_its_road() {
    for (pos_m, yields) in [(96.0, true), (10.0, false)] {
        let mut e = Engine::with_network(Config::default(), 1, two_roads());
        put_class(&mut e, pos_m, 0.0, &[0, 1], Class::Truck);
        assert_eq!(e.cuts_in(1, 4.5), yields, "truck at {pos_m} m");
    }
}

/// The room check counts on a tail that drives away at least as fast as the
/// head approaches (M4 trial, finding 5: trucks braked behind a truck that had
/// just crossed, until its rear was 12 m into the next road); a standing
/// tail must have left the room already.
#[test]
fn the_room_check_counts_on_a_tail_driving_away() {
    for (tail_speed, room) in [(8.0, true), (0.0, false)] {
        let mut e = Engine::with_network(Config::default(), 1, two_roads());
        // A car on road 1 with its rear 3.5 m from its start.
        put_class(&mut e, 8.0, tail_speed, &[1], Class::Car);
        let head = put_class(&mut e, 80.0, 8.0, &[0, 1], Class::Truck);
        let free = !no_room(&e.network, &e.vehicles, head, 0);
        assert_eq!(free, room, "tail at {tail_speed} m/s");
    }
}
