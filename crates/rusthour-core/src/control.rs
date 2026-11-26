//! Intersection controllers: priority, stop, signals, roundabouts (M2, M3,
//! M5). Each lives in its own submodule implementing [`Controller`] and is
//! dispatched statically through [`NodeControl`], whose variant list and
//! `each!` macro are the only lines controllers share (ADR-0004).

use crate::StateHasher;
use crate::network::Network;
use crate::vehicle::Vehicles;

pub mod priority;
pub mod signal;

/// A lane head near the end of a road into the node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Approach {
    /// Road the vehicle is on.
    pub from_road: u32,
    /// Its lane on that road.
    pub lane: u8,
    /// Next road on its route.
    pub to_road: u32,
    /// The lane of `to_road` it enters (ADR-0005 §5).
    pub to_lane: u8,
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
    /// Sets `go[i]` for `approaches[i]`, which are sorted by
    /// `(from_road, lane)`; `vs` is the vehicle state at the start of the
    /// step, for controllers that look beyond the lane heads.
    fn decide(
        &mut self,
        net: &Network,
        vs: &Vehicles,
        tick: u64,
        approaches: &[Approach],
        go: &mut [bool],
    );
    /// Feeds the controller's state into `h`.
    fn hash_into(&self, h: &mut StateHasher);
    /// Whether `a` waits only to stand at a stop sign first; valid after
    /// `decide`. The engine uses it to label why a head waits.
    fn holds_at_stop_sign(&self, _a: &Approach) -> bool {
        false
    }
    /// Whether `a` faces an amber or red signal at `tick`. Besides labelling
    /// why a head waits, this changes behaviour: a wrong-lane head at red
    /// waits for green without counting towards its fallback re-plan.
    fn holds_at_red(&self, _a: &Approach, _tick: u64) -> bool {
        false
    }
}

/// The controller of one node.
#[derive(Debug, Clone)]
pub enum NodeControl {
    /// Priority rule: signs, priority roads, class, right before left
    /// (M2.5, M3.3).
    Priority(priority::Priority),
    /// Fixed-time signals (M3.4).
    Signal(signal::Signal),
}

/// Runs `$body` with `$c` bound to the controller inside `$node`: the one
/// list of variants to extend besides the enum.
macro_rules! each {
    ($node:expr, $c:ident => $body:expr) => {
        match $node {
            NodeControl::Priority($c) => $body,
            NodeControl::Signal($c) => $body,
        }
    };
}

impl Controller for NodeControl {
    fn decide(
        &mut self,
        net: &Network,
        vs: &Vehicles,
        tick: u64,
        approaches: &[Approach],
        go: &mut [bool],
    ) {
        each!(self, c => c.decide(net, vs, tick, approaches, go));
    }

    fn hash_into(&self, h: &mut StateHasher) {
        each!(self, c => c.hash_into(h));
    }

    fn holds_at_stop_sign(&self, a: &Approach) -> bool {
        each!(self, c => c.holds_at_stop_sign(a))
    }

    fn holds_at_red(&self, a: &Approach, tick: u64) -> bool {
        each!(self, c => c.holds_at_red(a, tick))
    }
}
