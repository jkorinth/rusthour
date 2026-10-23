//! The running simulation behind the map view: stepping through the
//! [`Pacer`], one snapshot per frame, reset with the same seed (ADR-0004 §3).

use rusthour_api::{
    Config, InProcess, Network, NetworkError, Pacer, Simulation, Status, VehicleSnapshot,
    scale_from_slider,
};
use web_time::Instant;

/// Every map starts from this seed, so a reset replays the same traffic.
pub const SEED: u64 = 0;
/// Road length per vehicle in the default population: about 5,000 vehicles
/// on bensheim-5km (250 km of directed roads), the ADR-0003 test scene.
const ROAD_M_PER_VEHICLE: f64 = 50.0;
/// Speed slider range, as exponents of ten: 0.1× to 1000×.
pub const SLIDER_RANGE: std::ops::RangeInclusive<f64> = -1.0..=3.0;
/// Below this share of the wanted speed, the readout shows the achieved one.
const SHOW_ACHIEVED_BELOW: f64 = 0.9;

/// A simulation, its pacing and its latest snapshot.
pub struct Traffic {
    sim: InProcess,
    config: Config,
    pub pacer: Pacer,
    /// Taken once per frame, after stepping.
    pub snapshot: VehicleSnapshot,
    /// Smoothed real seconds per step, `0` until measured.
    pub step_cost_s: f64,
    /// Steps run in the last frame.
    pub last_steps: u64,
}

impl Traffic {
    /// Starts a simulation on `network` with a population suited to its size.
    pub fn new(network: Network) -> Result<Self, NetworkError> {
        let config = Config {
            target_vehicles: default_vehicles(&network),
            ..Config::default()
        };
        Ok(Self {
            sim: InProcess::with_network(config, SEED, network)?,
            config,
            pacer: Pacer::new(config.dt_ms),
            snapshot: VehicleSnapshot::default(),
            step_cost_s: 0.0,
            last_steps: 0,
        })
    }

    /// The network the simulation runs on.
    pub fn network(&self) -> &Network {
        self.sim.network()
    }

    /// The current status, for the readouts.
    pub fn status(&self) -> Status {
        self.sim.status()
    }

    /// Vehicles the demand keeps on the network (active plus waiting).
    pub fn target_vehicles(&self) -> u32 {
        self.config.target_vehicles
    }

    /// Restarts from tick 0 with the same seed, network and population; keeps
    /// the speed and whether it is paused.
    pub fn reset(&mut self) {
        let network = self.sim.network().clone();
        self.sim = InProcess::with_network(self.config, SEED, network)
            .expect("validated when the map was loaded");
        self.sim.snapshot_into(&mut self.snapshot);
    }

    /// Changes the population, which restarts the simulation.
    pub fn set_target_vehicles(&mut self, n: u32) {
        self.config.target_vehicles = n;
        self.reset();
    }

    /// Runs the steps the pacer allows after `frame_s` real seconds, then
    /// takes the frame's snapshot.
    pub fn frame(&mut self, frame_s: f64) {
        self.last_steps = self.pacer.frame(frame_s, self.step_cost_s);
        if self.last_steps > 0 {
            let start = Instant::now();
            self.sim.step(self.last_steps);
            #[expect(clippy::cast_precision_loss, reason = "a few hundred steps")]
            let cost = start.elapsed().as_secs_f64() / self.last_steps as f64;
            // Smoothed, so one slow frame doesn't halve the next one's steps.
            self.step_cost_s = if self.step_cost_s > 0.0 {
                0.8 * self.step_cost_s + 0.2 * cost
            } else {
                cost
            };
        }
        self.sim.snapshot_into(&mut self.snapshot);
    }

    /// Sim seconds to extrapolate the snapshot by when drawing: at 1× or
    /// slower motion stays smooth between steps; above, several steps land in
    /// a frame and the snapshot is drawn as is (ADR-0004 §3).
    #[expect(clippy::cast_possible_truncation, reason = "less than one step")]
    pub fn ahead_s(&self) -> f32 {
        if self.pacer.scale() <= 1.0 {
            self.pacer.since_step_s() as f32
        } else {
            0.0
        }
    }

    /// Play or pause.
    pub fn toggle_pause(&mut self) {
        if self.pacer.is_paused() {
            self.pacer.resume();
        } else {
            self.pacer.pause();
        }
    }

    /// Pauses, then runs exactly one step on the next frame.
    pub fn step_once(&mut self) {
        self.pacer.pause();
        self.pacer.step_once();
    }
}

/// One vehicle per [`ROAD_M_PER_VEHICLE`] of road, at least one.
pub fn default_vehicles(network: &Network) -> u32 {
    let length_m: f64 = network
        .roads
        .iter()
        .flat_map(|r| r.geometry.windows(2))
        .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
        .sum();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "non-negative, far below u32::MAX"
    )]
    let n = (length_m / ROAD_M_PER_VEHICLE).round() as u32;
    n.max(1)
}

/// `0.1×`, `2.5×`, `37×`: two significant digits below 10, whole numbers above.
pub fn format_scale(scale: f64) -> String {
    let digits = if scale >= 10.0 {
        0
    } else if scale >= 1.0 {
        1
    } else {
        2
    };
    let s = format!("{scale:.digits$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    format!("{s}×")
}

/// The speed readout: the wanted scale, and the achieved one when the step
/// budget holds it below 90%, e.g. "100× (running at 37×)".
pub fn speed_label(wanted: f64, achieved: f64, paused: bool) -> String {
    if !paused && achieved < SHOW_ACHIEVED_BELOW * wanted {
        format!(
            "{} (running at {})",
            format_scale(wanted),
            format_scale(achieved)
        )
    } else {
        format_scale(wanted)
    }
}

/// The sim clock, `h:mm:ss.s`.
pub fn format_clock(sim_time_ms: u64) -> String {
    let tenths = sim_time_ms / 100;
    let (h, m) = (tenths / 36_000, tenths / 600 % 60);
    let (s, t) = (tenths / 10 % 60, tenths % 10);
    format!("{h}:{m:02}:{s:02}.{t}")
}

/// The time scale at slider position `t`, clamped to [`SLIDER_RANGE`].
pub fn scale_at(t: f64) -> f64 {
    scale_from_slider(t.clamp(*SLIDER_RANGE.start(), *SLIDER_RANGE.end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn traffic() -> Traffic {
        Traffic::new(crate::demo::map().network).expect("the demo network is valid")
    }

    /// Runs `n` frames of 0.1 s at 1×, i.e. one step each.
    fn run(t: &mut Traffic, n: usize) {
        for _ in 0..n {
            t.frame(0.1);
        }
    }

    #[test]
    fn slider_is_log_scale_from_a_tenth_to_a_thousand() {
        assert!((scale_at(-1.0) - 0.1).abs() < 1e-12);
        assert!((scale_at(0.0) - 1.0).abs() < 1e-12);
        assert!((scale_at(2.0) - 100.0).abs() < 1e-9);
        assert!((scale_at(9.0) - 1000.0).abs() < 1e-9, "clamped");
        assert!((scale_at(-5.0) - 0.1).abs() < 1e-12, "clamped");
        // Equal slider distances are equal factors.
        assert!((scale_at(1.5) / scale_at(1.0) - scale_at(0.5)).abs() < 1e-9);
    }

    #[test]
    fn speed_label_shows_the_achieved_speed_only_when_held_back() {
        assert_eq!(speed_label(100.0, 37.4, false), "100× (running at 37×)");
        assert_eq!(speed_label(100.0, 95.0, false), "100×");
        assert_eq!(speed_label(100.0, 0.0, true), "100×");
        assert_eq!(speed_label(0.1, 0.1, false), "0.1×");
        assert_eq!(speed_label(2.5, 2.5, false), "2.5×");
        assert_eq!(format_clock(3_723_400), "1:02:03.4");
    }

    #[test]
    fn reset_replays_the_same_traffic() {
        let mut t = traffic();
        assert!(t.target_vehicles() > 50, "{}", t.target_vehicles());
        run(&mut t, 300);
        let first = t.snapshot.clone();
        assert!(!first.is_empty(), "the demo map shows traffic");
        assert_eq!(first.tick, 300);
        t.reset();
        assert_eq!(t.status().tick, 0);
        run(&mut t, 300);
        assert_eq!(t.snapshot, first);
    }

    #[test]
    fn pause_and_single_step_drive_the_pacer() {
        let mut t = traffic();
        run(&mut t, 3);
        assert_eq!(t.status().tick, 3);
        t.toggle_pause();
        assert!(t.pacer.is_paused());
        run(&mut t, 5);
        assert_eq!(t.status().tick, 3, "paused");
        assert!(
            t.ahead_s().abs() < f32::EPSILON,
            "no extrapolation while paused"
        );
        t.step_once();
        run(&mut t, 5);
        assert_eq!(t.status().tick, 4, "exactly one step");
        assert_eq!(t.snapshot.tick, 4, "snapshot after stepping");
        t.toggle_pause();
        run(&mut t, 2);
        assert_eq!(t.status().tick, 6, "running again, no burst");
        // Step while running pauses first.
        t.step_once();
        run(&mut t, 3);
        assert!(t.pacer.is_paused());
        assert_eq!(t.status().tick, 7);
    }
}
