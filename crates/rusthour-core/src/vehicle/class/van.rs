//! Van: a light commercial vehicle up to 3.5 t; drives like a car, only
//! bigger (ADR-0007 §2).

use super::Spec;

pub(super) const SPEC: Spec = Spec {
    length_m: 5.5,
    width_m: 2.0,
    ..Spec::NONE
};
