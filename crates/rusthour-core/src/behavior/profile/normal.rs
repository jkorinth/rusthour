//! Normal: the M2 car (ADR-0006 §2; after Kesting, Treiber and Helbing 2010).

use super::{Driver, idm};
use crate::behavior::mobil::MobilParams;

pub(super) fn driver() -> Driver {
    Driver {
        idm: idm(1.0, 1.5, 1.5, 2.0, 2.0),
        mobil: MobilParams::default(),
    }
}
