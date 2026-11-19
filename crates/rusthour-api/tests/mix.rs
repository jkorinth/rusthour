//! The driver and fleet mix of a run (M4.3): validated, normalised, parsed
//! from `name=share,…`, and the default reproduces the default run.

use proptest::prelude::*;
use rusthour_api::{Config, DriverMix, FleetMix, InProcess, MixError, NetworkError, Simulation};

fn share() -> impl Strategy<Value = f64> {
    prop_oneof![Just(0.0), 0.0..100.0, any::<f64>()]
}

proptest! {
    /// Shares must be finite and ≥ 0 with a positive sum; the normalised mix
    /// sums to 1 and keeps the ratios.
    #[test]
    fn shares_are_validated_and_normalised(s in prop::array::uniform4(share())) {
        let got = FleetMix::from_shares(s).normalised();
        let valid = s.iter().all(|x| x.is_finite() && *x >= 0.0);
        let max = s.iter().copied().fold(0.0, f64::max);
        match got {
            Err(MixError::BadShare { share, .. }) => {
                let ok = share >= 0.0 && share.is_finite();
                prop_assert!(!valid && !ok, "{share}");
            }
            Err(MixError::Empty) => prop_assert!(valid && max == 0.0),
            Err(e) => prop_assert!(false, "unexpected {e}"),
            Ok(n) => {
                prop_assert!(valid && max > 0.0);
                let n = n.shares();
                prop_assert!((n.iter().sum::<f64>() - 1.0).abs() < 1e-12);
                let n_max = n.iter().copied().fold(0.0, f64::max);
                for (a, b) in n.iter().zip(s) {
                    prop_assert!((a / n_max - b / max).abs() < 1e-12, "{n:?} {s:?}");
                }
            }
        }
    }
}

#[test]
fn a_mix_is_parsed_from_names_and_shares() {
    let m: DriverMix = "aggressive=15, normal=60,cautious=20 ,erratic=5"
        .parse()
        .unwrap();
    assert_eq!(m.normalised().unwrap(), DriverMix::default());
    let m: FleetMix = "car=1,bus=0.5".parse().unwrap();
    assert_eq!(
        m,
        FleetMix {
            car: 1.0,
            van: 0.0,
            truck: 0.0,
            bus: 0.5
        },
        "a missing class is 0"
    );
    let err = |s: &str| s.parse::<FleetMix>().unwrap_err().to_string();
    assert!(
        err("car=85,lorry=5").contains("unknown vehicle class “lorry”; use car, van, truck, bus")
    );
    assert!(err("car=85,car=5").contains("car is given twice"));
    assert!(err("car").contains("“car” is not name=share"));
    assert!(err("car=lots").contains("“lots” is not a number"));
    assert!(err("car=-1").contains("car must be a number ≥ 0, got -1"));
    assert!(err("car=0").contains("at least one share must be above 0"));
}

#[test]
fn the_default_mix_reproduces_the_default_run() {
    let run = |config| {
        let mut sim = InProcess::new(config, 42);
        sim.step(100);
        sim.state_hash()
    };
    let config = Config {
        profile_mix: DriverMix::default().weights().unwrap(),
        class_mix: FleetMix::default().weights().unwrap(),
        ..Config::default()
    };
    assert_eq!(run(config), run(Config::default()));
    let all_zero = Config {
        class_mix: [0; 4],
        ..Config::default()
    };
    assert_eq!(
        InProcess::with_network(all_zero, 1, rusthour_api::Network::default()).err(),
        Some(NetworkError::EmptyMix)
    );
}
