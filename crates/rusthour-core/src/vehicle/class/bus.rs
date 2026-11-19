//! Bus: a 12 m city bus with standing passengers, gentle and capped at
//! 80 km/h; no stops or lines yet (ADR-0007 §2, §7).

use super::Spec;

pub(super) const SPEC: Spec = Spec {
    length_m: 12.0,
    width_m: 2.55,
    max_accel_mps2: 1.0,
    comfort_decel_mps2: 1.5,
    max_speed_mps: 80.0 / 3.6,
    keep_right_mps2: 0.3,
    ..Spec::NONE
};
