//! Per-profile tuning of the driver parameters (ADR-0006, M4.4 addendum):
//! the fields a frontend can edit and the ranges the api accepts.

use std::fmt;
use std::ops::RangeInclusive;

use crate::DriverProfile;

pub use rusthour_core::behavior::profile::Tuning as DriverTuning;

/// One tunable parameter of [`DriverTuning`].
pub struct TuningField {
    /// Its name in the viewer and in errors.
    pub name: &'static str,
    /// Its unit, empty for none.
    pub unit: &'static str,
    /// The values the api accepts; the model misbehaves far outside them.
    pub range: RangeInclusive<f64>,
    /// Reads it.
    pub get: fn(&DriverTuning) -> f64,
    /// Writes it.
    pub set: fn(&mut DriverTuning, f64),
}

/// Every parameter of [`DriverTuning`], in the viewer's order.
pub const TUNING_FIELDS: [TuningField; 6] = [
    TuningField {
        name: "desired speed",
        unit: "× limit",
        range: 0.5..=1.5,
        get: |t| t.desired_speed_factor,
        set: |t, v| t.desired_speed_factor = v,
    },
    TuningField {
        name: "headway",
        unit: "s",
        range: 0.5..=3.0,
        get: |t| t.time_headway_s,
        set: |t, v| t.time_headway_s = v,
    },
    TuningField {
        name: "acceleration",
        unit: "m/s²",
        range: 0.3..=4.0,
        get: |t| t.max_accel_mps2,
        set: |t, v| t.max_accel_mps2 = v,
    },
    TuningField {
        name: "braking",
        unit: "m/s²",
        range: 0.5..=5.0,
        get: |t| t.comfort_decel_mps2,
        set: |t, v| t.comfort_decel_mps2 = v,
    },
    TuningField {
        name: "min gap",
        unit: "m",
        range: 0.5..=6.0,
        get: |t| t.min_gap_m,
        set: |t, v| t.min_gap_m = v,
    },
    TuningField {
        name: "politeness",
        unit: "",
        range: 0.0..=1.0,
        get: |t| t.politeness,
        set: |t, v| t.politeness = v,
    },
];

/// The first parameter of `t` outside its range, and its value.
pub(crate) fn out_of_range(t: &DriverTuning) -> Option<(&'static str, f64)> {
    TUNING_FIELDS
        .iter()
        .map(|f| (f.name, (f.get)(t), &f.range))
        .find(|(_, v, range)| !range.contains(v))
        .map(|(name, v, _)| (name, v))
}

/// The message of [`NetworkError::BadTuning`](crate::NetworkError::BadTuning).
pub(crate) fn explain(
    f: &mut fmt::Formatter<'_>,
    profile: DriverProfile,
    parameter: &str,
    value: f64,
) -> fmt::Result {
    let profile = format!("{profile:?}").to_lowercase();
    write!(f, "the {profile} drivers' {parameter} must be ")?;
    match TUNING_FIELDS.iter().find(|t| t.name == parameter) {
        Some(t) if t.unit.is_empty() => write!(f, "{} to {}", t.range.start(), t.range.end())?,
        Some(t) => write!(f, "{} to {} {}", t.range.start(), t.range.end(), t.unit)?,
        None => write!(f, "in range")?,
    }
    write!(f, ", got {value}")
}
