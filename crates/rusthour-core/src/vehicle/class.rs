//! Vehicle classes (ADR-0007): each sets the vehicle's dimensions and caps
//! or scales the driver profile's IDM and MOBIL parameters at spawn. Each
//! class is a submodule with its [`Spec`]; the `match` here is the only
//! shared line.

mod bus;
mod car;
mod truck;
mod van;

use rand_chacha::ChaCha8Rng;

use crate::behavior::profile::{Driver, draw_index};

/// What a vehicle is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Class {
    /// The M2 car.
    #[default]
    Car,
    /// Light commercial vehicle up to 3.5 t.
    Van,
    /// Truck over 3.5 t.
    Truck,
    /// City bus; no stops or lines yet.
    Bus,
}

/// A class's dimensions and limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec {
    /// Length (m).
    pub length_m: f64,
    /// Width (m); only drawn.
    pub width_m: f64,
    /// Cap on the profile's maximum acceleration `a` (m/s²).
    pub max_accel_mps2: f64,
    /// Cap on the profile's comfortable deceleration `b` (m/s²).
    pub comfort_decel_mps2: f64,
    /// Factor on the profile's time headway `T`.
    pub headway_scale: f64,
    /// Factor on the profile's minimum gap `s0`.
    pub min_gap_scale: f64,
    /// Cap on the desired speed and the speed itself (m/s).
    pub max_speed_mps: f64,
    /// Added to the profile's keep-right bias `Δa_bias` (m/s²).
    pub keep_right_mps2: f64,
}

impl Spec {
    /// No caps, no scaling; a class sets its dimensions on top.
    const NONE: Self = Self {
        length_m: 0.0,
        width_m: 0.0,
        max_accel_mps2: f64::INFINITY,
        comfort_decel_mps2: f64::INFINITY,
        headway_scale: 1.0,
        min_gap_scale: 1.0,
        max_speed_mps: f64::INFINITY,
        keep_right_mps2: 0.0,
    };
}

// `Vehicles::new` reserves each lane for vehicles 4.5 m long or more, so a
// lane queue never reallocates (ADR-0005 §5); `Spec::NONE` has length 0.
const _: () = {
    let specs = [car::SPEC, van::SPEC, truck::SPEC, bus::SPEC];
    let mut i = 0;
    while i < specs.len() {
        assert!(specs[i].length_m >= 4.5 && specs[i].width_m > 0.0);
        i += 1;
    }
};

/// Default mix in percent, in [`Class::ALL`] order ([`crate::Config`]).
pub const DEFAULT_MIX: [u32; 4] = [85, 8, 5, 2];

impl Class {
    /// All classes, in [`DEFAULT_MIX`] order.
    pub const ALL: [Self; 4] = [Self::Car, Self::Van, Self::Truck, Self::Bus];

    /// Draws a class by the weights `mix` (one `next_u32`).
    pub fn draw(rng: &mut ChaCha8Rng, mix: &[u32; 4]) -> Self {
        Self::ALL[draw_index(rng, mix)]
    }

    /// The class's dimensions and limits.
    #[must_use]
    pub fn spec(self) -> Spec {
        match self {
            Self::Car => car::SPEC,
            Self::Van => van::SPEC,
            Self::Truck => truck::SPEC,
            Self::Bus => bus::SPEC,
        }
    }

    /// `d`'s driver in a vehicle of this class: vehicle limits cap, driving
    /// style scales (ADR-0007 §3).
    #[must_use]
    pub fn apply(self, mut d: Driver) -> Driver {
        let s = self.spec();
        let p = &mut d.idm;
        p.length_m = s.length_m;
        p.max_accel_mps2 = p.max_accel_mps2.min(s.max_accel_mps2);
        p.comfort_decel_mps2 = p.comfort_decel_mps2.min(s.comfort_decel_mps2);
        p.sqrt_ab = (p.max_accel_mps2 * p.comfort_decel_mps2).sqrt();
        p.time_headway_s *= s.headway_scale;
        p.min_gap_m *= s.min_gap_scale;
        p.max_speed_mps = p.max_speed_mps.min(s.max_speed_mps);
        d.mobil.bias_right_mps2 += s.keep_right_mps2;
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_chacha::rand_core::SeedableRng;

    #[test]
    fn default_mix_draws_its_shares() {
        let mut r = ChaCha8Rng::seed_from_u64(7);
        r.set_stream(2);
        let mut n = [0u32; 4];
        for _ in 0..100_000 {
            let c = Class::draw(&mut r, &DEFAULT_MIX);
            n[Class::ALL.iter().position(|&k| k == c).unwrap()] += 1;
        }
        for (got, want) in n.iter().zip(DEFAULT_MIX) {
            assert!(got.abs_diff(want * 1000) < 600, "{n:?}"); // ±0.6 points
        }
    }
}
