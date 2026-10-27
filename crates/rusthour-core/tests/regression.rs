//! Regression tests for bugs found on `main` (AGENTS.md §5).

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

/// M2 trial: a priority winner whose exit road is full still held the
/// approaches that yield to it. At a dual junction (nodes P and Q joined by
/// two 7.8 m roads) that closes a cycle: `a` on Q→P yields at P to `x` from
/// its right, `x` waits for room behind `b` on P→Q, `b` yields at Q to `y`
/// from its right, and `y` waits for room behind `a`. Traffic froze.
#[test]
fn regression_m2_trial_full_exit_holds_the_node() {
    let road = |from, to, length_m, dir| Road {
        from,
        to,
        length_m,
        speed_limit_mps: 14.0,
        lanes: 1,
        start_dir: dir,
        end_dir: dir,
        roundabout: false,
    };
    let (east, west, north, south) = ([1.0, 0.0], [-1.0, 0.0], [0.0, 1.0], [0.0, -1.0]);
    // Nodes: P, Q, north of P, south of Q, far west, far east.
    let net = Network {
        nodes: vec![
            [0.0, 0.0],
            [8.0, 0.0],
            [0.0, 100.0],
            [8.0, -100.0],
            [-200.0, 0.0],
            [208.0, 0.0],
        ],
        roads: vec![
            road(0, 1, 7.8, east),    // 0: P→Q
            road(1, 0, 7.8, west),    // 1: Q→P
            road(2, 0, 100.0, south), // 2: into P from the north
            road(3, 1, 100.0, north), // 3: into Q from the south
            road(0, 4, 200.0, west),  // 4: out of P
            road(1, 5, 200.0, east),  // 5: out of Q
        ],
        connections: vec![(2, 0), (3, 1), (0, 5), (1, 4)],
    };
    let mut e = Engine::with_network(Config::default(), 1, net);
    e.spawn(&[2, 0, 5], car()).expect("x enters");
    e.spawn(&[3, 1, 4], car()).expect("y enters");
    // x and y are 40 m from their nodes: arriving, too fast to stop.
    e.step(43);
    e.spawn(&[1, 4], car()).expect("a enters");
    e.spawn(&[0, 5], car()).expect("b enters");
    e.step(1200);
    assert_eq!(
        e.vehicles().live_count(),
        0,
        "gridlock: not all arrived in 2 min"
    );
}
