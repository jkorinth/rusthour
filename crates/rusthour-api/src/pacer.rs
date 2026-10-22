//! Real-time stepping policy for frontends (ADR-0004 §3). Pure logic: the
//! caller passes the time in, so it is testable and wasm-safe.

/// Steps the simulation at a chosen time scale without starving the frame.
#[derive(Debug, Clone)]
pub struct Pacer {
    dt_s: f64,
    budget_s: f64,
    scale: f64,
    paused: bool,
    /// Steps owed (fractional); never carries more than the budget allows.
    backlog: f64,
    single: bool,
    achieved: f64,
    /// Current measuring window: real seconds, sim seconds owed, sim seconds
    /// dropped by the step cap.
    win_real: f64,
    win_owed: f64,
    win_dropped: f64,
}

/// Browser frame budget for stepping (single-threaded wasm blocks painting).
pub const DEFAULT_BUDGET_S: f64 = 0.006;
/// Real seconds over which [`Pacer::achieved_scale`] is measured.
const WINDOW_S: f64 = 0.5;
/// Longest frame honoured: a hidden tab or a hiccup must not cause a burst.
pub const MAX_FRAME_S: f64 = 0.25;

impl Pacer {
    /// A pacer for steps of `dt_ms`, running at 1x with the default budget.
    #[must_use]
    pub fn new(dt_ms: u32) -> Self {
        Self {
            dt_s: f64::from(dt_ms) / 1000.0,
            budget_s: DEFAULT_BUDGET_S,
            scale: 1.0,
            paused: false,
            backlog: 0.0,
            single: false,
            achieved: 1.0,
            win_real: 0.0,
            win_owed: 0.0,
            win_dropped: 0.0,
        }
    }

    /// Sets the wanted sim-seconds per real second (non-positive values are ignored).
    pub fn set_scale(&mut self, scale: f64) {
        if scale > 0.0 && scale.is_finite() {
            self.scale = scale;
            self.achieved = scale;
        }
    }

    /// The wanted time scale.
    #[must_use]
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// Stops stepping.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Resumes with an empty backlog: no burst to catch up on the pause.
    pub fn resume(&mut self) {
        if self.paused {
            self.paused = false;
            self.backlog = 0.0;
            self.achieved = self.scale;
            self.win_real = 0.0;
            self.win_owed = 0.0;
            self.win_dropped = 0.0;
        }
    }

    /// Whether stepping is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// While paused, makes the next [`frame`](Self::frame) run exactly one step.
    pub fn step_once(&mut self) {
        self.single = self.paused;
    }

    /// Sim-seconds per real second actually reached, measured over the last
    /// half second; below the wanted scale only when the step budget dropped
    /// steps. Meaningless while paused.
    #[must_use]
    pub fn achieved_scale(&self) -> f64 {
        self.achieved
    }

    /// Sim time since the last step, in `[0, dt)` seconds. The viewer
    /// extrapolates positions by `speed * since_step_s()` at 1x or slower, so
    /// motion stays smooth between steps. Zero while paused.
    #[must_use]
    pub fn since_step_s(&self) -> f64 {
        if self.paused {
            0.0
        } else {
            self.backlog * self.dt_s
        }
    }

    /// How many steps to run now, after `frame_s` real seconds since the last
    /// frame (clamped to [`MAX_FRAME_S`]), given the measured cost of one step
    /// (`0` if not yet known: then at most one step runs). Runs at most
    /// `budget / step_cost` steps (at least one, or a slow step would stall the
    /// clock) and drops any backlog beyond that.
    pub fn frame(&mut self, frame_s: f64, step_cost_s: f64) -> u64 {
        if self.paused {
            return u64::from(std::mem::take(&mut self.single));
        }
        let frame_s = frame_s.clamp(0.0, MAX_FRAME_S);
        self.backlog += frame_s * self.scale / self.dt_s;
        let cap = if step_cost_s > 0.0 {
            (self.budget_s / step_cost_s).floor().max(1.0)
        } else {
            1.0
        };
        let whole = self.backlog.floor();
        let steps = whole.min(cap);
        // The fraction stays owed; steps past the cap are dropped.
        self.backlog = if whole > cap {
            0.0
        } else {
            self.backlog - steps
        };
        self.win_real += frame_s;
        self.win_owed += frame_s * self.scale;
        self.win_dropped += (whole - steps) * self.dt_s;
        if self.win_real >= WINDOW_S {
            self.achieved = (self.win_owed - self.win_dropped) / self.win_real;
            self.win_real = 0.0;
            self.win_owed = 0.0;
            self.win_dropped = 0.0;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "0 <= steps <= cap"
        )]
        let n = steps as u64;
        n
    }
}

/// Time scale for a log-scale slider position: `10^t` (`t = 0` is real time).
#[must_use]
pub fn scale_from_slider(t: f64) -> f64 {
    10f64.powf(t)
}

/// Inverse of [`scale_from_slider`].
#[must_use]
pub fn slider_from_scale(scale: f64) -> f64 {
    scale.log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_follow_scale_and_budget_caps_them() {
        let mut p = Pacer::new(100);
        p.set_scale(10.0);
        // 160 ms frame at 10x: 16 s sim = 16 steps; cheap steps all run.
        assert_eq!(p.frame(0.16, 0.0001), 16);
        // 1 ms per step: only 6 fit; the rest is dropped, not carried.
        assert_eq!(p.frame(0.16, 0.001), 6);
        assert_eq!(p.frame(0.0, 0.001), 0);
    }

    #[test]
    fn reports_achieved_speed_when_limited() {
        let mut p = Pacer::new(100);
        p.set_scale(100.0);
        for _ in 0..200 {
            p.frame(0.016, 0.001); // 6 steps = 0.6 sim s per 16 ms = 37.5x
        }
        assert!(
            (p.achieved_scale() - 37.5).abs() < 0.5,
            "{}",
            p.achieved_scale()
        );
    }

    #[test]
    fn no_burst_after_pause_and_single_step_while_paused() {
        let mut p = Pacer::new(100);
        p.pause();
        assert_eq!(p.frame(5.0, 0.0001), 0);
        p.step_once();
        assert_eq!(p.frame(5.0, 0.0001), 1);
        assert_eq!(p.frame(5.0, 0.0001), 0);
        p.resume();
        assert_eq!(p.frame(0.016, 0.0001), 0); // 1.6 steps owed at 1x: one whole step needs 100 ms
        assert!(p.frame(0.1, 0.0001) <= 2);
    }

    #[test]
    fn achieved_speed_is_steady_when_the_budget_is_not_limiting() {
        for scale in [1.0, 0.1] {
            let mut p = Pacer::new(100);
            p.set_scale(scale);
            for i in 0..2000 {
                p.frame(0.016, 0.0001);
                let a = p.achieved_scale();
                assert!((a - scale).abs() <= 0.05 * scale, "{scale}x frame {i}: {a}");
            }
        }
    }

    #[test]
    fn long_frames_are_clamped_and_unknown_cost_runs_one_step() {
        let mut p = Pacer::new(100);
        p.set_scale(100.0);
        // 30 s hidden tab: 0.25 s at 100x = 250 steps, not 30000.
        assert_eq!(p.frame(30.0, 0.00001), 250);
        assert_eq!(p.frame(0.25, 0.0), 1);
    }

    #[test]
    fn since_step_tracks_the_fraction_of_a_step() {
        let mut p = Pacer::new(100);
        assert_eq!(p.frame(0.16, 0.0001), 1);
        assert!((p.since_step_s() - 0.06).abs() < 1e-9);
        p.pause();
        assert!(p.since_step_s().abs() < 1e-12);
    }

    #[test]
    fn slider_is_log_scale() {
        assert!((scale_from_slider(-1.0) - 0.1).abs() < 1e-12);
        assert!((slider_from_scale(scale_from_slider(1.5)) - 1.5).abs() < 1e-12);
    }
}
