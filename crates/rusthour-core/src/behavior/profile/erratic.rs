//! Erratic: each driver's IDM parameters are the normal ones times
//! U(0.8, 1.2), politeness is U(0, 0.4), and the acceleration gets an offset
//! in ±`NOISE_MPS2`, redrawn every simulated second (ADR-0006 §4).

use rand_chacha::ChaCha8Rng;

use super::{Driver, idm, normal, unit};

/// Noise amplitude (m/s²); below the lowest jittered `a` (1.2 m/s²).
pub(super) const NOISE_MPS2: f64 = 0.5;

pub(super) fn driver(rng: &mut ChaCha8Rng) -> Driver {
    let n = normal::driver();
    let mut f = |x: f64| x * (0.8 + 0.4 * unit(rng));
    let p = &n.idm;
    let idm = idm(
        f(p.desired_speed_factor),
        f(p.time_headway_s),
        f(p.max_accel_mps2),
        f(p.comfort_decel_mps2),
        f(p.min_gap_m),
    );
    let mut mobil = n.mobil;
    mobil.politeness = 0.4 * unit(rng);
    Driver { idm, mobil }
}

pub(super) fn noise_mps2(rng: &mut ChaCha8Rng) -> f64 {
    NOISE_MPS2 * (2.0 * unit(rng) - 1.0)
}
