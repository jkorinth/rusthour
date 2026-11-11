//! What a frontend shows when the user selects a vehicle or road (ADR-0005,
//! addendum M3.11). Not part of the wire protocol until M8, so growth is
//! safe: the enums are `#[non_exhaustive]`; the plain data structs are
//! constructible, so frontends can build them in tests.

use rusthour_core::Engine;
use rusthour_core::network::movement::Turn;
use serde::{Deserialize, Serialize};

use crate::{Network, RoadId, VehicleId};

/// What became of a vehicle that is no longer on the network.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Gone {
    /// Reached its destination.
    Arrived {
        /// Its last road.
        road: RoadId,
        /// Offset on it (m).
        offset_m: f32,
    },
    /// Removed after standing still too long (gridlock safety net).
    RemovedStuck {
        /// Its last road.
        road: RoadId,
        /// Offset on it (m).
        offset_m: f32,
    },
    /// Older than the last 256 exits, or never existed.
    Unknown,
}

/// The IDM driver of a vehicle; MOBIL joins with M3.6, profiles with M4.1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DriverInfo {
    /// Desired speed as a factor of the road's speed limit.
    pub desired_speed_factor: f32,
    /// Desired time headway (s).
    pub time_headway_s: f32,
    /// Maximum acceleration (m/s²).
    pub max_accel_mps2: f32,
    /// Comfortable deceleration (m/s²).
    pub comfort_decel_mps2: f32,
    /// Minimum gap (m).
    pub min_gap_m: f32,
}

/// Why a stopped lane head waits. More with M3.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum StopCause {
    /// The junction's controller made it wait.
    Junction,
    /// Must give way.
    Yield,
    /// Stop sign.
    Stop,
    /// Red signal.
    Red,
    /// In the wrong lane for its next turn.
    WrongLane,
}

/// What a vehicle waits for. It can be one step (0.1 s) old.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WaitReason {
    /// Nothing ahead matters.
    FreeRoad,
    /// Braking for a leader.
    Following {
        /// The vehicle ahead.
        leader: VehicleId,
        /// Bumper-to-bumper gap (m).
        gap_m: f32,
    },
    /// Held at the road end.
    StopLine {
        /// Why.
        cause: StopCause,
    },
    /// The next road has no room for the whole car yet.
    NoRoom {
        /// The road it wants to enter.
        next_road: RoadId,
    },
}

/// A live vehicle, as seen by the inspector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VehicleInfo {
    /// The vehicle.
    pub id: VehicleId,
    /// Its road.
    pub road: RoadId,
    /// Its lane, 0 = rightmost.
    pub lane: u8,
    /// Front bumper distance from the road start (m).
    pub offset_m: f32,
    /// Speed (m/s).
    pub speed_mps: f32,
    /// Acceleration of the last step (m/s²).
    pub accel_mps2: f32,
    /// Time since it entered the network (s).
    pub trip_time_s: f32,
    /// Roads still to drive, the current road first.
    pub route_ahead: Vec<RoadId>,
    /// Its driver.
    pub driver: DriverInfo,
    /// What it waits for.
    pub wait: WaitReason,
    /// How long it has stood still (s).
    pub standing_s: f32,
    /// Standing time after which it is removed (s); 0 = the net is off.
    pub stuck_limit_s: f32,
}

/// The kind of turn a movement makes, in right-hand traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TurnType {
    /// Within about 30° of straight on.
    Straight,
    /// A right turn.
    Right,
    /// A left turn with no oncoming road.
    Left,
    /// A left turn across oncoming traffic.
    LeftAcross,
    /// A turn back of more than about 155°.
    UTurn,
}

impl From<Turn> for TurnType {
    fn from(t: Turn) -> Self {
        match t {
            Turn::Straight => Self::Straight,
            Turn::Right => Self::Right,
            Turn::Left => Self::Left,
            Turn::LeftAcross => Self::LeftAcross,
            Turn::UTurn => Self::UTurn,
        }
    }
}

/// One way on from a road's end, with the routing delay estimates (M3.1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MovementInfo {
    /// The road it leads to.
    pub to_road: RoadId,
    /// The turn it makes.
    pub turn: TurnType,
    /// Smoothed time heads stand at the line for it now (s).
    pub delay_s: f32,
    /// What that decays to without traffic (s).
    pub prior_s: f32,
}

/// A road, as seen by the inspector.
// shortcut: no `class` yet, the road class arrives with schema 4 (M3.2); add the field then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadInfo {
    /// The road.
    pub road: RoadId,
    /// Street name, if the map has one.
    pub name: Option<String>,
    /// Number of lanes.
    pub lanes: u8,
    /// Speed limit (m/s).
    pub speed_limit_mps: f32,
    /// Length (m).
    pub length_m: f32,
    /// Vehicles on it now, all lanes.
    pub vehicles: u32,
    /// Their mean speed (m/s); `None` on an empty road.
    pub mean_speed_mps: Option<f32>,
    /// Vehicles removed on it by the stuck net so far.
    pub removed_stuck: u32,
    /// Ways on from its end.
    pub movements: Vec<MovementInfo>,
}

#[allow(clippy::cast_possible_truncation)] // the wire format is f32 (ADR-0004 §3)
fn gone(g: rusthour_core::Gone) -> Gone {
    match g {
        rusthour_core::Gone::Arrived { road, offset_m } => Gone::Arrived {
            road: RoadId(road),
            offset_m: offset_m as f32,
        },
        rusthour_core::Gone::RemovedStuck { road, offset_m } => Gone::RemovedStuck {
            road: RoadId(road),
            offset_m: offset_m as f32,
        },
        _ => Gone::Unknown,
    }
}

#[allow(clippy::cast_possible_truncation)]
fn wait(w: rusthour_core::WaitReason) -> WaitReason {
    use rusthour_core::WaitReason as W;
    match w {
        W::FreeRoad => WaitReason::FreeRoad,
        W::Following { leader, gap_m } => WaitReason::Following {
            leader: VehicleId::from_core(leader),
            gap_m: gap_m as f32,
        },
        W::NoRoom { next_road } => WaitReason::NoRoom {
            next_road: RoadId(next_road),
        },
        W::StopLine { cause } => WaitReason::StopLine {
            cause: match cause {
                rusthour_core::StopCause::Yield => StopCause::Yield,
                rusthour_core::StopCause::Stop => StopCause::Stop,
                rusthour_core::StopCause::Red => StopCause::Red,
                _ => StopCause::Junction,
            },
        },
        // A stop cause this crate does not know yet is still a wait at the line.
        _ => WaitReason::StopLine {
            cause: StopCause::Junction,
        },
    }
}

/// Describes the live vehicle `id`, or says what became of it.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // wire f32, display values
pub(crate) fn vehicle(e: &Engine, id: VehicleId) -> Result<VehicleInfo, Gone> {
    let core_id = id.to_core();
    let vs = e.vehicles();
    let Some(w) = e
        .wait_reason(core_id.slot as usize)
        .filter(|_| vs.is_alive(core_id))
    else {
        return Err(gone(e.gone(core_id)));
    };
    let s = core_id.slot as usize;
    let dt_s = f64::from(e.config().dt_ms) / 1000.0;
    let p = &vs.params[s];
    Ok(VehicleInfo {
        id,
        road: RoadId(vs.road(s)),
        lane: vs.lane_of(s),
        offset_m: vs.pos_m[s] as f32,
        speed_mps: vs.speed_mps[s] as f32,
        accel_mps2: vs.accel_mps2[s] as f32,
        trip_time_s: (e.tick().saturating_sub(vs.depart_tick[s]) as f64 * dt_s) as f32,
        route_ahead: vs.route[s][vs.route_pos[s] as usize..]
            .iter()
            .map(|&r| RoadId(r))
            .collect(),
        driver: DriverInfo {
            desired_speed_factor: p.desired_speed_factor as f32,
            time_headway_s: p.time_headway_s as f32,
            max_accel_mps2: p.max_accel_mps2 as f32,
            comfort_decel_mps2: p.comfort_decel_mps2 as f32,
            min_gap_m: p.min_gap_m as f32,
        },
        wait: wait(w),
        standing_s: (f64::from(vs.stuck_ticks[s]) * dt_s) as f32,
        stuck_limit_s: e.config().stuck_timeout_s as f32,
    })
}

/// Describes `road` of `network`, or `None` for an unknown id.
#[allow(clippy::cast_possible_truncation)] // wire f32
pub(crate) fn road(e: &Engine, network: &Network, road: RoadId) -> Option<RoadInfo> {
    let r = network.roads.get(road.0 as usize)?;
    let vs = e.vehicles();
    let (mut n, mut sum) = (0u32, 0.0f64);
    for lane in 0..r.lanes {
        for &s in vs.lane(vs.lane_id(road.0, lane)) {
            n += 1;
            sum += vs.speed_mps[s as usize];
        }
    }
    Some(RoadInfo {
        road,
        name: r.name.clone(),
        lanes: r.lanes,
        speed_limit_mps: r.speed_limit_mps as f32,
        length_m: r
            .geometry
            .windows(2)
            .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
            .sum::<f64>() as f32,
        vehicles: n,
        mean_speed_mps: (n > 0).then(|| (sum / f64::from(n)) as f32),
        removed_stuck: e.stuck_by_road()[road.0 as usize],
        movements: (e.movement_delays(road.0))
            .map(|(to, turn, delay, prior)| MovementInfo {
                to_road: RoadId(to),
                turn: turn.into(),
                delay_s: delay as f32,
                prior_s: prior as f32,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusthour_core::WaitReason as W;

    #[test]
    fn stop_causes_reach_the_wait_reason() {
        for (core, api) in [
            (rusthour_core::StopCause::Yield, StopCause::Yield),
            (rusthour_core::StopCause::Stop, StopCause::Stop),
            (rusthour_core::StopCause::Red, StopCause::Red),
        ] {
            let w = wait(W::StopLine { cause: core });
            assert_eq!(w, WaitReason::StopLine { cause: api });
        }
    }
}
