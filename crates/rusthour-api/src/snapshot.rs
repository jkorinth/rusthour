//! Per-frame vehicle state for frontends (ADR-0004 §3).

use serde::{Deserialize, Serialize};

use crate::RoadId;

/// Identifies a vehicle for its whole life: `generation << 32 | slot`.
///
/// Exceeds 2^53 once a slot's generation passes about 2 million, so a JS
/// consumer parsing the JSON as a number would lose bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VehicleId(pub u64);

impl VehicleId {
    pub(crate) fn from_core(id: rusthour_core::vehicle::VehicleId) -> Self {
        Self(u64::from(id.generation) << 32 | u64::from(id.slot))
    }

    #[allow(clippy::cast_possible_truncation)] // the two halves
    pub(crate) fn to_core(self) -> rusthour_core::vehicle::VehicleId {
        rusthour_core::vehicle::VehicleId {
            slot: self.0 as u32,
            generation: (self.0 >> 32) as u32,
        }
    }
}

/// All vehicles at one tick, as parallel columns in lane order. Entry `i` is
/// `id[i]`, `road[i]`, and so on. Reused between frames to avoid allocation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VehicleSnapshot {
    /// Step count the snapshot was taken at.
    pub tick: u64,
    /// Vehicle ids.
    pub id: Vec<VehicleId>,
    /// Road each vehicle is on.
    pub road: Vec<RoadId>,
    /// Lane, 0 = rightmost in the direction of travel.
    pub lane: Vec<u8>,
    /// How far the vehicle is still drawn towards the lane it just left, in
    /// lanes (positive: left); fades to 0 over 2 s of simulated time after a
    /// lane change. Draw it at `lane + lane_shift` (ADR-0005 §6).
    #[serde(default)]
    pub lane_shift: Vec<f32>,
    /// Front bumper distance from the road start, in meters.
    pub offset_m: Vec<f32>,
    /// Speed in meters per second.
    pub speed_mps: Vec<f32>,
    /// Vehicle length in meters.
    pub length_m: Vec<f32>,
}

impl VehicleSnapshot {
    /// Empties every column but keeps their capacity.
    pub fn clear(&mut self) {
        self.id.clear();
        self.road.clear();
        self.lane.clear();
        self.lane_shift.clear();
        self.offset_m.clear();
        self.speed_mps.clear();
        self.length_m.clear();
    }

    /// Number of vehicles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.id.len()
    }

    /// Whether there are no vehicles.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_keeps_capacity_and_round_trips() {
        let mut s = VehicleSnapshot {
            tick: 3,
            ..Default::default()
        };
        s.id.push(VehicleId(1 << 32 | 7));
        s.road.push(RoadId(2));
        s.lane.push(0);
        s.lane_shift.push(0.5);
        s.offset_m.push(1.5);
        s.speed_mps.push(2.5);
        s.length_m.push(4.5);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<VehicleSnapshot>(&json).unwrap(), s);
        let cap = s.offset_m.capacity();
        s.clear();
        assert!(s.is_empty() && s.offset_m.capacity() == cap);
    }
}
