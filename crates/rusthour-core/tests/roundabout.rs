//! Roundabout entries with gap acceptance (M5.2, ADR-0009), on a synthetic
//! single-lane roundabout with four arms.

use proptest::prelude::*;
use rusthour_core::behavior::profile::Profile;
use rusthour_core::network::{EndControl, Network, Road};
use rusthour_core::vehicle::VehicleId;
use rusthour_core::{Config, Engine};

const RING_RADIUS_M: f64 = 15.0;
/// Half the angle between an arm's exit and entry node on the ring.
const HALF_ARM_DEG: f64 = 15.0;
const ARM_M: f64 = 200.0;
const RING_MPS: f64 = 50.0 / 3.6;
const ARM_MPS: f64 = 50.0 / 3.6;
const HOUR: u64 = 36_000;

/// Road ids: ring road `k` runs from arm `k`'s exit node to its entry node,
/// ring road `4 + k` from that entry node to arm `k + 1`'s exit node;
/// `8 + k` is arm `k`'s entry, `12 + k` its exit. Arms lie east, north,
/// west and south; the ring turns counter-clockwise.
fn roundabout() -> Network {
    let at = |deg: f64, r: f64| {
        let a = deg.to_radians();
        [r * a.cos(), r * a.sin()]
    };
    let tangent = |deg: f64| {
        let a = deg.to_radians();
        [-a.sin(), a.cos()]
    };
    let arm = |k: u32| f64::from(k) * 90.0;
    let (exit_deg, entry_deg) = (|k| arm(k) - HALF_ARM_DEG, |k| arm(k) + HALF_ARM_DEG);
    // Nodes: exit node k, entry node 4 + k, arm ends 8 + k (in) and 12 + k (out).
    let mut nodes: Vec<[f64; 2]> = (0..4).map(|k| at(exit_deg(k), RING_RADIUS_M)).collect();
    nodes.extend((0..4).map(|k| at(entry_deg(k), RING_RADIUS_M)));
    nodes.extend((0..4).map(|k| at(entry_deg(k), RING_RADIUS_M + ARM_M)));
    nodes.extend((0..4).map(|k| at(exit_deg(k), RING_RADIUS_M + ARM_M)));
    let road = |from: u32, to: u32, length_m, speed, dirs: [[f64; 2]; 2], ring| Road {
        from,
        to,
        length_m,
        speed_limit_mps: speed,
        lanes: 1,
        start_dir: dirs[0],
        end_dir: dirs[1],
        roundabout: ring,
        class: 0,
        priority_road: false,
        end_control: if ring {
            EndControl::None
        } else {
            EndControl::GiveWay
        },
        turn_lanes: vec![],
    };
    let arc = |deg: f64| RING_RADIUS_M * deg.to_radians();
    let mut roads = Vec::new();
    for k in 0..4 {
        let d = [tangent(exit_deg(k)), tangent(entry_deg(k))];
        roads.push(road(k, 4 + k, arc(2.0 * HALF_ARM_DEG), RING_MPS, d, true));
    }
    for k in 0..4 {
        let d = [tangent(entry_deg(k)), tangent(exit_deg((k + 1) % 4))];
        let deg = 90.0 - 2.0 * HALF_ARM_DEG;
        roads.push(road(4 + k, (k + 1) % 4, arc(deg), RING_MPS, d, true));
    }
    for k in 0..4 {
        let inward = at(entry_deg(k) + 180.0, 1.0);
        roads.push(road(8 + k, 4 + k, ARM_M, ARM_MPS, [inward; 2], false));
    }
    for k in 0..4 {
        let outward = at(exit_deg(k), 1.0);
        let mut r = road(k, 12 + k, ARM_M, ARM_MPS, [outward; 2], false);
        r.end_control = EndControl::None;
        roads.push(r);
    }
    let mut connections = Vec::new();
    for k in 0..4 {
        let next = (k + 1) % 4;
        connections.extend([
            (k, 4 + k),
            (8 + k, 4 + k),
            (4 + k, next),
            (4 + k, 12 + next),
        ]);
    }
    Network {
        nodes,
        roads,
        connections,
    }
}

/// From arm `from`'s entry around `turns` quarters (1 to 4) to that exit.
fn route(from: u32, turns: u32) -> Vec<u32> {
    let mut r = vec![8 + from, 4 + from];
    for i in 1..turns {
        let k = (from + i) % 4;
        r.extend([k, 4 + k]);
    }
    r.push(12 + (from + turns) % 4);
    r
}

/// `SplitMix64` for arrival times.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `(0, 1]`.
    #[allow(clippy::cast_precision_loss)]
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 + f64::EPSILON
    }

    /// Exponential gap at `vph` vehicles per hour, in ticks of 0.1 s.
    fn gap_ticks(&mut self, vph: f64) -> f64 {
        -self.unit().ln() * 36_000.0 / vph
    }
}

/// Entry capacity of arm 0 (veh/h): a queue of `profile` drivers always
/// waits there, while normal cars from arm 3 circulate past it to arm 1 as
/// a Poisson stream of `vc` veh/h. Returns the entries and the circulating
/// flow measured in front of the entry, per hour, after five minutes.
#[allow(clippy::cast_precision_loss)]
fn capacity(vc: f64, profile: Profile, hours: u64, seed: u64) -> (f64, f64) {
    let mut e = Engine::with_network(Config::default(), seed, roundabout());
    let mut rng = Rng(seed);
    let (subject, circulating) = (route(0, 1), route(3, 2));
    let mut next = rng.gap_ticks(vc);
    let mut due = 0;
    let warm_up = 3_000;
    let (mut entered, mut passed) = (0u32, 0u32);
    for t in 0..warm_up + hours * HOUR {
        while next <= t as f64 {
            due += 1;
            next += rng.gap_ticks(vc);
        }
        if due > 0 && e.spawn_profile(&circulating, Profile::Normal).is_some() {
            due -= 1;
        }
        e.spawn_profile(&subject, profile);
        // The heads that leave the entry, or pass in front of it, this step.
        let vs = e.vehicles();
        let head = |road: u32| {
            vs.lane(vs.lane_id(road, 0))
                .front()
                .map(|&s| vs.id(s as usize))
        };
        let (at_entry, on_ring) = (head(8), head(0));
        e.step(1);
        let vs = e.vehicles();
        let left = |h: Option<VehicleId>, road| {
            h.is_some_and(|id| !vs.is_alive(id) || vs.road(id.slot as usize) != road)
        };
        if t >= warm_up {
            entered += u32::from(left(at_entry, 8));
            // Only arm 3's cars pass in front of arm 0's entry.
            passed += u32::from(left(on_ring, 0));
        }
    }
    let h = hours as f64;
    (f64::from(entered) / h, f64::from(passed) / h)
}

/// HCM 2010 (exhibit 21-7) capacity of a single-lane entry against a
/// single-lane ring, in passenger cars per hour: `1130·e^(−0.001·vc)`.
fn hcm2010(vc: f64) -> f64 {
    1130.0 * (-0.001 * vc).exp()
}

/// HCM 2016 (6th ed., eq. 22-1): `1380·e^(−0.00102·vc)`.
fn hcm2016(vc: f64) -> f64 {
    1380.0 * (-0.00102 * vc).exp()
}

#[test]
fn entry_capacity_follows_the_hcm_2010_curve_at_low_and_moderate_flow() {
    // Above about 400 veh/h the model falls below the curve (ADR-0009).
    for vc in [150.0, 300.0] {
        let (c, measured_vc) = capacity(vc, Profile::Normal, 1, 7);
        let reference = hcm2010(measured_vc);
        assert!(
            (c / reference - 1.0).abs() <= 0.25,
            "vc {vc} (measured {measured_vc:.0}): {c:.0} veh/h against HCM 2010 {reference:.0}"
        );
    }
}

#[test]
fn aggressive_drivers_enter_more_often_than_cautious_ones() {
    let (aggressive, _) = capacity(600.0, Profile::Aggressive, 1, 3);
    let (cautious, _) = capacity(600.0, Profile::Cautious, 1, 3);
    assert!(
        aggressive > 1.3 * cautious,
        "aggressive {aggressive:.0}, cautious {cautious:.0} veh/h"
    );
}

proptest! {
    /// However hard four arms push in, the ring keeps moving: no
    /// circulating vehicle stands for 20 s, and vehicles keep leaving.
    #[test]
    fn a_saturated_roundabout_keeps_moving(
        vph in prop::array::uniform4(300.0..2000.0f64),
        seed in any::<u64>(),
    ) {
        let mut e = Engine::with_network(Config::default(), seed, roundabout());
        let mut rng = Rng(seed);
        let mut next = vph.map(|v| rng.gap_ticks(v));
        let mut due = [0u32; 4];
        let mut last_arrival = (0, 0);
        for t in 0..3_000u32 {
            for k in 0..4 {
                while next[k] <= f64::from(t) {
                    due[k] += 1;
                    next[k] += rng.gap_ticks(vph[k]);
                }
                let r = rng.next();
                let profile = Profile::ALL[usize::from(u8::try_from(r & 3).unwrap())];
                let arm = u32::try_from(k).unwrap();
                let trip = route(arm, 1 + u32::try_from(r >> 2 & 3).unwrap());
                if due[k] > 0 && e.spawn_profile(&trip, profile).is_some() {
                    due[k] -= 1;
                }
            }
            e.step(1);
            let vs = e.vehicles();
            for ring in 0..8 {
                for &s in vs.lane(vs.lane_id(ring, 0)) {
                    prop_assert!(vs.stuck_ticks[s as usize] < 200, "stood 20 s on ring road {} at {}", ring, t);
                }
            }
            if e.arrived() > last_arrival.0 {
                last_arrival = (e.arrived(), t);
            }
            // The first cars arrive after about 40 s.
            prop_assert!(t < 600 || t - last_arrival.1 < 600, "no vehicle left for 60 s at {}", t);
        }
    }
}

#[test]
#[ignore = "prints the calibration table; run with --ignored --nocapture"]
fn print_entry_capacity() {
    println!("profile vc measured_vc capacity hcm2010 hcm2016");
    for profile in Profile::ALL {
        for vc in [0.0, 150.0, 300.0, 600.0, 900.0, 1200.0] {
            let (c, m) = capacity(vc, profile, 2, 7);
            println!(
                "{profile:?} {vc} {m:.0} {c:.0} {:.0} {:.0}",
                hcm2010(m),
                hcm2016(m)
            );
        }
    }
}
