//! The running simulation behind the map view: stepping through the
//! [`Pacer`], one snapshot per frame, reset with the same seed (ADR-0004 §3).

use rusthour_api::{
    Config, DriverMix, DriverTuning, FleetMix, InProcess, Network, NetworkError, Pacer, RoadId,
    SignalSnapshot, Simulation, Status, TUNING_FIELDS, VehicleSnapshot, scale_from_slider,
};
use web_time::Instant;

/// Every map starts from this seed, so a reset replays the same traffic.
pub const SEED: u64 = 0;
/// Road length per vehicle in the default population: about 4,000 vehicles
/// on bensheim-5km (250 km of directed roads). At 50 m (5,000 vehicles, the
/// ADR-0003 performance scene) the central ring still jams: 7.3% stuck removals per arrival
/// (stuck-vehicle net), against 3.5% here (ADR-0004, M2.9 addendum).
const ROAD_M_PER_VEHICLE: f64 = 60.0;
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
    /// Roads with stuck removals and their counts, refreshed with the snapshot.
    pub stuck_roads: Vec<(RoadId, u32)>,
    /// Signal states, refreshed with the snapshot.
    pub signals: SignalSnapshot,
    /// Smoothed real seconds per step, `0` until measured.
    pub step_cost_s: f64,
    /// Steps run in the last frame.
    pub last_steps: u64,
    /// Counts restarts: vehicle slots mean something else afterwards.
    pub epoch: u32,
    /// The driver mix as edited, in percent; the next restart applies it.
    pub drivers: DriverMix,
    /// The fleet mix as edited, in percent; the next restart applies it.
    pub fleet: FleetMix,
    /// Each profile's parameters as edited; the next restart applies them.
    pub tuning: [DriverTuning; 4],
}

impl Traffic {
    /// Starts a simulation on `network` with a population suited to its size.
    pub fn new(network: Network) -> Result<Self, NetworkError> {
        let drivers = DriverMix::from_shares(DriverMix::default().shares().map(percent));
        let fleet = FleetMix::from_shares(FleetMix::default().shares().map(percent));
        let config = Config {
            target_vehicles: default_vehicles(&network),
            // As `edits_pending` compares them; the same ratios as the default.
            profile_mix: drivers.weights().expect("the default is valid"),
            class_mix: fleet.weights().expect("the default is valid"),
            ..Config::default()
        };
        Ok(Self {
            sim: InProcess::with_network(config, SEED, network)?,
            config,
            pacer: Pacer::new(config.dt_ms),
            snapshot: VehicleSnapshot::default(),
            stuck_roads: Vec::new(),
            signals: SignalSnapshot::default(),
            step_cost_s: 0.0,
            last_steps: 0,
            epoch: 0,
            drivers,
            fleet,
            tuning: config.tuning,
        })
    }

    /// The network the simulation runs on.
    pub fn network(&self) -> &Network {
        self.sim.network()
    }

    /// The simulation, for the inspector's queries.
    pub fn sim(&self) -> &InProcess {
        &self.sim
    }

    /// The current status, for the readouts.
    pub fn status(&self) -> Status {
        self.sim.status()
    }

    /// Vehicles the demand keeps on the network (active plus waiting).
    pub fn target_vehicles(&self) -> u32 {
        self.config.target_vehicles
    }

    /// Whether the edited mix or tuning differs from the running one; an
    /// all-zero mix never applies, so it doesn't count.
    pub fn edits_pending(&self) -> bool {
        let differs = |w: Result<[u32; 4], _>, now| w.is_ok_and(|w| w != now);
        differs(self.drivers.weights(), self.config.profile_mix)
            || differs(self.fleet.weights(), self.config.class_mix)
            || self.tuning != self.config.tuning
    }

    /// Whether both edited mixes have a share above 0; "Reset" is off
    /// until they do.
    pub fn mix_valid(&self) -> bool {
        self.drivers.weights().is_ok() && self.fleet.weights().is_ok()
    }

    /// Restarts from tick 0 with the same seed, network and population, the
    /// edited mix, and the edited tuning where it is in range; keeps the
    /// speed and whether it is paused. Does nothing while the mix is
    /// invalid, so a run never uses a mix the panel doesn't show.
    pub fn reset(&mut self) {
        let (Ok(drivers), Ok(fleet)) = (self.drivers.weights(), self.fleet.weights()) else {
            return;
        };
        self.config.profile_mix = drivers;
        self.config.class_mix = fleet;
        let in_range =
            |t: &DriverTuning| TUNING_FIELDS.iter().all(|f| f.range.contains(&(f.get)(t)));
        if self.tuning.iter().all(in_range) {
            self.config.tuning = self.tuning;
        }
        let network = self.sim.network().clone();
        self.sim = InProcess::with_network(self.config, SEED, network)
            .expect("validated when the map was loaded");
        self.epoch = self.epoch.wrapping_add(1);
        self.refresh();
    }

    /// Takes the snapshots the frame draws.
    fn refresh(&mut self) {
        self.sim.snapshot_into(&mut self.snapshot);
        self.sim.stuck_by_road_into(&mut self.stuck_roads);
        self.sim.signals_into(&mut self.signals);
    }

    /// Changes the population, which restarts the simulation; does nothing
    /// while the mix is invalid, as [`Self::reset`].
    pub fn set_target_vehicles(&mut self, n: u32) {
        if !self.mix_valid() {
            return;
        }
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
        self.refresh();
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

/// The "removed (stuck)" counter text, and whether it is tinted: any removal
/// means a gridlock was broken by the safety net (ADR-0005 §8).
pub fn stuck_counter(status: &Status) -> (String, bool) {
    (
        format!("{} removed (stuck)", status.removed_stuck),
        status.removed_stuck > 0,
    )
}

/// Opacity of a hot road's tint: from a visible floor for one removal up to
/// opaque for the road with the most.
pub fn hot_road_alpha(removed: u32, max: u32) -> u8 {
    const FLOOR: f64 = 90.0;
    let share = f64::from(removed) / f64::from(max.max(1));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "0..=255"
    )]
    let alpha = (FLOOR + (255.0 - FLOOR) * share.min(1.0)).round() as u8;
    alpha
}

/// A share as a whole percentage, e.g. 0.15 as 15.
fn percent(share: f64) -> f64 {
    (share * 100.0).round()
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
    fn an_edited_mix_applies_on_reset_unless_it_is_all_zero() {
        use rusthour_api::{FleetMix, VehicleClass};
        let mut t = traffic();
        assert!(!t.edits_pending(), "nothing edited yet");
        let trucks = FleetMix::from_shares([0.0, 0.0, 1.0, 0.0]);
        t.fleet = trucks;
        assert!(t.edits_pending());
        run(&mut t, 300);
        assert!(t.snapshot.class.iter().any(|&c| c != VehicleClass::Truck));
        t.reset();
        assert!(!t.edits_pending());
        run(&mut t, 300);
        assert!(!t.snapshot.is_empty());
        assert!(t.snapshot.class.iter().all(|&c| c == VehicleClass::Truck));
        t.fleet = FleetMix::from_shares([0.0; 4]);
        t.reset();
        run(&mut t, 300);
        assert!(t.snapshot.class.iter().all(|&c| c == VehicleClass::Truck));
    }

    #[test]
    fn regression_m4_13_reset_refuses_an_all_zero_driver_mix() {
        use rusthour_api::{DriverMix, FleetMix, VehicleClass};
        let mut t = traffic();
        run(&mut t, 300);
        let tick = t.status().tick;
        t.drivers = DriverMix::from_shares([0.0; 4]);
        t.fleet = FleetMix::from_shares([0.0, 0.0, 1.0, 0.0]);
        assert!(!t.mix_valid());
        t.reset();
        t.set_target_vehicles(t.target_vehicles() + 10);
        assert_eq!(t.status().tick, tick, "no restart with a mix it can't show");
        run(&mut t, 300);
        assert!(t.snapshot.class.iter().any(|&c| c != VehicleClass::Truck));
        t.drivers = DriverMix::default();
        assert!(t.mix_valid());
        t.reset();
        assert_eq!(t.status().tick, 0);
        run(&mut t, 300);
        assert!(t.snapshot.class.iter().all(|&c| c == VehicleClass::Truck));
    }

    #[test]
    fn edited_tuning_applies_on_reset() {
        use rusthour_api::{DriverMix, FleetMix};
        let mut t = traffic();
        t.drivers = DriverMix::from_shares([0.0, 0.0, 1.0, 0.0]);
        t.fleet = FleetMix::from_shares([1.0, 0.0, 0.0, 0.0]);
        t.tuning[2].time_headway_s = 2.8;
        assert!(t.edits_pending());
        t.reset();
        assert!(!t.edits_pending());
        run(&mut t, 300);
        let id = t.snapshot.id[0];
        let info = t.sim().inspect_vehicle(id).unwrap();
        assert!((info.driver.time_headway_s - 2.8).abs() < 1e-6, "{info:?}");
        // Out of range (the fields clamp, but not every caller): kept as it was.
        t.tuning[2].time_headway_s = 99.0;
        t.reset();
        run(&mut t, 300);
        let info = t.sim().inspect_vehicle(t.snapshot.id[0]).unwrap();
        assert!((info.driver.time_headway_s - 2.8).abs() < 1e-6, "{info:?}");
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

    #[test]
    fn the_stuck_counter_is_tinted_only_above_zero() {
        let base = traffic().status();
        assert_eq!(stuck_counter(&base), ("0 removed (stuck)".into(), false));
        let status = Status {
            removed_stuck: 12,
            ..base
        };
        assert_eq!(stuck_counter(&status), ("12 removed (stuck)".into(), true));
    }

    #[test]
    fn hot_roads_tint_stronger_with_more_removals() {
        assert_eq!(hot_road_alpha(10, 10), 255);
        assert!(hot_road_alpha(1, 10) < hot_road_alpha(5, 10));
        assert!(hot_road_alpha(1, 1000) >= 90, "one removal stays visible");
        assert_eq!(hot_road_alpha(0, 0), 90, "no division by zero");
    }
}
