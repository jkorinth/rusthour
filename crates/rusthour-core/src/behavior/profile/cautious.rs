//! Cautious: long headway, gentle, polite (ADR-0006 §2).

use super::{Driver, idm};
use crate::behavior::mobil::MobilParams;

pub(super) fn driver() -> Driver {
    Driver {
        idm: idm(0.9, 2.0, 1.0, 1.5, 3.0),
        mobil: MobilParams {
            politeness: 0.5,
            threshold_mps2: 0.2,
            bias_right_mps2: 0.4,
            safe_decel_mps2: 3.0,
        },
    }
}
