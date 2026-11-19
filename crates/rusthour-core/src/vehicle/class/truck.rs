//! Truck over 3.5 t: 12 m, sluggish, longer headway, 80 km/h (ADR-0007 §2;
//! Kesting, Treiber and Helbing 2009, table 2; `StVO` §3(3)).

use super::Spec;

pub(super) const SPEC: Spec = Spec {
    length_m: 12.0,
    width_m: 2.55,
    max_accel_mps2: 0.7,
    comfort_decel_mps2: 2.0,
    // The normal driver's 1.5 s and 2 m become table 2's 2.0 s and 4 m.
    headway_scale: 4.0 / 3.0,
    min_gap_scale: 2.0,
    max_speed_mps: 80.0 / 3.6,
    keep_right_mps2: 0.3,
};
