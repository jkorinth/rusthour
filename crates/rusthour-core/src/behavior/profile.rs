//! Driver profiles (ADR-0006): each sets a vehicle's IDM and MOBIL
//! parameters at spawn; erratic drivers also get parameter jitter and an
//! acceleration noise, and each its gap acceptance (ADR-0009). Each profile is a submodule; the `match` arms here are
//! the only shared lines.

mod aggressive;
mod cautious;
mod erratic;
mod normal;

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::Rng;

use crate::behavior::mobil::MobilParams;
use crate::vehicle::idm::IdmParams;

/// A driver's style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    /// Short headway, brisk, egoistic lane changes.
    Aggressive,
    /// The M2 car.
    #[default]
    Normal,
    /// Long headway, gentle, polite.
    Cautious,
    /// Inconsistent: jittered parameters and an acceleration noise.
    Erratic,
}

/// What a profile sets at spawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Driver {
    /// Car-following parameters.
    pub idm: IdmParams,
    /// Lane-changing parameters.
    pub mobil: MobilParams,
    /// Gap acceptance at roundabout entries (ADR-0009).
    pub gap: Gap,
}

/// How long a gap in circulating traffic a driver accepts (ADR-0009). The
/// follow-up time is not a parameter: it emerges from car following.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gap {
    /// Critical gap `tc`: the shortest time to the next circulating vehicle
    /// a driver at the line enters in (s).
    pub critical_s: f64,
}

/// The parameters of a profile a run can tune (ADR-0006, M4.4 addendum):
/// the IDM ones and MOBIL politeness. Erratic drivers draw theirs around
/// these: IDM values × U(0.8, 1.2), politeness U(0, 2 × `politeness`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// Desired speed as a factor of the speed limit.
    pub desired_speed_factor: f64,
    /// Time headway `T` (s).
    pub time_headway_s: f64,
    /// Maximum acceleration `a` (m/s²).
    pub max_accel_mps2: f64,
    /// Comfortable deceleration `b` (m/s²).
    pub comfort_decel_mps2: f64,
    /// Minimum gap `s0` (m).
    pub min_gap_m: f64,
    /// MOBIL politeness `p`.
    pub politeness: f64,
}

/// Default mix in percent, in [`Profile::ALL`] order ([`crate::Config`]).
pub const DEFAULT_MIX: [u32; 4] = [15, 60, 20, 5];

/// The car's length; vehicle classes (ADR-0007) set others after the profile.
const CAR_LENGTH_M: f64 = 4.5;

impl Profile {
    /// All profiles, in [`DEFAULT_MIX`] order.
    pub const ALL: [Self; 4] = [
        Self::Aggressive,
        Self::Normal,
        Self::Cautious,
        Self::Erratic,
    ];

    /// Draws a profile by the weights `mix` (one `next_u32`).
    pub fn draw(rng: &mut ChaCha8Rng, mix: &[u32; 4]) -> Self {
        Self::ALL[draw_index(rng, mix)]
    }

    /// The ADR-0006 values; erratic politeness is the mean of its draw.
    #[must_use]
    pub fn tuning(self) -> Tuning {
        let d = self.untuned();
        let p = &d.idm;
        Tuning {
            desired_speed_factor: p.desired_speed_factor,
            time_headway_s: p.time_headway_s,
            max_accel_mps2: p.max_accel_mps2,
            comfort_decel_mps2: p.comfort_decel_mps2,
            min_gap_m: p.min_gap_m,
            politeness: match self {
                Self::Erratic => erratic::MEAN_POLITENESS,
                _ => d.mobil.politeness,
            },
        }
    }

    fn untuned(self) -> Driver {
        match self {
            Self::Aggressive => aggressive::driver(),
            Self::Normal | Self::Erratic => normal::driver(),
            Self::Cautious => cautious::driver(),
        }
    }

    /// The profile's parameters without jitter.
    #[must_use]
    pub fn base(self) -> Driver {
        self.base_with(&self.tuning())
    }

    /// The parameters of tuning `t` without jitter.
    #[must_use]
    pub fn base_with(self, t: &Tuning) -> Driver {
        let mut d = self.untuned();
        d.idm = idm(
            t.desired_speed_factor,
            t.time_headway_s,
            t.max_accel_mps2,
            t.comfort_decel_mps2,
            t.min_gap_m,
        );
        d.mobil.politeness = t.politeness;
        d
    }

    /// The parameters of a new driver of this profile; only erratic draws.
    pub fn driver(self, rng: &mut ChaCha8Rng) -> Driver {
        self.driver_with(&self.tuning(), rng)
    }

    /// The parameters of a new driver with tuning `t`; only erratic draws.
    pub fn driver_with(self, t: &Tuning, rng: &mut ChaCha8Rng) -> Driver {
        match self {
            Self::Erratic => erratic::driver(t, rng),
            _ => self.base_with(t),
        }
    }

    /// A fresh acceleration offset (m/s²): 0 without drawing, unless erratic.
    pub fn noise_mps2(self, rng: &mut ChaCha8Rng) -> f64 {
        match self {
            Self::Erratic => erratic::noise_mps2(rng),
            _ => 0.0,
        }
    }
}

/// Draws an index by the weights `mix` (one `next_u32`). Only their ratios
/// matter: `k < upto` below is `u < upto / total · 2³²` exactly, so
/// percentages and parts per million of the same mix draw alike. All-zero
/// weights give index 0; the api rejects them.
pub(crate) fn draw_index(rng: &mut ChaCha8Rng, mix: &[u32]) -> usize {
    let total: u128 = mix.iter().map(|&w| u128::from(w)).sum();
    // 0..total without float rounding or overflow; the cumulative weights
    // pick the index.
    let k = (u128::from(rng.next_u32()) * total) >> 32;
    let mut upto = 0;
    mix.iter()
        .position(|&share| {
            upto += u128::from(share);
            k < upto
        })
        .unwrap_or(0)
}

/// A uniform float in `[0, 1)` from the top 53 bits of one draw (ADR-0004).
#[allow(clippy::cast_precision_loss)] // 53 bits are exact in f64
fn unit(rng: &mut ChaCha8Rng) -> f64 {
    (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// IDM parameters with `√(a·b)` filled in.
fn idm(desired_speed_factor: f64, t: f64, a: f64, b: f64, s0: f64) -> IdmParams {
    IdmParams {
        desired_speed_factor,
        time_headway_s: t,
        max_accel_mps2: a,
        comfort_decel_mps2: b,
        min_gap_m: s0,
        length_m: CAR_LENGTH_M,
        max_speed_mps: f64::INFINITY,
        sqrt_ab: (a * b).sqrt(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vehicle::idm::{Leader, acceleration};
    use proptest::prelude::*;
    use rand_chacha::rand_core::SeedableRng;

    fn rng(seed: u64) -> ChaCha8Rng {
        let mut r = ChaCha8Rng::seed_from_u64(seed);
        r.set_stream(2);
        r
    }

    /// Gap a follower of `p` settles at behind a leader at 10 m/s on a
    /// 50 km/h road, after starting 100 m behind it at 10 m/s.
    fn settled_gap(p: Profile) -> f64 {
        let d = p.base().idm;
        let (v0, dt, lead_v) = (13.9 * d.desired_speed_factor, 0.1, 10.0);
        let (mut gap, mut v) = (100.0, 10.0);
        for _ in 0..3000 {
            let a = acceleration(
                &d,
                v0,
                v,
                Some(Leader {
                    gap_m: gap,
                    speed_mps: lead_v,
                }),
            );
            let v1 = (v + a * dt).max(0.0);
            gap += (lead_v - f64::midpoint(v, v1)) * dt;
            v = v1;
        }
        gap
    }

    #[test]
    fn aggressive_follows_closer_than_normal_and_normal_closer_than_cautious() {
        let [aggressive, normal, cautious] =
            [Profile::Aggressive, Profile::Normal, Profile::Cautious].map(settled_gap);
        assert!(
            aggressive + 3.0 < normal && normal + 3.0 < cautious,
            "{aggressive} {normal} {cautious}"
        );
    }

    #[test]
    fn default_mix_draws_its_shares() {
        let mut r = rng(7);
        let mut n = [0u32; 4];
        for _ in 0..100_000 {
            let p = Profile::draw(&mut r, &DEFAULT_MIX);
            n[Profile::ALL.iter().position(|&q| q == p).unwrap()] += 1;
        }
        for (got, want) in n.iter().zip(DEFAULT_MIX) {
            assert!(got.abs_diff(want * 1000) < 600, "{n:?}"); // ±0.6 points
        }
    }

    #[test]
    fn draws_depend_only_on_the_ratios_of_the_weights() {
        let (mut a, mut b) = (rng(3), rng(3));
        let scaled = DEFAULT_MIX.map(|w| w * 10_000);
        for _ in 0..10_000 {
            assert_eq!(
                Profile::draw(&mut a, &DEFAULT_MIX),
                Profile::draw(&mut b, &scaled)
            );
            assert_eq!(Profile::draw(&mut b, &[0, 0, 7, 0]), Profile::Cautious);
            a.next_u32();
        }
    }

    proptest! {
        /// Erratic jitter and noise depend only on the seed and stay in
        /// their bounds; the other profiles never draw.
        #[test]
        fn erratic_draws_are_deterministic_and_bounded(seed: u64) {
            let (mut r1, mut r2) = (rng(seed), rng(seed));
            let d = Profile::Erratic.driver(&mut r1);
            prop_assert_eq!(d, Profile::Erratic.driver(&mut r2));
            let n = Profile::Normal.base();
            let within = |x: f64, base: f64| (0.8 * base..=1.2 * base).contains(&x);
            prop_assert!(within(d.idm.desired_speed_factor, n.idm.desired_speed_factor));
            prop_assert!(within(d.idm.time_headway_s, n.idm.time_headway_s));
            prop_assert!(within(d.idm.max_accel_mps2, n.idm.max_accel_mps2));
            prop_assert!(within(d.idm.comfort_decel_mps2, n.idm.comfort_decel_mps2));
            prop_assert!(within(d.idm.min_gap_m, n.idm.min_gap_m));
            prop_assert!((0.0..=0.4).contains(&d.mobil.politeness));
            for _ in 0..20 {
                let x = Profile::Erratic.noise_mps2(&mut r1);
                prop_assert_eq!(x.to_bits(), Profile::Erratic.noise_mps2(&mut r2).to_bits());
                // Below the lowest erratic `a`, so it never cancels a start.
                prop_assert!(x.abs() <= erratic::NOISE_MPS2 && erratic::NOISE_MPS2 < 0.8 * 1.5);
            }
            let pos = r1.get_word_pos();
            for p in [Profile::Aggressive, Profile::Normal, Profile::Cautious] {
                prop_assert_eq!(p.driver(&mut r1), p.base());
                prop_assert_eq!(p.noise_mps2(&mut r1).to_bits(), 0);
            }
            prop_assert_eq!(r1.get_word_pos(), pos);
        }
    }
}
