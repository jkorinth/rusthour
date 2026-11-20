//! Junction delay against reference curves (M4.7, ADR-0005 addendum M4.7).

use rusthour_core::behavior::profile::Profile;
use rusthour_core::network::{EndControl, Network, Road};
use rusthour_core::routing::Router;
use rusthour_core::{Config, Engine};

/// Arm `k` lies in direction `ARMS[k]` from the node: east, north, west, south.
const ARMS: [[f64; 2]; 4] = [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]];
const LENGTH_M: f64 = 300.0;
const SPEED_MPS: f64 = 13.9;

#[derive(Clone, Copy, Debug)]
enum Control {
    /// Every arm unsigned and equal: right before left.
    Equal,
    /// The minor roads (north, south) give way.
    GiveWay,
    /// The minor roads stop.
    Stop,
    /// The major roads (east, west) are priority roads.
    PriorityRoad,
    /// Every arm signalled: two phases of 25 s green, 3 s amber, 2 s all-red.
    Signal,
}

/// A movement measured and its conflicting streams, as `(in road, out road)`.
#[derive(Clone, Copy, Debug)]
enum Movement {
    /// South → east, conflicting with west → east.
    MinorRight,
    /// South → north, crossing both major streams.
    MinorStraight,
    /// South → west, crossing both major streams.
    MinorLeft,
    /// West → north, across east → west.
    MajorLeft,
}

impl Movement {
    fn route(self) -> [u32; 2] {
        match self {
            Self::MinorRight => [3, 4],
            Self::MinorStraight => [3, 5],
            Self::MinorLeft => [3, 6],
            Self::MajorLeft => [2, 5],
        }
    }

    /// Conflicting streams and their share of the conflicting flow.
    fn conflicting(self) -> &'static [([u32; 2], f64)] {
        match self {
            Self::MinorRight => &[([2, 4], 1.0)],
            Self::MinorStraight | Self::MinorLeft => &[([2, 4], 0.5), ([0, 6], 0.5)],
            Self::MajorLeft => &[([0, 6], 1.0)],
        }
    }
}

/// A four-arm cross; road `k` comes in from arm `k`, road `4 + k` leaves to it.
fn cross(control: Control) -> Network {
    let mut nodes = vec![[0.0, 0.0]];
    nodes.extend(ARMS.iter().map(|d| [LENGTH_M * d[0], LENGTH_M * d[1]]));
    let road = |from: u32, to: u32, dir: [f64; 2]| Road {
        from,
        to,
        length_m: LENGTH_M,
        speed_limit_mps: SPEED_MPS,
        lanes: 1,
        start_dir: dir,
        end_dir: dir,
        roundabout: false,
        class: 0,
        priority_road: false,
        end_control: EndControl::None,
        turn_lanes: vec![],
    };
    let mut roads: Vec<Road> = (0..4u32)
        .map(|k| {
            let d = ARMS[k as usize];
            let mut r = road(k + 1, 0, [-d[0], -d[1]]);
            let minor = k % 2 == 1;
            match control {
                Control::GiveWay if minor => r.end_control = EndControl::GiveWay,
                Control::Stop if minor => r.end_control = EndControl::Stop,
                Control::PriorityRoad if !minor => r.priority_road = true,
                Control::Signal => r.end_control = EndControl::Signals,
                _ => {}
            }
            r
        })
        .collect();
    roads.extend((0..4u32).map(|k| {
        let mut r = road(0, k + 1, ARMS[k as usize]);
        r.priority_road = matches!(control, Control::PriorityRoad) && k % 2 == 0;
        r
    }));
    Network {
        nodes,
        roads,
        connections: (0..4)
            .flat_map(|i| (0..4).filter(move |&j| j != i).map(move |j| (i, 4 + j)))
            .collect(),
    }
}

/// `SplitMix64`: a tiny seeded generator for the arrival times.
struct Rng(u64);

impl Rng {
    /// Uniform in `(0, 1]`.
    #[allow(clippy::cast_precision_loss)]
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64 + f64::EPSILON
    }

    /// Exponential gap at `vph` vehicles per hour, in ticks of 0.1 s.
    fn gap_ticks(&mut self, vph: f64) -> f64 {
        -self.unit().ln() * 36_000.0 / vph
    }
}

/// Mean delays of one movement (s).
#[derive(Debug)]
struct Delay {
    /// Time held at the line (`go = false`), what routing observes.
    held_s: f64,
    /// Time from entering the approach to leaving it, less a lone car's
    /// time at an empty uncontrolled node: the control delay.
    control_s: f64,
    n: usize,
}

/// Runs `hours` with `movement` arriving at `minor_vph` and its conflicting
/// streams at `conflicting_vph` in all, all as Poisson streams of normal cars.
#[allow(clippy::cast_precision_loss)]
fn measure(
    control: Control,
    movement: Movement,
    conflicting_vph: f64,
    minor_vph: f64,
    hours: u64,
    seed: u64,
) -> Delay {
    let free_ticks = lone_ticks(movement);
    let mut e = Engine::with_network(Config::default(), seed, cross(control));
    let car = Profile::Normal.base().idm;
    let mut rng = Rng(seed);
    let mut streams: Vec<([u32; 2], f64, f64, u32)> = movement
        .conflicting()
        .iter()
        .filter(|_| conflicting_vph > 0.0)
        .map(|&(route, share)| (route, conflicting_vph * share, 0.0, 0))
        .collect();
    streams.push((movement.route(), minor_vph, 0.0, 0));
    for s in &mut streams {
        s.2 = rng.gap_ticks(s.1);
    }
    let minor = streams.len() - 1;
    let warm_up = 3_000;
    // `(slot, entry tick, held ticks so far)` of the measured cars on the approach.
    let mut on: Vec<(u32, u64, u32)> = Vec::new();
    let (mut held, mut control_ticks, mut n) = (0u64, 0u64, 0usize);
    for t in 0..hours * 36_000 + warm_up {
        for (i, (route, vph, next, due)) in streams.iter_mut().enumerate() {
            while *next <= t as f64 {
                *due += 1;
                *next += rng.gap_ticks(*vph);
            }
            // Queued arrivals enter when the entry has room.
            if *due > 0
                && let Some(id) = e.spawn(route, car)
            {
                *due -= 1;
                if i == minor && t >= warm_up {
                    on.push((id.slot, t, 0));
                }
            }
        }
        e.step(1);
        let vs = e.vehicles();
        on.retain_mut(|(slot, t0, h)| {
            if vs.road(*slot as usize) == movement.route()[0] {
                *h = vs.held_ticks[*slot as usize];
                return true;
            }
            held += u64::from(*h);
            control_ticks += (t + 1 - *t0).saturating_sub(free_ticks);
            n += 1;
            false
        });
    }
    Delay {
        held_s: held as f64 / 10.0 / n.max(1) as f64,
        control_s: control_ticks as f64 / 10.0 / n.max(1) as f64,
        n,
    }
}

/// Ticks a lone car takes from entering `movement`'s approach to leaving it
/// at an empty uncontrolled node.
fn lone_ticks(movement: Movement) -> u64 {
    let mut e = Engine::with_network(Config::default(), 1, cross(Control::Equal));
    let id = e
        .spawn(&movement.route(), Profile::Normal.base().idm)
        .unwrap();
    let mut t = 0;
    while e.vehicles().road(id.slot as usize) == movement.route()[0] {
        e.step(1);
        t += 1;
    }
    t
}

/// HCM 2000 (eq. 17-37, 17-38) control delay of a minor movement at
/// minor flow `vx` with critical gap `tc` and follow-up `tf` (s), against
/// conflicting flow `vc` (veh/h), over `period_h`; `stop` adds the 5 s of
/// slowing to and starting from the stop line.
fn hcm_delay(vc: f64, vx: f64, tc: f64, tf: f64, stop: bool) -> f64 {
    let c = if vc > 0.0 {
        vc * (-vc * tc / 3600.0).exp() / (1.0 - (-vc * tf / 3600.0).exp())
    } else {
        3600.0 / tf
    };
    let (x, t) = (vx / c, 0.25);
    3600.0 / c
        + 900.0 * t * ((x - 1.0) + ((x - 1.0).powi(2) + 3600.0 / c * x / (450.0 * t)).sqrt())
        + if stop { 5.0 } else { 0.0 }
}

/// Adams' delay: mean wait of a lone minor car for a gap of `tc` (s) in a
/// Poisson stream of `vc` veh/h.
fn adams_delay(vc: f64, tc: f64) -> f64 {
    if vc <= 0.0 {
        return 0.0;
    }
    let q = vc / 3600.0;
    ((q * tc).exp() - q * tc - 1.0) / q
}

#[test]
#[ignore = "prints the calibration table; run with --ignored --nocapture"]
fn print_junction_delays() {
    let minor_vph = 60.0;
    println!("control movement vc n held_s prior_s control_s hcm_s adams_s");
    for (control, movement, tc, tf) in [
        (Control::GiveWay, Movement::MinorRight, 6.2, 3.3),
        (Control::GiveWay, Movement::MinorStraight, 6.5, 4.0),
        (Control::GiveWay, Movement::MinorLeft, 7.1, 3.5),
        (Control::PriorityRoad, Movement::MinorRight, 6.2, 3.3),
        (Control::Stop, Movement::MinorRight, 6.2, 3.3),
        (Control::Stop, Movement::MinorStraight, 6.5, 4.0),
        (Control::Stop, Movement::MinorLeft, 7.1, 3.5),
        (Control::GiveWay, Movement::MajorLeft, 4.1, 2.2),
        (Control::Equal, Movement::MinorRight, 6.2, 3.3),
        (Control::Equal, Movement::MinorStraight, 6.5, 4.0),
        (Control::Equal, Movement::MinorLeft, 7.1, 3.5),
        (Control::Signal, Movement::MinorRight, 0.0, 0.0),
        (Control::Signal, Movement::MinorStraight, 0.0, 0.0),
        (Control::Signal, Movement::MinorLeft, 0.0, 0.0),
    ] {
        let [from, to] = movement.route();
        let prior = (Router::new(&cross(control)).movement_delays(from))
            .find(|m| m.0 == to)
            .map_or(f64::NAN, |m| m.3);
        for vc in [0.0, 150.0, 300.0, 600.0, 900.0] {
            let d = measure(control, movement, vc, minor_vph, 4, 7);
            let stop = matches!(control, Control::Stop);
            println!(
                "{control:?} {movement:?} {vc} {} {:.2} {prior:.2} {:.2} {:.2} {:.2}",
                d.n,
                d.held_s,
                d.control_s,
                hcm_delay(vc, minor_vph, tc, tf, stop),
                adams_delay(vc, tc),
            );
        }
    }
}

#[test]
fn a_give_way_right_turn_waits_longer_with_more_conflicting_flow_as_in_the_hcm() {
    let (tc, tf) = (6.2, 3.3);
    let delay = |vc| measure(Control::GiveWay, Movement::MinorRight, vc, 60.0, 4, 7).control_s;
    let zero = delay(0.0);
    let mut last = zero;
    for vc in [150.0, 300.0, 600.0] {
        let d = delay(vc);
        assert!(d > last, "{d:.2} s at {vc} veh/h, {last:.2} s below");
        last = d;
        // Against the HCM curve above zero flow: there a lone car passes a
        // give-way sign at speed, while the HCM counts slowing down for it.
        let (sim, hcm) = (
            d - zero,
            hcm_delay(vc, 60.0, tc, tf, false) - hcm_delay(0.0, 60.0, tc, tf, false),
        );
        if vc <= 300.0 {
            assert!(
                (sim / hcm - 1.0).abs() <= 0.5,
                "{sim:.2} s against {hcm:.2} s at {vc} veh/h"
            );
        }
    }
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn signal_delay_at_low_flow_is_close_to_websters_uniform_delay() {
    // Lone cars, one per second of the 60 s cycle.
    let free = lone_ticks(Movement::MinorStraight);
    let mut sum = 0;
    for t0 in (0..600).step_by(10) {
        let mut e = Engine::with_network(Config::default(), 1, cross(Control::Signal));
        e.step(t0);
        let route = Movement::MinorStraight.route();
        let id = e.spawn(&route, Profile::Normal.base().idm).unwrap();
        let mut t = 0;
        while e.vehicles().road(id.slot as usize) == route[0] {
            e.step(1);
            t += 1;
        }
        sum += t - free;
    }
    let sim = sum as f64 / 10.0 / 60.0;
    // Webster's uniform delay at zero flow, C (1 − g/C)² / 2; amber counts as red.
    let (cycle, green) = (60.0, 25.0);
    let webster = cycle * (1.0f64 - green / cycle).powi(2) / 2.0;
    assert!(
        (sim / webster - 1.0).abs() <= 0.1,
        "{sim:.2} s against {webster:.2} s"
    );
}

#[test]
fn the_routers_prior_matches_the_simulated_delay_at_low_load() {
    for control in [
        Control::Equal,
        Control::GiveWay,
        Control::Stop,
        Control::Signal,
    ] {
        let router = Router::new(&cross(control));
        for movement in [
            Movement::MinorRight,
            Movement::MinorStraight,
            Movement::MinorLeft,
        ] {
            let [from, to] = movement.route();
            let prior = (router.movement_delays(from))
                .find(|m| m.0 == to)
                .map(|m| m.3)
                .unwrap();
            let held = measure(control, movement, 300.0, 60.0, 2, 7).held_s;
            assert!(
                (prior - held).abs() <= (0.25 * held).max(1.5),
                "{control:?} {movement:?}: prior {prior:.2} s, observed {held:.2} s"
            );
        }
    }
}
