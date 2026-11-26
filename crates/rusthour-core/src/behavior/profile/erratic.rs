//! Erratic: each driver's IDM parameters are the tuned ones (by default
//! the normal ones) times U(0.8, 1.2), politeness is U(0, 0.4) by default,
//! its gap acceptance is the normal one times U(0.8, 1.2) (ADR-0009),
//! and the acceleration gets an offset in ±`NOISE_MPS2`, redrawn every
//! simulated second (ADR-0006 §4).

use rand_chacha::ChaCha8Rng;

use super::{Driver, Gap, Tuning, idm, normal, unit};

/// The mean of the default politeness draw, U(0, 0.4).
pub(super) const MEAN_POLITENESS: f64 = 0.2;

/// Noise amplitude (m/s²); below the lowest jittered `a` (1.2 m/s²).
pub(super) const NOISE_MPS2: f64 = 0.5;

pub(super) fn driver(t: &Tuning, rng: &mut ChaCha8Rng) -> Driver {
    let mut f = |x: f64| x * (0.8 + 0.4 * unit(rng));
    let idm = idm(
        f(t.desired_speed_factor),
        f(t.time_headway_s),
        f(t.max_accel_mps2),
        f(t.comfort_decel_mps2),
        f(t.min_gap_m),
    );
    let mut mobil = normal::driver().mobil;
    // 2 × 0.2 is exactly 0.4, so the default draws as before M4.4.
    mobil.politeness = 2.0 * t.politeness * unit(rng);
    // Drawn last, so the draws above stay as before M5.2.
    let k = 0.8 + 0.4 * unit(rng);
    let gap = Gap {
        critical_s: normal::GAP.critical_s * k,
    };
    Driver { idm, mobil, gap }
}

pub(super) fn noise_mps2(rng: &mut ChaCha8Rng) -> f64 {
    NOISE_MPS2 * (2.0 * unit(rng) - 1.0)
}
