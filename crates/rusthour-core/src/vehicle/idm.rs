//! Intelligent Driver Model (Treiber et al.), exponent δ = 4 (ADR-0004).

/// Per-vehicle IDM parameters; one value per vehicle so profiles and noise
/// (M4) need no lookup table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IdmParams {
    /// `v0 = factor × road speed limit`.
    pub desired_speed_factor: f64,
    /// Desired time headway `T` (s).
    pub time_headway_s: f64,
    /// Maximum acceleration `a` (m/s²).
    pub max_accel_mps2: f64,
    /// Comfortable deceleration `b` (m/s²).
    pub comfort_decel_mps2: f64,
    /// Minimum gap `s0` (m).
    pub min_gap_m: f64,
    /// Vehicle length (m).
    pub length_m: f64,
    /// `√(a·b)`, computed once at spawn.
    pub sqrt_ab: f64,
}

/// The vehicle or standing obstacle ahead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leader {
    /// Bumper-to-bumper gap (m), > 0.
    pub gap_m: f64,
    /// Leader speed (m/s); 0 for a standing obstacle.
    pub speed_mps: f64,
}

/// IDM acceleration (m/s²) for a vehicle at speed `v` with desired speed
/// `v0`; `leader` is `None` on a free road. Unclamped below: the engine
/// clamps to its braking limit.
#[must_use]
pub fn acceleration(p: &IdmParams, v0: f64, v: f64, leader: Option<Leader>) -> f64 {
    let ratio = v / v0;
    let ratio2 = ratio * ratio;
    let free = 1.0 - ratio2 * ratio2;
    let Some(lead) = leader else {
        return p.max_accel_mps2 * free;
    };
    let s_star = p.min_gap_m + v * p.time_headway_s + v * (v - lead.speed_mps) / (2.0 * p.sqrt_ab);
    // shortcut: gap floor avoids inf at contact, revisit if the engine ever lets gaps reach 0.
    let inter = s_star.max(p.min_gap_m) / lead.gap_m.max(1e-3);
    p.max_accel_mps2 * (free - inter * inter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn params(a: f64, b: f64, t: f64, s0: f64) -> IdmParams {
        IdmParams {
            desired_speed_factor: 1.0,
            time_headway_s: t,
            max_accel_mps2: a,
            comfort_decel_mps2: b,
            min_gap_m: s0,
            length_m: 4.5,
            sqrt_ab: (a * b).sqrt(),
        }
    }

    fn car() -> IdmParams {
        params(1.5, 2.0, 1.5, 2.0)
    }

    #[test]
    fn free_road_converges_to_desired_speed() {
        let (p, v0, dt) = (car(), 14.0, 0.1);
        let mut v = 0.0;
        for _ in 0..3000 {
            v += acceleration(&p, v0, v, None) * dt;
        }
        assert!((v - v0).abs() < 0.01 && v <= v0, "v = {v}");
    }

    #[test]
    fn stops_at_standing_obstacle_with_at_least_min_gap() {
        let (p, v0, dt) = (car(), 14.0, 0.1);
        let (mut x, mut v) = (0.0, 14.0);
        let obstacle = 150.0;
        for _ in 0..3000 {
            let a = acceleration(
                &p,
                v0,
                v,
                Some(Leader {
                    gap_m: obstacle - x,
                    speed_mps: 0.0,
                }),
            )
            .max(-9.0);
            // Ballistic integration with non-negative speed, as in ADR-0004.
            if v + a * dt < 0.0 {
                x += -v * v / (2.0 * a);
                v = 0.0;
            } else {
                x += v * dt + a * dt * dt / 2.0;
                v += a * dt;
            }
        }
        assert!(v < 0.01, "v = {v}");
        // 1 mm slack for the discrete-step approach to s0.
        assert!(obstacle - x >= p.min_gap_m - 1e-3, "gap = {}", obstacle - x);
    }

    #[test]
    fn equilibrium_gap_gives_zero_acceleration() {
        let (p, v0) = (car(), 14.0);
        for v in [0.0, 5.0, 10.0] {
            let gap = (p.min_gap_m + v * p.time_headway_s) / (1.0 - (v / v0).powi(4)).sqrt();
            let a = acceleration(
                &p,
                v0,
                v,
                Some(Leader {
                    gap_m: gap,
                    speed_mps: v,
                }),
            );
            assert!(a.abs() < 1e-9, "v = {v}, a = {a}");
        }
    }

    proptest! {
        #[test]
        fn acceleration_is_finite_and_at_most_max(
            a in 0.5..4.0f64, b in 0.5..4.0f64, t in 0.5..3.0f64, s0 in 0.5..5.0f64,
            v0 in 1.0..40.0f64, v in 0.0..45.0f64,
            gap in 0.01..500.0f64, lv in 0.0..45.0f64, has_leader: bool,
        ) {
            let p = params(a, b, t, s0);
            let leader = has_leader.then_some(Leader { gap_m: gap, speed_mps: lv });
            let acc = acceleration(&p, v0, v, leader);
            prop_assert!(acc.is_finite() && acc <= a);
        }
    }
}
