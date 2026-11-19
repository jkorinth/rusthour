//! Car: the M2 vehicle, 4.5 m (ADR-0007 §2).

use super::Spec;

pub(super) const SPEC: Spec = Spec {
    length_m: 4.5,
    width_m: 1.8,
    ..Spec::NONE
};
