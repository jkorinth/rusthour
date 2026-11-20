//! Per-profile tuning (M4.4): validated at the api boundary.

use proptest::prelude::*;
use rusthour_api::{Config, DriverProfile, InProcess, Network, NetworkError, TUNING_FIELDS};

proptest! {
    /// A tuned parameter is accepted inside its range and rejected outside,
    /// NaN included, with the profile and parameter named.
    #[test]
    fn tuning_is_validated(
        profile in 0..4usize,
        field in 0..TUNING_FIELDS.len(),
        value in prop_oneof![-10.0..20.0, any::<f64>()],
    ) {
        let f = &TUNING_FIELDS[field];
        let mut config = Config::default();
        (f.set)(&mut config.tuning[profile], value);
        let got = InProcess::with_network(config, 1, Network::default()).err();
        if f.range.contains(&value) {
            prop_assert_eq!(got, None);
        } else {
            let Some(NetworkError::BadTuning { profile: p, parameter, value: v }) = got else {
                return Err(TestCaseError::fail(format!("{got:?}")));
            };
            let want = [
                DriverProfile::Aggressive,
                DriverProfile::Normal,
                DriverProfile::Cautious,
                DriverProfile::Erratic,
            ][profile];
            prop_assert_eq!((p, parameter), (want, f.name));
            prop_assert_eq!(v.to_bits(), value.to_bits());
        }
    }
}

#[test]
fn the_default_tuning_is_valid_and_errors_read_plainly() {
    let config = Config::default();
    for t in &config.tuning {
        for f in &TUNING_FIELDS {
            assert!(f.range.contains(&(f.get)(t)), "{} {t:?}", f.name);
        }
    }
    let err = NetworkError::BadTuning {
        profile: DriverProfile::Cautious,
        parameter: "headway",
        value: 9.0,
    };
    assert_eq!(
        err.to_string(),
        "the cautious drivers' headway must be 0.5 to 3 s, got 9"
    );
}
