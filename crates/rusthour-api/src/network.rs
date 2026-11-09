//! Road network interchange data (ADR-0002); see [`Network`].

use std::collections::HashSet;
use std::fmt;

use rusthour_core::network as core;
use serde::{Deserialize, Serialize};

/// Version of the network schema (`Network` and the types it contains).
///
/// Written into the `.rhnet` asset header (ADR-0002). Bump on any
/// incompatible change to these types.
///
/// Version 2 added OSM source references ([`Node::osm_node`], [`Road::osm`])
/// and [`Road::name`]. Version 3 added [`Road::roundabout`]. Version 4 added
/// [`Road::class`], [`Road::priority_road`], [`Road::end_control`] and
/// [`Road::turn_lanes`] (ADR-0005).
pub const NETWORK_SCHEMA_VERSION: u32 = 4;

/// Most lanes a road may have: lane sets are `u16` masks (ADR-0005).
pub const MAX_LANES: u8 = 16;

/// Index of a node in [`Network::nodes`]. Serializes as a plain number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u32);

/// Index of a road in [`Network::roads`]. Serializes as a plain number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoadId(pub u32);

/// A position in the network's local planar frame, in meters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// Meters east of the origin.
    pub x: f64,
    /// Meters north of the origin.
    pub y: f64,
}

/// An intersection, or any other point where roads meet or end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// The intersection's reference point (e.g. its center). Roads attached to
    /// the node need not start or end exactly here; see [`Road::geometry`].
    pub position: Point,
    /// The OpenStreetMap node this node was imported from, if any. `None` for
    /// hand-built networks.
    pub osm_node: Option<i64>,
}

/// A directed road between two nodes; all its lanes run from `from_node` to
/// `to_node`. A two-way street is two roads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Road {
    /// The node the road starts at.
    pub from_node: NodeId,
    /// The node the road ends at.
    pub to_node: NodeId,
    /// Centerline polyline in travel direction: at least two points and no
    /// zero-length segments. It may start and end at the intersection's
    /// boundary rather than at the node position, so renderers should expect
    /// a gap there. Its length is the road's length.
    pub geometry: Vec<Point>,
    /// Speed limit in meters per second.
    pub speed_limit_mps: f64,
    /// Number of lanes, at least one.
    pub lanes: u8,
    /// Street name for display only (OSM `name`, else `ref`); never used to
    /// identify the road.
    pub name: Option<String>,
    /// The part of an OpenStreetMap way this road was imported from, if any.
    /// `None` for hand-built networks.
    pub osm: Option<OsmWayPart>,
    /// Part of a roundabout (OSM `junction=roundabout|circular`): traffic on
    /// it has priority over roads entering it.
    pub roundabout: bool,
    /// Road class (OSM `highway`), a proxy for right of way where signs are
    /// missing from the map.
    #[serde(default)]
    pub class: RoadClass,
    /// Part of a priority road (OSM `priority_road=designated|yes_unposted`).
    #[serde(default)]
    pub priority_road: bool,
    /// The sign or signal at the road's end, for traffic on this road.
    #[serde(default)]
    pub end_control: EndControl,
    /// Allowed turns per lane, rightmost lane (lane 0) first: empty if
    /// unknown, else one entry per lane.
    #[serde(default)]
    pub turn_lanes: Vec<Turns>,
}

/// A road's class, from OSM `highway`; a `_link` road takes its parent's
/// class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoadClass {
    /// Not known, e.g. a hand-built road.
    #[default]
    Unknown,
    /// `motorway`.
    Motorway,
    /// `trunk`.
    Trunk,
    /// `primary`.
    Primary,
    /// `secondary`.
    Secondary,
    /// `tertiary`.
    Tertiary,
    /// `unclassified`: minor public roads.
    Unclassified,
    /// `residential`.
    Residential,
    /// `living_street`.
    LivingStreet,
}

/// What traffic on a road meets at the road's end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndControl {
    /// No sign or signal.
    #[default]
    None,
    /// A give-way sign.
    GiveWay,
    /// A stop sign.
    Stop,
    /// Traffic signals.
    Signals,
}

/// The turns a lane allows: a bit set of [`Turns::LEFT`], [`Turns::THROUGH`],
/// [`Turns::RIGHT`] and [`Turns::U_TURN`]. Serializes as a plain number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Turns(pub u8);

impl Turns {
    /// Left turns.
    pub const LEFT: Self = Self(1);
    /// Straight on.
    pub const THROUGH: Self = Self(2);
    /// Right turns.
    pub const RIGHT: Self = Self(4);
    /// U-turns.
    pub const U_TURN: Self = Self(8);
    /// Every known bit.
    pub const ALL: Self = Self(15);
}

impl std::ops::BitOr for Turns {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Where a [`Road`] comes from in OpenStreetMap: the stretch of way `way`
/// between its nodes `from_node` and `to_node`, in travel direction.
///
/// An importer splits ways at intersections and may merge or move nodes, so
/// the way id alone is ambiguous and the road's [`Node`]s need not carry these
/// OSM node ids. Together the three ids identify the road again after a
/// re-import (M7.0), unless the way itself changed there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsmWayPart {
    /// OSM way id.
    pub way: i64,
    /// OSM id of the way node the road starts at.
    pub from_node: i64,
    /// OSM id of the way node the road ends at.
    pub to_node: i64,
}

/// An allowed movement at a node: traffic may continue from `from_road` onto
/// `to_road`, where `from_road` ends at the node `to_road` starts at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    /// The incoming road.
    pub from_road: RoadId,
    /// The outgoing road.
    pub to_road: RoadId,
}

/// A complete road network: plain, serde-serializable interchange data.
///
/// The importer produces it, the `.rhnet` asset stores it (ADR-0002), and a
/// simulation is created from it with
/// [`InProcess::with_network`](crate::InProcess::with_network), which
/// validates it. [`Simulation::network`](crate::Simulation::network) returns
/// it for rendering.
///
/// Items live in flat lists and are addressed by their index, via
/// [`NodeId`] and [`RoadId`].
///
/// Coordinates are planar, in meters, in a local frame: `x` points east, `y`
/// points north. The importer chooses the origin (e.g. the area's center) and
/// projects WGS84 into this frame, so geometry is ready for rendering and
/// simulation without further conversion.
///
/// # Example
///
/// Two intersections joined by a straight two-lane road:
///
/// ```
/// use rusthour_api::{
///     Config, EndControl, InProcess, Network, Node, NodeId, Point, Road, RoadClass, Simulation,
/// };
///
/// let a = Point { x: 0.0, y: 0.0 };
/// let b = Point { x: 100.0, y: 0.0 };
/// let network = Network {
///     nodes: vec![
///         Node { position: a, osm_node: None },
///         Node { position: b, osm_node: None },
///     ],
///     roads: vec![Road {
///         from_node: NodeId(0),
///         to_node: NodeId(1),
///         geometry: vec![a, b],
///         speed_limit_mps: 13.9,
///         lanes: 2,
///         name: Some("Hauptstraße".into()),
///         osm: None,
///         roundabout: false,
///         class: RoadClass::Secondary,
///         priority_road: true,
///         end_control: EndControl::None,
///         turn_lanes: vec![],
///     }],
///     connections: vec![],
/// };
/// let sim = InProcess::with_network(Config::default(), 42, network)?;
/// assert_eq!(sim.network().roads[0].lanes, 2);
/// # Ok::<(), rusthour_api::NetworkError>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Network {
    /// Nodes, addressed by [`NodeId`].
    pub nodes: Vec<Node>,
    /// Roads, addressed by [`RoadId`].
    pub roads: Vec<Road>,
    /// Allowed movements between roads.
    pub connections: Vec<Connection>,
}

/// Why a [`Network`] was rejected. Indices refer to the network's lists.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum NetworkError {
    /// A node position is NaN or infinite.
    NonFiniteNode {
        /// Offending node.
        node: NodeId,
    },
    /// A road starts or ends at a node that does not exist.
    UnknownNode {
        /// Offending road.
        road: RoadId,
        /// The missing node.
        node: NodeId,
    },
    /// A road's geometry has fewer than two points.
    TooFewPoints {
        /// Offending road.
        road: RoadId,
        /// Number of points it has.
        points: usize,
    },
    /// A road's geometry has a NaN or infinite coordinate.
    NonFiniteGeometry {
        /// Offending road.
        road: RoadId,
        /// Index of the offending point.
        point: usize,
    },
    /// Two consecutive points of a road's geometry coincide.
    ZeroLengthSegment {
        /// Offending road.
        road: RoadId,
        /// Index of the segment's first point.
        point: usize,
    },
    /// A road's length overflows, i.e. its coordinates are absurdly large.
    LengthOverflow {
        /// Offending road.
        road: RoadId,
    },
    /// A road's speed limit is not a positive, finite number.
    BadSpeedLimit {
        /// Offending road.
        road: RoadId,
        /// The rejected value, in meters per second.
        speed_limit_mps: f64,
    },
    /// A road has no lanes.
    NoLanes {
        /// Offending road.
        road: RoadId,
    },
    /// A road has more than [`MAX_LANES`] lanes.
    TooManyLanes {
        /// Offending road.
        road: RoadId,
        /// Its lane count.
        lanes: u8,
    },
    /// A road's `turn_lanes` is neither empty nor one entry per lane.
    TurnLanesMismatch {
        /// Offending road.
        road: RoadId,
        /// Its lane count.
        lanes: u8,
        /// Its number of `turn_lanes` entries.
        turn_lanes: usize,
    },
    /// A lane's [`Turns`] is empty or has unknown bits.
    BadTurns {
        /// Offending road.
        road: RoadId,
        /// The lane, 0 being the rightmost.
        lane: usize,
        /// The rejected bits.
        turns: u8,
    },
    /// A connection refers to a road that does not exist.
    UnknownRoad {
        /// The connection's incoming road.
        from_road: RoadId,
        /// The connection's outgoing road.
        to_road: RoadId,
        /// Whichever of the two does not exist.
        missing: RoadId,
    },
    /// A connection's incoming road does not end where its outgoing road starts.
    Disconnected {
        /// The incoming road.
        from_road: RoadId,
        /// Where the incoming road ends.
        ends_at: NodeId,
        /// The outgoing road.
        to_road: RoadId,
        /// Where the outgoing road starts.
        starts_at: NodeId,
    },
    /// The same connection appears more than once.
    DuplicateConnection {
        /// The connection's incoming road.
        from_road: RoadId,
        /// The connection's outgoing road.
        to_road: RoadId,
    },
    /// The network has more than `u32::MAX` nodes or roads.
    TooLarge,
}

impl fmt::Display for NetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteNode { node } => {
                write!(f, "node {} has a non-finite position", node.0)
            }
            Self::UnknownNode { road, node } => write!(
                f,
                "road {} references node {}, which does not exist",
                road.0, node.0
            ),
            Self::TooFewPoints { road, points } => write!(
                f,
                "road {} has {points} geometry point(s) (needs at least 2)",
                road.0
            ),
            Self::NonFiniteGeometry { road, point } => write!(
                f,
                "road {} has a non-finite coordinate at point {point}",
                road.0
            ),
            Self::ZeroLengthSegment { road, point } => write!(
                f,
                "road {} has a zero-length segment: points {point} and {} coincide",
                road.0,
                point + 1
            ),
            Self::LengthOverflow { road } => write!(
                f,
                "road {}'s length overflows (coordinates must be meters in a local frame)",
                road.0
            ),
            Self::BadSpeedLimit {
                road,
                speed_limit_mps,
            } => write!(
                f,
                "road {} needs a positive, finite speed limit, got {speed_limit_mps} m/s",
                road.0
            ),
            Self::NoLanes { road } => write!(f, "road {} has no lanes", road.0),
            Self::TooManyLanes { road, lanes } => write!(
                f,
                "road {} has {lanes} lanes (at most {MAX_LANES} are supported)",
                road.0
            ),
            Self::TurnLanesMismatch {
                road,
                lanes,
                turn_lanes,
            } => write!(
                f,
                "road {} has {lanes} lane(s) but {turn_lanes} turn_lanes entries (needs none or \
                 one per lane)",
                road.0
            ),
            Self::BadTurns { road, lane, turns } => write!(
                f,
                "road {} lane {lane} has turns {turns:#06b}, which must be non-empty and use \
                 only the bits 0b1111 (left, through, right, U-turn)",
                road.0
            ),
            Self::UnknownRoad {
                from_road,
                to_road,
                missing,
            } => write!(
                f,
                "the connection from road {} to road {} refers to road {}, which does not exist",
                from_road.0, to_road.0, missing.0
            ),
            Self::Disconnected {
                from_road,
                ends_at,
                to_road,
                starts_at,
            } => write!(
                f,
                "the connection from road {} to road {} is impossible: road {} ends at node {} but road {} starts at node {}",
                from_road.0, to_road.0, from_road.0, ends_at.0, to_road.0, starts_at.0
            ),
            Self::DuplicateConnection { from_road, to_road } => write!(
                f,
                "the connection from road {} to road {} appears twice",
                from_road.0, to_road.0
            ),
            Self::TooLarge => write!(f, "the network has more than {} nodes or roads", u32::MAX),
        }
    }
}

impl std::error::Error for NetworkError {}

/// Index of the `i`-th item; only called after the list length was checked.
#[allow(clippy::cast_possible_truncation)]
fn id(i: usize) -> u32 {
    i as u32
}

/// Unit vector along the segment `[a, b]`, which is non-degenerate. Basic
/// IEEE operations only, like the length, so the state hash is portable.
fn heading(segment: &[Point]) -> [f64; 2] {
    let (dx, dy) = (segment[1].x - segment[0].x, segment[1].y - segment[0].y);
    let len = (dx * dx + dy * dy).sqrt();
    [dx / len, dy / len]
}

/// Checks a road's lane count and turn lanes.
fn check_lanes(road: RoadId, r: &Road) -> Result<(), NetworkError> {
    if r.lanes == 0 {
        return Err(NetworkError::NoLanes { road });
    }
    if r.lanes > MAX_LANES {
        return Err(NetworkError::TooManyLanes {
            road,
            lanes: r.lanes,
        });
    }
    if !r.turn_lanes.is_empty() && r.turn_lanes.len() != usize::from(r.lanes) {
        return Err(NetworkError::TurnLanesMismatch {
            road,
            lanes: r.lanes,
            turn_lanes: r.turn_lanes.len(),
        });
    }
    match r
        .turn_lanes
        .iter()
        .position(|t| t.0 == 0 || t.0 & !Turns::ALL.0 != 0)
    {
        Some(lane) => Err(NetworkError::BadTurns {
            road,
            lane,
            turns: r.turn_lanes[lane].0,
        }),
        None => Ok(()),
    }
}

impl EndControl {
    fn to_core(self) -> core::EndControl {
        match self {
            Self::None => core::EndControl::None,
            Self::GiveWay => core::EndControl::GiveWay,
            Self::Stop => core::EndControl::Stop,
            Self::Signals => core::EndControl::Signals,
        }
    }
}

impl RoadClass {
    /// The class's rank: higher is more important, 0 is unknown.
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::LivingStreet => 1,
            Self::Residential => 2,
            Self::Unclassified => 3,
            Self::Tertiary => 4,
            Self::Secondary => 5,
            Self::Primary => 6,
            Self::Trunk => 7,
            Self::Motorway => 8,
        }
    }
}

impl Network {
    /// Checks the network and builds the core's representation of it.
    ///
    /// This is a trust boundary: networks may come from files or remote
    /// clients, and the core assumes valid data.
    pub(crate) fn to_core(&self) -> Result<core::Network, NetworkError> {
        let max = u32::MAX as usize;
        if self.nodes.len() > max || self.roads.len() > max {
            return Err(NetworkError::TooLarge);
        }
        let finite = |p: &Point| p.x.is_finite() && p.y.is_finite();

        let mut nodes = Vec::with_capacity(self.nodes.len());
        for (i, n) in self.nodes.iter().enumerate() {
            if !finite(&n.position) {
                return Err(NetworkError::NonFiniteNode {
                    node: NodeId(id(i)),
                });
            }
            nodes.push([n.position.x, n.position.y]);
        }

        let mut roads = Vec::with_capacity(self.roads.len());
        for (i, r) in self.roads.iter().enumerate() {
            let road = RoadId(id(i));
            for node in [r.from_node, r.to_node] {
                if node.0 as usize >= self.nodes.len() {
                    return Err(NetworkError::UnknownNode { road, node });
                }
            }
            if r.geometry.len() < 2 {
                return Err(NetworkError::TooFewPoints {
                    road,
                    points: r.geometry.len(),
                });
            }
            if let Some(point) = r.geometry.iter().position(|p| !finite(p)) {
                return Err(NetworkError::NonFiniteGeometry { road, point });
            }
            let mut length_m = 0.0;
            for (point, w) in r.geometry.windows(2).enumerate() {
                // Basic IEEE operations only (no `powi`/`hypot`), so the
                // length and thus the state hash match on every platform.
                let (dx, dy) = (w[1].x - w[0].x, w[1].y - w[0].y);
                let segment = (dx * dx + dy * dy).sqrt();
                if segment <= 0.0 {
                    return Err(NetworkError::ZeroLengthSegment { road, point });
                }
                length_m += segment;
            }
            if !length_m.is_finite() {
                return Err(NetworkError::LengthOverflow { road });
            }
            if r.speed_limit_mps <= 0.0 || !r.speed_limit_mps.is_finite() {
                return Err(NetworkError::BadSpeedLimit {
                    road,
                    speed_limit_mps: r.speed_limit_mps,
                });
            }
            check_lanes(road, r)?;
            roads.push(core::Road {
                from: r.from_node.0,
                to: r.to_node.0,
                length_m,
                speed_limit_mps: r.speed_limit_mps,
                lanes: r.lanes,
                start_dir: heading(&r.geometry[..2]),
                end_dir: heading(&r.geometry[r.geometry.len() - 2..]),
                roundabout: r.roundabout,
                class: r.class.rank(),
                priority_road: r.priority_road,
                end_control: r.end_control.to_core(),
                turn_lanes: r.turn_lanes.iter().map(|t| t.0).collect(),
            });
        }

        let mut connections = Vec::with_capacity(self.connections.len());
        let mut seen = HashSet::with_capacity(self.connections.len());
        for c in &self.connections {
            let [from, to] = [c.from_road, c.to_road].map(|missing| {
                self.roads
                    .get(missing.0 as usize)
                    .ok_or(NetworkError::UnknownRoad {
                        from_road: c.from_road,
                        to_road: c.to_road,
                        missing,
                    })
            });
            let (from, to) = (from?, to?);
            if from.to_node != to.from_node {
                return Err(NetworkError::Disconnected {
                    from_road: c.from_road,
                    ends_at: from.to_node,
                    to_road: c.to_road,
                    starts_at: to.from_node,
                });
            }
            if !seen.insert((c.from_road, c.to_road)) {
                return Err(NetworkError::DuplicateConnection {
                    from_road: c.from_road,
                    to_road: c.to_road,
                });
            }
            connections.push((c.from_road.0, c.to_road.0));
        }

        Ok(core::Network {
            nodes,
            roads,
            connections,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn road(geometry: Vec<Point>, roundabout: bool) -> Network {
        let nodes = [0, 1].map(|i| Node {
            position: geometry[i * (geometry.len() - 1)],
            osm_node: None,
        });
        Network {
            nodes: nodes.to_vec(),
            roads: vec![Road {
                from_node: NodeId(0),
                to_node: NodeId(1),
                geometry,
                speed_limit_mps: 10.0,
                lanes: 1,
                name: None,
                osm: None,
                roundabout,
                class: RoadClass::Unknown,
                priority_road: false,
                end_control: EndControl::None,
                turn_lanes: vec![],
            }],
            connections: vec![],
        }
    }

    #[test]
    #[allow(clippy::float_cmp)] // 3-4-5 triangle: exact
    fn core_road_gets_unit_headings_of_first_and_last_segment() {
        let p = |x, y| Point { x, y };
        let core = road(
            vec![p(0.0, 0.0), p(30.0, 0.0), p(30.0, -40.0), p(60.0, 0.0)],
            true,
        )
        .to_core()
        .unwrap();
        let r = &core.roads[0];
        assert_eq!(r.start_dir, [1.0, 0.0]);
        assert_eq!(r.end_dir, [0.6, 0.8]);
        assert!(r.roundabout);
    }
}
