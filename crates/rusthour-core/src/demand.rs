//! Travel demand: agents, activity chains, time-of-day and weekday profiles (M5).
//!
//! M2 has one model, [`RandomOd`]: a closed system that keeps a target
//! population of vehicles on random origin–destination pairs (ADR-0004).

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::Rng;

/// What the engine tells the demand model each step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DemandContext {
    /// Current step.
    pub tick: u64,
    /// Step length in milliseconds.
    pub dt_ms: u32,
    /// Vehicles on the network.
    pub active: u32,
    /// Trips generated but not yet entered.
    pub waiting: u32,
}

/// A wish to travel from one road to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trip {
    /// Road the vehicle starts on.
    pub origin: u32,
    /// Road the vehicle ends on.
    pub destination: u32,
}

/// Random origin–destination demand holding a target population.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomOd {
    /// Active plus waiting vehicles to maintain.
    pub target_vehicles: u32,
}

impl RandomOd {
    /// Appends enough trips to `out` to bring `active + waiting` up to the
    /// target. Origins and distinct destinations are drawn uniformly from
    /// `routable` (the router's strongly connected roads), so every trip has
    /// a route. `rng` is the engine's `Stream::Demand`.
    pub fn spawn(
        &mut self,
        ctx: &DemandContext,
        routable: &[u32],
        rng: &mut ChaCha8Rng,
        out: &mut Vec<Trip>,
    ) {
        let Ok(n) = u32::try_from(routable.len()) else {
            return;
        };
        if n < 2 {
            return;
        }
        let missing = self
            .target_vehicles
            .saturating_sub(ctx.active.saturating_add(ctx.waiting));
        for _ in 0..missing {
            let o = below(rng, n);
            let mut d = below(rng, n - 1);
            if d >= o {
                d += 1; // skip the origin: destination != origin
            }
            out.push(Trip {
                origin: routable[o as usize],
                destination: routable[d as usize],
            });
        }
    }
}

/// Uniform in `0..n` (`n > 0`) by multiply-shift; the bias is below 2^-32 · n.
fn below(rng: &mut ChaCha8Rng, n: u32) -> u32 {
    ((u64::from(rng.next_u32()) * u64::from(n)) >> 32) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{Network, Road};
    use crate::routing::Router;
    use rand_chacha::rand_core::SeedableRng;

    /// Cycle 0-1-2 (SCC) with a one-way spur 3 and a disconnected loop 4-5.
    fn net() -> Network {
        let road = |from, to| Road {
            from,
            to,
            length_m: 10.0,
            speed_limit_mps: 10.0,
            lanes: 1,
            start_dir: [1.0, 0.0],
            end_dir: [1.0, 0.0],
            roundabout: false,
        };
        Network {
            nodes: vec![[0.0; 2]; 6],
            roads: (0..6).map(|i| road(i, i)).collect(),
            connections: vec![(0, 1), (1, 2), (2, 0), (2, 3), (4, 5), (5, 4)],
        }
    }

    fn trips(seed: u64, active: u32, waiting: u32) -> Vec<Trip> {
        let net = net();
        let router = Router::new(&net);
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        rng.set_stream(1);
        let ctx = DemandContext {
            tick: 0,
            dt_ms: 100,
            active,
            waiting,
        };
        let mut out = Vec::new();
        RandomOd {
            target_vehicles: 50,
        }
        .spawn(&ctx, router.routable(), &mut rng, &mut out);
        out
    }

    #[test]
    fn tops_up_to_target_inside_the_scc_and_is_deterministic() {
        let t = trips(7, 10, 5);
        assert_eq!(t.len(), 35);
        assert!(
            t.iter()
                .all(|t| t.origin < 3 && t.destination < 3 && t.origin != t.destination)
        );
        assert_eq!(t, trips(7, 10, 5));
        assert_ne!(t, trips(8, 10, 5));
        assert!(trips(7, 50, 0).is_empty());
    }

    #[test]
    fn every_trip_has_a_route() {
        let (net, trips) = (net(), trips(1, 0, 0));
        let mut router = Router::new(&net);
        let mut route = Vec::new();
        assert!(trips.iter().all(|&t| router.route(&net, t, &mut route)));
    }
}
