//! The OSM importer (feature `osm`, native-only, ADR-0002): an `.osm.pbf`
//! extract → [`Network`].
//!
//! [`read_pbf`] loads an extract into [`OsmData`], [`import`] turns that into
//! a network clipped to an area's [`Bbox`]. Kept small on purpose: drivable
//! roads, lanes, speed limits and turn restrictions, nothing else yet.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use rusthour_api::{Connection, Network, Node, NodeId, OsmWayPart, Point, Road, RoadId};

/// OSM elements, as much of them as the importer needs.
#[derive(Debug, Clone, Default)]
pub struct OsmData {
    /// Node positions by OSM id: `(lat, lon)` in WGS84 degrees.
    pub nodes: HashMap<i64, (f64, f64)>,
    /// Ways in file order.
    pub ways: Vec<Way>,
    /// Relations in file order.
    pub relations: Vec<Relation>,
    /// The extract's `osmosis_replication_timestamp`, in seconds since the
    /// Unix epoch, if the file header has one.
    pub replication_timestamp: Option<i64>,
}

/// An OSM way.
#[derive(Debug, Clone, Default)]
pub struct Way {
    /// OSM way id.
    pub id: i64,
    /// OSM ids of its nodes, in order.
    pub nodes: Vec<i64>,
    /// Tags as key/value pairs.
    pub tags: Vec<(String, String)>,
}

/// An OSM relation.
#[derive(Debug, Clone, Default)]
pub struct Relation {
    /// OSM relation id.
    pub id: i64,
    /// Members in order.
    pub members: Vec<Member>,
    /// Tags as key/value pairs.
    pub tags: Vec<(String, String)>,
}

/// A relation member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Whether the member is a node, way or relation.
    pub kind: MemberKind,
    /// The member's OSM id.
    pub id: i64,
    /// The member's role, e.g. `from`.
    pub role: String,
}

/// The kind of element a [`Member`] refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    /// An OSM node.
    Node,
    /// An OSM way.
    Way,
    /// An OSM relation.
    Relation,
}

fn tag<'a>(tags: &'a [(String, String)], key: &str) -> Option<&'a str> {
    tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Reads an `.osm.pbf` file: all nodes, ways and relations.
///
/// # Errors
///
/// If the file can't be read or isn't valid PBF.
pub fn read_pbf(path: &Path) -> Result<OsmData, osmpbf::Error> {
    use osmpbf::{BlobDecode, BlobReader, Element, RelMemberType};

    let owned = |tags: osmpbf::TagIter<'_>| {
        tags.map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect::<Vec<_>>()
    };
    let mut data = OsmData::default();
    for blob in BlobReader::from_path(path)? {
        match blob?.decode()? {
            BlobDecode::OsmHeader(header) => {
                data.replication_timestamp = header.osmosis_replication_timestamp();
            }
            BlobDecode::OsmData(block) => {
                for element in block.elements() {
                    match element {
                        Element::Node(n) => {
                            data.nodes.insert(n.id(), (n.lat(), n.lon()));
                        }
                        Element::DenseNode(n) => {
                            data.nodes.insert(n.id(), (n.lat(), n.lon()));
                        }
                        Element::Way(w) => data.ways.push(Way {
                            id: w.id(),
                            nodes: w.refs().collect(),
                            tags: owned(w.tags()),
                        }),
                        Element::Relation(r) => data.relations.push(Relation {
                            id: r.id(),
                            members: r
                                .members()
                                .map(|m| Member {
                                    kind: match m.member_type {
                                        RelMemberType::Node => MemberKind::Node,
                                        RelMemberType::Way => MemberKind::Way,
                                        RelMemberType::Relation => MemberKind::Relation,
                                    },
                                    id: m.member_id,
                                    role: m.role().unwrap_or_default().to_owned(),
                                })
                                .collect(),
                            tags: owned(r.tags()),
                        }),
                    }
                }
            }
            BlobDecode::Unknown(_) => {}
        }
    }
    Ok(data)
}

/// A WGS84 bounding box in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bbox {
    /// Southern edge (minimum latitude).
    pub south: f64,
    /// Western edge (minimum longitude).
    pub west: f64,
    /// Northern edge (maximum latitude).
    pub north: f64,
    /// Eastern edge (maximum longitude).
    pub east: f64,
}

/// Why an area could not be read from `data/areas.toml`.
#[derive(Debug, Clone, PartialEq)]
pub enum AreaError {
    /// The file isn't valid TOML or has no `[areas.<name>]` tables with a
    /// `bbox`.
    Malformed(String),
    /// No area of that name; lists the known ones.
    Unknown {
        /// The requested name.
        area: String,
        /// The names in the file.
        known: Vec<String>,
    },
    /// The area's bbox isn't `[south, west, north, east]` with south < north
    /// and west < east, in degrees.
    BadBbox {
        /// The area's name.
        area: String,
        /// The bbox as written.
        bbox: [f64; 4],
    },
}

impl fmt::Display for AreaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(e) => write!(f, "the areas file is malformed: {e}"),
            Self::Unknown { area, known } => write!(
                f,
                "no area named `{area}`; known areas: {}",
                known.join(", ")
            ),
            Self::BadBbox { area, bbox } => write!(
                f,
                "area `{area}` has an invalid bbox {bbox:?}; it must be [south, west, north, \
                 east] in degrees, with south < north and west < east"
            ),
        }
    }
}

impl std::error::Error for AreaError {}

impl Bbox {
    /// Reads area `area`'s `bbox = [south, west, north, east]` from the
    /// contents of `data/areas.toml`.
    ///
    /// # Errors
    ///
    /// See [`AreaError`].
    pub fn from_areas_toml(areas_toml: &str, area: &str) -> Result<Self, AreaError> {
        #[derive(serde::Deserialize)]
        struct File {
            areas: std::collections::BTreeMap<String, Area>,
        }
        #[derive(serde::Deserialize)]
        struct Area {
            bbox: [f64; 4],
        }
        let file: File =
            toml::from_str(areas_toml).map_err(|e| AreaError::Malformed(e.to_string()))?;
        let Some(found) = file.areas.get(area) else {
            return Err(AreaError::Unknown {
                area: area.to_owned(),
                known: file.areas.into_keys().collect(),
            });
        };
        let [south, west, north, east] = found.bbox;
        let degrees = |v: f64, max: f64| v.is_finite() && v.abs() <= max;
        let lat = degrees(south, 90.0) && degrees(north, 90.0) && south < north;
        let lon = degrees(west, 180.0) && degrees(east, 180.0) && west < east;
        if !(lat && lon) {
            return Err(AreaError::BadBbox {
                area: area.to_owned(),
                bbox: found.bbox,
            });
        }
        Ok(Self {
            south,
            west,
            north,
            east,
        })
    }

    fn contains(&self, (lat, lon): (f64, f64)) -> bool {
        (self.south..=self.north).contains(&lat) && (self.west..=self.east).contains(&lon)
    }
}

/// Mean Earth radius in meters (IUGG).
const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// Projects WGS84 degrees into the network's planar frame: meters, `x` east,
/// `y` north, origin at the center of `bbox`.
///
/// Local equirectangular projection: `x = R·Δlon·cos(lat₀)`, `y = R·Δlat`
/// with `lat₀` the box's center latitude. Over a few kilometers at mid
/// latitudes its distance error stays well below a meter (it grows with the
/// square of the distance from the center). `cos` is evaluated once, for the
/// constant `lat₀`; everything else is basic IEEE arithmetic, so results are
/// the same wherever that `cos` is.
fn projection(bbox: Bbox) -> impl Fn((f64, f64)) -> Point {
    let lat0 = f64::midpoint(bbox.south, bbox.north);
    let lon0 = f64::midpoint(bbox.west, bbox.east);
    let m_per_deg_lat = EARTH_RADIUS_M * std::f64::consts::PI / 180.0;
    let m_per_deg_lon = m_per_deg_lat * lat0.to_radians().cos();
    move |(lat, lon)| Point {
        x: (lon - lon0) * m_per_deg_lon,
        y: (lat - lat0) * m_per_deg_lat,
    }
}

/// Drivable `highway` values. `service` is left out: in towns it is mostly
/// parking aisles and driveways, i.e. many short dead ends no trip uses.
const DRIVABLE: &[&str] = &[
    "motorway",
    "motorway_link",
    "trunk",
    "trunk_link",
    "primary",
    "primary_link",
    "secondary",
    "secondary_link",
    "tertiary",
    "tertiary_link",
    "unclassified",
    "residential",
    "living_street",
];

/// Which way(s) traffic may drive along a way's node order.
#[derive(Clone, Copy)]
struct Directions {
    forward: bool,
    backward: bool,
}

fn directions(highway: &str, tags: &[(String, String)]) -> Option<Directions> {
    let one = |forward| Directions {
        forward,
        backward: !forward,
    };
    match tag(tags, "oneway") {
        Some("yes" | "true" | "1") => Some(one(true)),
        Some("-1") => Some(one(false)),
        // Direction changes over time; not modeled.
        Some("reversible" | "alternating") => None,
        Some("no" | "false" | "0") => Some(Directions {
            forward: true,
            backward: true,
        }),
        _ if matches!(highway, "motorway" | "motorway_link")
            || matches!(tag(tags, "junction"), Some("roundabout" | "circular")) =>
        {
            Some(one(true))
        }
        _ => Some(Directions {
            forward: true,
            backward: true,
        }),
    }
}

/// Lanes in one direction.
fn lanes(highway: &str, tags: &[(String, String)], forward: bool, oneway: bool) -> u8 {
    let parse = |key| tag(tags, key).and_then(|v| v.trim().parse::<u8>().ok());
    let directional = parse(if forward {
        "lanes:forward"
    } else {
        "lanes:backward"
    });
    let from_total = parse("lanes").map(|total| match (oneway, forward) {
        (true, _) => total,
        (false, true) => total.div_ceil(2),
        (false, false) => total / 2,
    });
    let default = if matches!(highway, "motorway" | "trunk") {
        2
    } else {
        1
    };
    directional.or(from_total).unwrap_or(default).max(1)
}

const LIVING_STREET_KMH: f64 = 7.0;

/// Parses a `maxspeed` value into km/h.
fn parse_maxspeed(value: &str) -> Option<f64> {
    let value = value.trim();
    let kmh = match value {
        "DE:urban" => 50.0,
        "DE:rural" => 100.0,
        // No limit: use the advisory speed.
        "DE:motorway" | "none" => 130.0,
        "walk" | "DE:living_street" => LIVING_STREET_KMH,
        "DE:bicycle_road" => 30.0,
        // Tempo-30 (and 20/10) zones: `DE:zone30` or `DE:zone:30`.
        _ if value.starts_with("DE:zone") => value["DE:zone".len()..]
            .trim_start_matches(':')
            .parse()
            .ok()?,
        _ => match value.strip_suffix("mph") {
            Some(mph) => mph.trim().parse::<f64>().ok()? * 1.609_344,
            None => value
                .strip_suffix("km/h")
                .unwrap_or(value)
                .trim()
                .parse()
                .ok()?,
        },
    };
    (kmh > 0.0 && kmh.is_finite()).then_some(kmh)
}

/// Speed limit in one direction, in m/s.
fn speed_limit_mps(highway: &str, tags: &[(String, String)], forward: bool) -> f64 {
    let key = if forward {
        "maxspeed:forward"
    } else {
        "maxspeed:backward"
    };
    // shortcut: defaults assume built-up areas except on motorways and trunk
    // roads (no urban/rural boundary detection); fine for town maps, detect
    // boundaries (`boundary=place`/traffic signs) for rural areas.
    let default = match highway {
        "motorway" => 130.0,
        "trunk" => 100.0,
        "living_street" => LIVING_STREET_KMH,
        h if h.ends_with("_link") => 60.0,
        _ => 50.0,
    };
    let kmh = [key, "maxspeed"]
        .into_iter()
        .find_map(|k| tag(tags, k).and_then(parse_maxspeed))
        .unwrap_or(default);
    kmh / 3.6
}

/// Imports the drivable roads of `data` inside `bbox` as a network.
///
/// Ways are cut at their last node inside the box: the segment that crosses
/// the edge is dropped, as is a way with no node inside even if it crosses
/// the box. Nodes at a cut become dead ends.
/// Ways are split into roads at their ends and at nodes they share with
/// other roads (or revisit). Each node of the network comes from an OSM
/// node; roads carry their way and end nodes ([`OsmWayPart`]). At every node,
/// each incoming road connects to each outgoing road except its own reverse,
/// then turn restrictions remove movements. The output depends only on the
/// input (including its element order), so imports are reproducible.
// shortcut: boundary nodes look like real dead ends; boundary through-traffic
// (M5.4) needs them marked, i.e. a flag or list in schema v3.
#[must_use]
pub fn import(data: &OsmData, bbox: Bbox) -> Network {
    let project = projection(bbox);
    let inside = |id: &i64| data.nodes.get(id).is_some_and(|&p| bbox.contains(p));

    // Runs of consecutive inside nodes of drivable ways, with their directions.
    let mut pieces = vec![];
    for way in &data.ways {
        let Some(highway) = tag(&way.tags, "highway").filter(|h| DRIVABLE.contains(h)) else {
            continue;
        };
        let Some(dirs) = directions(highway, &way.tags) else {
            continue;
        };
        for run in way.nodes.split(|id| !inside(id)) {
            if run.len() >= 2 {
                pieces.push((way, highway, dirs, run));
            }
        }
    }

    // Split points: piece ends and nodes used more than once.
    let mut uses: HashMap<i64, u32> = HashMap::new();
    for (.., run) in &pieces {
        for &id in *run {
            *uses.entry(id).or_default() += 1;
        }
        for end in [run[0], run[run.len() - 1]] {
            *uses.entry(end).or_default() += 2;
        }
    }
    let split = |id: &i64| uses[id] >= 2;

    let mut network = Network::default();
    let mut node_ids: HashMap<i64, NodeId> = HashMap::new();
    let mut node_id = |osm: i64, nodes: &mut Vec<Node>| {
        *node_ids.entry(osm).or_insert_with(|| {
            nodes.push(Node {
                position: project(data.nodes[&osm]),
                osm_node: Some(osm),
            });
            NodeId(index(nodes.len() - 1))
        })
    };
    // For each road, the road driving the same segment backwards.
    let mut twin = vec![];
    let mut end_pairs = HashSet::new();
    let mut refs = HashSet::new();
    for (way, highway, dirs, run) in pieces {
        let name = ["name", "ref"]
            .into_iter()
            .find_map(|k| tag(&way.tags, k).filter(|v| !v.is_empty()))
            .map(str::to_owned);
        let oneway = !(dirs.forward && dirs.backward);
        let mut segments = vec![];
        let mut start = 0;
        for end in 1..run.len() {
            if split(&run[end]) {
                split_segment(way.id, &run[start..=end], &mut end_pairs, &mut segments);
                start = end;
            }
        }
        for nodes in segments {
            let mut geometry: Vec<Point> = nodes.iter().map(|id| project(data.nodes[id])).collect();
            // Distinct OSM nodes can share a position.
            geometry.dedup();
            if geometry.len() < 2 {
                continue;
            }
            let first = network.roads.len();
            for (forward, allowed) in [(true, dirs.forward), (false, dirs.backward)] {
                if !allowed {
                    continue;
                }
                let (mut from, mut to) = (nodes[0], nodes[nodes.len() - 1]);
                if !forward {
                    (from, to) = (to, from);
                }
                // Refs identify roads (M7.0); a second way part between the
                // same two nodes is dropped.
                if !refs.insert((way.id, from, to)) {
                    continue;
                }
                let mut geometry = geometry.clone();
                if !forward {
                    geometry.reverse();
                }
                network.roads.push(Road {
                    from_node: node_id(from, &mut network.nodes),
                    to_node: node_id(to, &mut network.nodes),
                    geometry,
                    speed_limit_mps: speed_limit_mps(highway, &way.tags, forward),
                    lanes: lanes(highway, &way.tags, forward, oneway),
                    name: name.clone(),
                    osm: Some(OsmWayPart {
                        way: way.id,
                        from_node: from,
                        to_node: to,
                    }),
                });
                twin.push(None);
            }
            if network.roads.len() == first + 2 {
                twin[first] = Some(first + 1);
                twin[first + 1] = Some(first);
            }
        }
    }
    connect(&mut network, &twin, &node_ids, &data.relations);
    network
}

/// Adds a way segment to `out`, split at interior nodes until its ends
/// differ and no earlier segment of the way has the same two ends, so road
/// refs stay unique (a ring street or a way that revisits a node).
fn split_segment<'a>(
    way: i64,
    nodes: &'a [i64],
    end_pairs: &mut HashSet<(i64, i64, i64)>,
    out: &mut Vec<&'a [i64]>,
) {
    let (a, b) = (nodes[0], nodes[nodes.len() - 1]);
    if nodes.len() > 2 && (a == b || !end_pairs.insert((way, a.min(b), a.max(b)))) {
        let mid = nodes.len() / 2;
        split_segment(way, &nodes[..=mid], end_pairs, out);
        split_segment(way, &nodes[mid..], end_pairs, out);
    } else {
        end_pairs.insert((way, a.min(b), a.max(b)));
        out.push(nodes);
    }
}

/// Adds every movement at every node, except U-turns onto the same segment
/// (`twin`), then applies the turn restrictions.
fn connect(
    network: &mut Network,
    twin: &[Option<usize>],
    node_ids: &HashMap<i64, NodeId>,
    relations: &[Relation],
) {
    let mut outgoing = vec![vec![]; network.nodes.len()];
    for (i, road) in network.roads.iter().enumerate() {
        outgoing[road.from_node.0 as usize].push(i);
    }
    for (i, road) in network.roads.iter().enumerate() {
        for &j in &outgoing[road.to_node.0 as usize] {
            if twin[i] != Some(j) {
                network.connections.push(Connection {
                    from_road: RoadId(index(i)),
                    to_road: RoadId(index(j)),
                });
            }
        }
    }

    for relation in relations {
        let Some((only, from_way, via, to_way)) = restriction(relation) else {
            continue;
        };
        let Some(&via) = node_ids.get(&via) else {
            continue;
        };
        let roads = &network.roads;
        let on = |road: RoadId, way| roads[road.0 as usize].osm.is_some_and(|o| o.way == way);
        let from_via = |c: &Connection| {
            roads[c.from_road.0 as usize].to_node == via && on(c.from_road, from_way)
        };
        // U-turns onto the same segment are never connected, so `no_u_turn`
        // from a way onto itself has nothing left to remove; applying it
        // would cut straight-on traffic where the way passes the via node.
        if !only && from_way == to_way {
            continue;
        }
        // shortcut: an `only_*` whose to-way isn't reachable here (clipped,
        // not drivable, one-way the wrong way) is ignored rather than
        // stranding the from-road; revisit if routing (M2) needs it.
        if only
            && !network
                .connections
                .iter()
                .any(|c| from_via(c) && on(c.to_road, to_way))
        {
            continue;
        }
        // `no_*` drops the movement onto the to-way, `only_*` all others.
        network
            .connections
            .retain(|c| !from_via(c) || on(c.to_road, to_way) == only);
    }
}

/// A simple turn restriction: `(only, from way, via node, to way)`.
///
/// shortcut: via-way, conditional, vehicle-specific (`restriction:<mode>`)
/// restrictions and `except` tags are ignored; add them when routing needs
/// them (M2).
fn restriction(relation: &Relation) -> Option<(bool, i64, i64, i64)> {
    if tag(&relation.tags, "type") != Some("restriction") {
        return None;
    }
    let value = tag(&relation.tags, "restriction")?;
    let only = if value.starts_with("only_") {
        true
    } else if value.starts_with("no_") {
        false
    } else {
        return None;
    };
    let single = |role: &str, kind| {
        let mut found = relation.members.iter().filter(|m| m.role == role);
        match (found.next(), found.next()) {
            (Some(m), None) if m.kind == kind => Some(m.id),
            _ => None,
        }
    };
    Some((
        only,
        single("from", MemberKind::Way)?,
        single("via", MemberKind::Node)?,
        single("to", MemberKind::Way)?,
    ))
}

/// Index of an item just pushed.
fn index(i: usize) -> u32 {
    u32::try_from(i).expect("networks have fewer than 2^32 nodes and roads")
}
