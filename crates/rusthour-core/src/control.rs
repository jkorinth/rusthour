//! Intersection controllers: priority, stop, signals, roundabouts (M2, M3,
//! M7). Each lives in its own submodule implementing [`Controller`] and is
//! dispatched statically through [`NodeControl`], whose variant list and
//! `match` are the only lines controllers share (ADR-0004).

use crate::StateHasher;
use crate::network::Network;

pub mod priority;

/// A lane head near the end of a road into the node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Approach {
    /// Road the vehicle is on.
    pub from_road: u32,
    /// Next road on its route.
    pub to_road: u32,
    /// Vehicle slot.
    pub slot: u32,
    /// Front bumper to the road end (m).
    pub dist_m: f64,
    /// Speed (m/s).
    pub speed_mps: f64,
    /// The vehicle's minimum gap `s0` (m).
    pub min_gap_m: f64,
    /// `dist ≥ v²/(2b)`: it can still stop at the road end.
    pub can_stop: bool,
    /// No room for the whole car on `to_road`: it must not enter the node
    /// (don't block the box), and no one yields to it.
    pub blocked: bool,
}

/// Decides which approaches of one node may pass.
pub trait Controller {
    /// Sets `go[i]` for `approaches[i]`, which are sorted by `from_road`.
    fn decide(&mut self, net: &Network, tick: u64, approaches: &[Approach], go: &mut [bool]);
    /// Feeds the controller's state into `h`.
    fn hash_into(&self, h: &mut StateHasher);
}

/// The controller of one node.
#[derive(Debug, Clone)]
pub enum NodeControl {
    /// Priority rule (M2.5).
    Priority(priority::Priority),
}

impl Controller for NodeControl {
    fn decide(&mut self, net: &Network, tick: u64, approaches: &[Approach], go: &mut [bool]) {
        match self {
            Self::Priority(c) => c.decide(net, tick, approaches, go),
        }
    }

    fn hash_into(&self, h: &mut StateHasher) {
        match self {
            Self::Priority(c) => c.hash_into(h),
        }
    }
}
