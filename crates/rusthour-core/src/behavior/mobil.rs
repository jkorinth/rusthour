//! MOBIL lane changing, European asymmetric variant (Treiber and Kesting;
//! ADR-0005 §6). The engine gathers the accelerations; this decides.

/// Per-vehicle MOBIL parameters, a column of the vehicle store.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MobilParams {
    /// Politeness `p`: weight of the two followers' gain against one's own.
    pub politeness: f64,
    /// Changing threshold `Δa_th` (m/s²).
    pub threshold_mps2: f64,
    /// Keep-right bias `Δa_bias` (m/s²): added to the threshold to the left,
    /// subtracted to the right.
    pub bias_right_mps2: f64,
    /// Safe braking `b_safe` (m/s²): the most the new follower may have to brake.
    pub safe_decel_mps2: f64,
}

impl Default for MobilParams {
    fn default() -> Self {
        Self {
            politeness: 0.2,
            threshold_mps2: 0.1,
            bias_right_mps2: 0.3,
            safe_decel_mps2: 4.0,
        }
    }
}

/// IDM accelerations `[before, after]` the change (m/s²) of the changing
/// vehicle `c`, its new follower `n` and its old follower `o`; `[0, 0]` for
/// a follower that doesn't exist.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Accels {
    /// The changing vehicle.
    pub c: [f64; 2],
    /// The new follower, in the target lane.
    pub n: [f64; 2],
    /// The old follower, in the current lane.
    pub o: [f64; 2],
}

/// How far the incentive of a change to the left (`left`) or the right
/// clears its threshold: positive means change. Safety is the caller's.
#[must_use]
pub fn margin(p: &MobilParams, left: bool, a: &Accels) -> f64 {
    let gain = |x: [f64; 2]| x[1] - x[0];
    let incentive = gain(a.c) + p.politeness * (gain(a.n) + gain(a.o));
    let bias = if left {
        p.bias_right_mps2
    } else {
        -p.bias_right_mps2
    };
    incentive - (p.threshold_mps2 + bias)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_right_bias_is_asymmetric_and_politeness_sums_both_followers() {
        let p = MobilParams::default();
        // No gain anywhere: right clears its threshold (0.1 − 0.3), left doesn't.
        let none = Accels::default();
        assert!(margin(&p, false, &none) > 0.0 && margin(&p, true, &none) < 0.0);
        // A gain of 0.45 clears the left threshold of 0.4 …
        let own = Accels {
            c: [0.0, 0.45],
            ..Accels::default()
        };
        assert!(margin(&p, true, &own) > 0.0);
        // … unless both followers together lose more than 0.25 (p = 0.2).
        let rude = Accels {
            n: [0.0, -0.15],
            o: [0.0, -0.15],
            ..own
        };
        assert!(margin(&p, true, &rude) < 0.0);
    }
}
