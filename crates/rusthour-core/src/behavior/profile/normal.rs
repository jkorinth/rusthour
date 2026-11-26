//! Normal: the M2 car (ADR-0006 §2; after Kesting, Treiber and Helbing 2010).

use super::{Driver, Gap, idm};
use crate::behavior::mobil::MobilParams;

/// Within the 4–5 s reported for single-lane roundabouts (ADR-0009).
pub(super) const GAP: Gap = Gap { critical_s: 4.5 };

pub(super) fn driver() -> Driver {
    Driver {
        idm: idm(1.0, 1.5, 1.5, 2.0, 2.0),
        mobil: MobilParams::default(),
        gap: GAP,
    }
}
