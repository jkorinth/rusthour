//! Diagnostics for inspecting a vehicle (ADR-0005, addendum M3.11).
//!
//! Phases write them, nothing reads them back into the simulation, and none
//! of it is hashed.

use super::{Engine, leader_ahead};
use crate::vehicle::VehicleId;
use crate::vehicle::idm::{self, Leader};

/// Why a lane head stands at its road end. More with M3.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopCause {
    /// It gives way to another movement: a sign, a higher-ranked road, the
    /// right, or oncoming traffic.
    Yield,
    /// It must stand at a stop sign before it may go.
    Stop,
    /// Its signal shows amber or red.
    Red,
}

/// What a vehicle waits for, from the state after the last step. The stop
/// cause was decided before integration, so it can be one step old.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum WaitReason {
    /// Nothing ahead matters.
    FreeRoad,
    /// A leader brakes it: the IDM interaction term is at least 0.1.
    Following {
        /// The vehicle ahead.
        leader: VehicleId,
        /// Bumper-to-bumper gap (m).
        gap_m: f64,
    },
    /// Held at the road end by the node.
    StopLine {
        /// Why.
        cause: StopCause,
    },
    /// The next road has no room for the whole car yet.
    NoRoom {
        /// The road it wants to enter.
        next_road: u32,
    },
}

/// What became of a vehicle that is no longer on the network.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Gone {
    /// Reached its destination.
    Arrived {
        /// Its last road.
        road: u32,
        /// Offset on it (m).
        offset_m: f64,
    },
    /// Removed by the stuck-vehicle safety net.
    RemovedStuck {
        /// Its last road.
        road: u32,
        /// Offset on it (m).
        offset_m: f64,
    },
    /// Left longer ago than the exit ring remembers, or never existed.
    Unknown,
}

/// Per lane, the diagnostic twin of `Engine::stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LaneStop {
    Go,
    NoRoom,
    Yield,
    Stop,
    Red,
}

/// Exits remembered.
const EXITS: usize = 256;

/// The last `EXITS` exits; written at despawn, allocated once.
#[derive(Debug, Clone)]
pub(super) struct Exits {
    buf: Vec<(VehicleId, Gone)>,
    /// Oldest entry once full.
    next: usize,
}

impl Exits {
    pub(super) fn new() -> Self {
        Self {
            buf: Vec::with_capacity(EXITS),
            next: 0,
        }
    }

    pub(super) fn push(&mut self, id: VehicleId, gone: Gone) {
        if self.buf.len() < EXITS {
            self.buf.push((id, gone));
        } else {
            self.buf[self.next] = (id, gone);
            self.next = (self.next + 1) % EXITS;
        }
    }
}

impl Engine {
    /// Why the vehicle in `slot` waits; `None` for a free or unknown slot.
    /// Meaningful after the first step: before it, the answer is `Following`
    /// or `FreeRoad`.
    #[must_use]
    pub fn wait_reason(&self, slot: usize) -> Option<WaitReason> {
        let (vs, net) = (&self.vehicles, &self.network);
        if slot >= vs.slot_count() || !vs.is_live(slot) {
            return None;
        }
        let road = vs.road(slot);
        let q = vs.lane_id(road, vs.lane_of(slot));
        let lane = vs.lane(q);
        let i = lane.iter().position(|&s| s as usize == slot)?;
        let to_road = vs.route[slot].get(vs.route_pos[slot] as usize + 1).copied();
        if i == 0 {
            match (self.stop_cause[q], to_road) {
                (LaneStop::NoRoom, Some(next_road)) => {
                    return Some(WaitReason::NoRoom { next_road });
                }
                (LaneStop::Yield, _) => {
                    return Some(WaitReason::StopLine {
                        cause: StopCause::Yield,
                    });
                }
                (LaneStop::Stop, _) => {
                    return Some(WaitReason::StopLine {
                        cause: StopCause::Stop,
                    });
                }
                (LaneStop::Red, _) => {
                    return Some(WaitReason::StopLine {
                        cause: StopCause::Red,
                    });
                }
                _ => {}
            }
        }
        let found = if i > 0 {
            let l = lane[i - 1] as usize;
            let gap_m = (vs.pos_m[l] - vs.params[l].length_m - vs.pos_m[slot]).max(0.0);
            let leader = Leader {
                gap_m,
                speed_mps: vs.speed_mps[l],
            };
            Some((leader, lane[i - 1]))
        } else {
            let dist_m = net.roads[road as usize].length_m - vs.pos_m[slot];
            leader_ahead(net, &self.lanes, vs, slot, vs.lane_of(slot), dist_m)
        };
        let Some((leader, ls)) = found else {
            return Some(WaitReason::FreeRoad);
        };
        let ratio = idm::interaction(&vs.params[slot], vs.speed_mps[slot], leader);
        Some(if ratio * ratio >= 0.1 {
            WaitReason::Following {
                leader: vs.id(ls as usize),
                gap_m: leader.gap_m,
            }
        } else {
            WaitReason::FreeRoad
        })
    }

    /// What became of `id`, which no longer names a live vehicle.
    #[must_use]
    pub fn gone(&self, id: VehicleId) -> Gone {
        let e = &self.exits;
        (e.buf.iter())
            .find(|(i, _)| *i == id)
            .map_or(Gone::Unknown, |&(_, g)| g)
    }

    /// The movements out of `road` as `(to, turn, delay_s, prior_s)`.
    pub fn movement_delays(
        &self,
        road: u32,
    ) -> impl Iterator<Item = (u32, crate::network::movement::Turn, f64, f64)> + '_ {
        self.router.movement_delays(road)
    }
}

#[cfg(test)]
mod tests {
    use crate::network::{EndControl, Network, Road};
    use crate::vehicle::idm::IdmParams;
    use crate::{Config, Engine, Gone, StopCause, WaitReason};

    fn car() -> IdmParams {
        let (a, b) = (1.5, 2.0);
        IdmParams {
            desired_speed_factor: 1.0,
            time_headway_s: 1.5,
            max_accel_mps2: a,
            comfort_decel_mps2: b,
            min_gap_m: 2.0,
            length_m: 4.5,
            sqrt_ab: f64::sqrt(a * b),
        }
    }

    fn road(from: u32, to: u32, length_m: f64, d: [f64; 2]) -> Road {
        Road {
            from,
            to,
            length_m,
            speed_limit_mps: 10.0,
            lanes: 1,
            start_dir: d,
            end_dir: d,
            roundabout: false,
            class: 0,
            priority_road: false,
            end_control: EndControl::None,
            turn_lanes: vec![],
        }
    }

    /// Two opposite roads of `length_m` between nodes 0 and 1.
    fn pair(length_m: f64) -> Network {
        Network {
            nodes: vec![[0.0, 0.0], [length_m, 0.0]],
            roads: vec![
                road(0, 1, length_m, [1.0, 0.0]),
                road(1, 0, length_m, [-1.0, 0.0]),
            ],
            connections: vec![(0, 1), (1, 0)],
        }
    }

    fn wait(e: &Engine, id: crate::vehicle::VehicleId) -> WaitReason {
        e.wait_reason(id.slot as usize).unwrap()
    }

    #[test]
    fn wait_reasons_follower_no_room_yield_and_stop() {
        // A follower enters behind a leader that is still slow.
        let mut e = Engine::with_network(Config::default(), 1, pair(400.0));
        let lead = e.spawn(&[0], car()).unwrap();
        e.step(30);
        let follower = e.spawn(&[0], car()).expect("room behind the leader");
        e.step(1);
        let WaitReason::Following { leader, gap_m } = wait(&e, follower) else {
            panic!("{:?}", wait(&e, follower));
        };
        assert_eq!(leader, lead);
        assert!(gap_m > 0.0);
        assert!(matches!(wait(&e, lead), WaitReason::FreeRoad));

        // A head at the end of road 0 whose next road starts with a car.
        let mut e = Engine::with_network(Config::default(), 1, pair(100.0));
        let head = e.spawn(&[0, 1], car()).unwrap();
        e.spawn(&[1, 0], car()).unwrap();
        e.vehicles.pos_m[head.slot as usize] = 95.0;
        e.step(1);
        assert_eq!(wait(&e, head), WaitReason::NoRoom { next_road: 1 });

        // Four arms meeting: somebody has to wait for the junction.
        let arms = [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]];
        let mut nodes = vec![[0.0, 0.0]];
        nodes.extend(arms.iter().map(|d| [100.0 * d[0], 100.0 * d[1]]));
        let mut roads: Vec<Road> = (0..4)
            .map(|k| {
                road(
                    k + 1,
                    0,
                    100.0,
                    [-arms[k as usize][0], -arms[k as usize][1]],
                )
            })
            .collect();
        roads.extend((0..4).map(|k| road(0, k + 1, 100.0, arms[k as usize])));
        let net = Network {
            nodes,
            roads,
            connections: (0..4u32)
                .flat_map(|i| (0..4).filter(move |&j| j != i).map(move |j| (i, 4 + j)))
                .collect(),
        };
        let mut e = Engine::with_network(Config::default(), 1, net.clone());
        let heads: Vec<_> = (0..4u32)
            .map(|k| {
                let id = e.spawn(&[k, 4 + (k + 2) % 4], car()).unwrap();
                e.vehicles.pos_m[id.slot as usize] = 95.0;
                id
            })
            .collect();
        e.step(1);
        let waits: Vec<_> = heads.iter().map(|&h| wait(&e, h)).collect();
        let yields = WaitReason::StopLine {
            cause: StopCause::Yield,
        };
        assert!(waits.contains(&yields), "{waits:?}");

        // Alone at a stop sign: it waits for the sign, not for traffic.
        let mut net = net;
        net.roads[0].end_control = EndControl::Stop;
        let mut e = Engine::with_network(Config::default(), 1, net);
        let head = e.spawn(&[0, 6], car()).unwrap();
        e.vehicles.pos_m[head.slot as usize] = 80.0;
        e.step(1);
        let stop = WaitReason::StopLine {
            cause: StopCause::Stop,
        };
        assert_eq!(wait(&e, head), stop);
    }

    #[test]
    fn arrivals_removals_and_old_exits_are_remembered() {
        // Arrived: the last road and the road's end.
        let mut e = Engine::with_network(Config::default(), 1, pair(50.0));
        let first = e.spawn(&[0], car()).unwrap();
        e.step(1);
        assert!(e.wait_reason(first.slot as usize).is_some());
        e.step(400);
        assert!(!e.vehicles().is_alive(first));
        assert_eq!(
            e.gone(first),
            Gone::Arrived {
                road: 0,
                offset_m: 50.0
            }
        );
        // More than 256 exits later it is forgotten, the newest is not.
        let mut last = first;
        for _ in 0..12_000 {
            if let Some(id) = e.spawn(&[0], car()) {
                last = id;
            }
            e.step(1);
        }
        assert!(e.arrived() > 256, "{}", e.arrived());
        assert_eq!(e.gone(first), Gone::Unknown);
        assert!(e.vehicles().is_alive(last) || matches!(e.gone(last), Gone::Arrived { .. }));
    }

    #[test]
    fn a_vehicle_removed_by_the_stuck_net_says_so() {
        // Four short roads in a ring, filled until nobody can move.
        let ring = Network {
            nodes: vec![[0.0, 0.0], [15.0, 0.0], [30.0, 0.0], [45.0, 0.0]],
            roads: (0..4)
                .map(|r| road(r, (r + 1) % 4, 15.0, [1.0, 0.0]))
                .collect(),
            connections: (0..4).map(|r| (r, (r + 1) % 4)).collect(),
        };
        let config = Config {
            stuck_timeout_s: 20,
            ..Config::default()
        };
        let mut e = Engine::with_network(config, 1, ring);
        let mut ids = Vec::new();
        for _ in 0..600 {
            for start in 0..4u32 {
                let route: Vec<u32> = (0..100).map(|i| (start + i) % 4).collect();
                ids.extend(e.spawn(&route, car()));
            }
            e.step(1);
        }
        e.step(300);
        assert!(e.removed_stuck() > 0);
        let removed = ids
            .iter()
            .find(|&&id| matches!(e.gone(id), Gone::RemovedStuck { .. }));
        assert!(removed.is_some(), "no removal recorded");
        assert_eq!(e.arrived(), 0);
    }
}
