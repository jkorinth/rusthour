//! Cautious: long headway, gentle, polite (ADR-0006 §2).

use super::{Driver, Gap, idm};
use crate::behavior::mobil::MobilParams;

/// Wants gaps 0.8 s longer than normal (ADR-0009).
pub(super) const GAP: Gap = Gap { critical_s: 5.3 };

pub(super) fn driver() -> Driver {
    Driver {
        idm: idm(0.9, 2.0, 1.0, 1.5, 3.0),
        mobil: MobilParams {
            politeness: 0.5,
            threshold_mps2: 0.2,
            bias_right_mps2: 0.4,
            safe_decel_mps2: 3.0,
        },
        gap: GAP,
    }
}
