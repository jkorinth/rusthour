//! Signal states for frontends (ADR-0005 §4, M3.4).

use serde::{Deserialize, Serialize};

use crate::RoadId;

/// What a signal shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalState {
    /// Go.
    Green,
    /// Stop unless too close to stop.
    Amber,
    /// Stop.
    Red,
}

/// Every signal arm at one tick, as parallel columns: entry `i` is the
/// signal at the end of `road[i]`. Reused between frames to avoid
/// allocation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SignalSnapshot {
    /// Step count the snapshot was taken at.
    pub tick: u64,
    /// The road whose end the signal stands at.
    pub road: Vec<RoadId>,
    /// What it shows.
    pub state: Vec<SignalState>,
    /// Simulated seconds until it changes.
    pub next_change_s: Vec<f32>,
}

impl SignalSnapshot {
    /// Empties every column but keeps their capacity.
    pub fn clear(&mut self) {
        self.road.clear();
        self.state.clear();
        self.next_change_s.clear();
    }

    /// Number of signal arms.
    #[must_use]
    pub fn len(&self) -> usize {
        self.road.len()
    }

    /// Whether there are no signals.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.road.is_empty()
    }
}
