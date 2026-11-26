//! Aggressive: short headway, brisk, near-egoistic lane changes (ADR-0006 §2).

use super::{Driver, Gap, idm};
use crate::behavior::mobil::MobilParams;

/// Accepts gaps 0.7 s shorter than normal (ADR-0009).
pub(super) const GAP: Gap = Gap { critical_s: 3.8 };

pub(super) fn driver() -> Driver {
    Driver {
        idm: idm(1.1, 1.0, 2.0, 3.0, 1.5),
        mobil: MobilParams {
            politeness: 0.1,
            threshold_mps2: 0.05,
            bias_right_mps2: 0.2,
            safe_decel_mps2: 5.0,
        },
        gap: GAP,
    }
}
