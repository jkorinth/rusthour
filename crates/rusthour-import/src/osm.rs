//! The OSM importer (feature `osm`, native-only, ADR-0002): an `.osm.pbf`
//! extract → [`Network`].
//!
//! [`read_pbf`] loads an extract into [`OsmData`], [`import`] turns that into
//! a network clipped to an area's [`Bbox`], and [`keep_central_part`] drops
//! the islands that clipping leaves. Kept small on purpose: drivable
//! roads, lanes, speed limits, turn restrictions and right of way (road
//! class, priority roads, signs, signals and turn lanes; ADR-0005).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::path::Path;

use rusthour_api::{
    Connection, EndControl, MAX_LANES, Network, Node, NodeId, OsmWayPart, Point, Road, RoadClass,
    RoadId, Turns,
};

/// OSM elements, as much of them as the importer needs.
#[derive(Debug, Clone, Default)]
pub struct OsmData {
    /// Node positions by OSM id: `(lat, lon)` in WGS84 degrees.
    pub nodes: HashMap<i64, (f64, f64)>,
    /// Ways in file order.
    pub ways: Vec<Way>,
    /// Relations in file order.
    pub relations: Vec<Relation>,
    /// Tags of the nodes that carry a sign or signal
    /// (`highway=give_way|stop|traffic_signals`), by OSM id.
    pub node_tags: HashMap<i64, Vec<(String, String)>>,
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
                            keep_sign(&mut data, n.id(), n.tags());
                        }
                        Element::DenseNode(n) => {
                            data.nodes.insert(n.id(), (n.lat(), n.lon()));
                            keep_sign(&mut data, n.id(), n.tags());
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

/// Keeps the tags of node `id` if it carries a sign or signal.
fn keep_sign<'a>(
    data: &mut OsmData,
    id: i64,
    tags: impl Iterator<Item = (&'a str, &'a str)> + Clone,
) {
    let sign = |(k, v): (&str, &str)| {
        k == "highway" && matches!(v, "give_way" | "stop" | "traffic_signals")
    };
    if tags.clone().any(sign) {
        let tags = tags.map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        data.node_tags.insert(id, tags);
    }
}

/// The sign or signal a node's tags describe.
fn control(tags: &[(String, String)]) -> Option<EndControl> {
    match tag(tags, "highway")? {
        "give_way" => Some(EndControl::GiveWay),
        "stop" => Some(EndControl::Stop),
        "traffic_signals" => Some(EndControl::Signals),
        _ => None,
    }
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

fn is_roundabout(tags: &[(String, String)]) -> bool {
    matches!(tag(tags, "junction"), Some("roundabout" | "circular"))
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
        _ if matches!(highway, "motorway" | "motorway_link") || is_roundabout(tags) => {
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
    import_with_report(data, bbox).0
}

/// [`import`], with a [`Report`] of the right of way it read.
#[must_use]
pub fn import_with_report(data: &OsmData, bbox: Bbox) -> (Network, Report) {
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
    let mut parts: Vec<Part<'_>> = vec![];
    let mut report = Report::default();
    let mut end_pairs = HashSet::new();
    let mut refs = HashSet::new();
    for (way, highway, dirs, run) in pieces {
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
                parts.push((way, nodes, forward));
                network.roads.push(Road {
                    from_node: node_id(from, &mut network.nodes),
                    to_node: node_id(to, &mut network.nodes),
                    geometry,
                    osm: Some(OsmWayPart {
                        way: way.id,
                        from_node: from,
                        to_node: to,
                    }),
                    ..attributes(way, highway, forward, oneway, &mut report)
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
    resolve_controls(&mut network, &parts, data, &node_ids, &project, &mut report);

    report.count(&network, &parts);
    (network, report)
}

/// Signs and signals more than this far from a road's end, along the road,
/// don't apply to it (ADR-0005).
const SIGN_REACH_M: f64 = 30.0;

/// A road named in a [`Report`]: its name and OSM way id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// The road's name, if any.
    pub name: Option<String>,
    /// Its OSM way id.
    pub way: i64,
}

impl Named {
    fn of(way: i64, name: Option<&String>) -> Self {
        Self {
            name: name.cloned(),
            way,
        }
    }
}

/// What [`import_with_report`] read about right of way, counted before
/// [`keep_central_part`] drops islands. Ignored and ambiguous cases name
/// their roads, so a mapper can check them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Road ends with traffic signals.
    pub signals: usize,
    /// Road ends with a stop sign.
    pub stops: usize,
    /// Road ends with a give-way sign.
    pub give_ways: usize,
    /// Junctions with signals on at least one approach.
    pub signal_nodes: usize,
    /// Groups of two or more signalled junctions joined by short links
    /// (split junctions).
    pub signal_clusters: usize,
    /// Short links inside such groups, left without signals so traffic on
    /// them flows on.
    pub split_links: usize,
    /// Signalled junctions by `(signalled approaches, all approaches)`.
    pub signal_approaches: BTreeMap<(usize, usize), usize>,
    /// Signal nodes on imported roads.
    pub signal_tags: usize,
    /// Signal nodes ignored: mid-block (pedestrian crossings) or at a node
    /// that is no junction.
    pub ignored_signals: Vec<Named>,
    /// Sign nodes on imported roads.
    pub sign_tags: usize,
    /// Signs ignored: too far from a junction, or at a node that is no
    /// junction.
    pub ignored_signs: Vec<Named>,
    /// Signs at a junction without a direction, applied to the approaches of
    /// the lowest class there.
    pub undirected_lowest: Vec<Named>,
    /// Signs at a junction without a direction whose approaches are all of
    /// one class, applied to all of them (all-way stop).
    pub undirected_all_way: Vec<Named>,
    /// Roads with `turn:lanes`.
    pub turn_lanes: usize,
    /// Roads whose `turn:lanes` was dropped: its lane count differs from
    /// `lanes`, or it has an unknown value.
    pub ignored_turn_lanes: Vec<Named>,
    /// Roads whose lane count was clamped to [`MAX_LANES`].
    pub clamped_lanes: Vec<Named>,
    /// OSM ways tagged `priority_road`.
    pub priority_road_ways: usize,
    /// Other OSM ways tagged `traffic_sign=DE:306` (priority road sign).
    pub priority_sign_ways: usize,
    /// Roads of unknown class.
    pub unknown_class: usize,
}

impl Report {
    /// Counts the controlled road ends, priority ways and unknown classes.
    fn count(&mut self, network: &Network, parts: &[Part<'_>]) {
        let mut ways = HashSet::new();
        for (r, (way, ..)) in network.roads.iter().zip(parts) {
            match r.end_control {
                EndControl::Signals => self.signals += 1,
                EndControl::Stop => self.stops += 1,
                EndControl::GiveWay => self.give_ways += 1,
                EndControl::None => {}
            }
            if ways.insert(way.id) {
                match priority(&way.tags) {
                    (true, _) => self.priority_road_ways += 1,
                    (false, true) => self.priority_sign_ways += 1,
                    (false, false) => {}
                }
            }
            self.unknown_class += usize::from(r.class == RoadClass::Unknown);
        }
    }
}

/// `count` and a noun, singular or plural.
fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The first few distinct roads of `list`, or `all` of them, as
/// `Name (way 123)`.
fn first_few(list: &[Named], all: bool) -> String {
    let mut seen = HashSet::new();
    let distinct: Vec<_> = list.iter().filter(|n| seen.insert(n.way)).collect();
    let mut text = distinct
        .iter()
        .take(if all { usize::MAX } else { FEW })
        .map(|n| format!("{} (way {})", n.name.as_deref().unwrap_or("unnamed"), n.way))
        .collect::<Vec<_>>()
        .join(", ");
    if !all && distinct.len() > FEW {
        let _ = write!(text, ", and {} more", distinct.len() - FEW);
    }
    text
}

/// How many roads a report line names, unless verbose.
const FEW: usize = 3;

/// The report as lines; the alternate form (`{:#}`, `--verbose`) names
/// every road instead of the first few.
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_counts(f)?;
        self.write_problems(f, f.alternate())
    }
}

impl Report {
    /// The report's counts: what was applied.
    fn write_counts(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "right of way: {} signalled, {} stop and {} give-way road ends",
            self.signals, self.stops, self.give_ways
        )?;
        let histogram: Vec<_> = self
            .signal_approaches
            .iter()
            .map(|((signalled, of), n)| format!("{signalled} of {of} at {n}"))
            .collect();
        write!(
            f,
            "\nsignals: {}; by signalled of all approaches: {}",
            plural(
                self.signal_nodes,
                "signalled junction",
                "signalled junctions"
            ),
            histogram.join(", ")
        )?;
        if self.signal_clusters > 0 {
            write!(
                f,
                "\nsplit junctions: {} joined by short links; {} of those links left \
                 without signals, so traffic on them can't be trapped between two signals",
                plural(
                    self.signal_clusters,
                    "group of signalled junctions",
                    "groups of signalled junctions"
                ),
                self.split_links
            )?;
        }
        let priority = self.priority_road_ways + self.priority_sign_ways;
        if priority == 0 {
            write!(
                f,
                "\npriority roads: none tagged, road class is used instead"
            )?;
        } else {
            write!(
                f,
                "\npriority roads: {} ({} by priority_road, {} by traffic_sign=DE:306)",
                plural(priority, "way", "ways"),
                self.priority_road_ways,
                self.priority_sign_ways
            )?;
        }
        write!(
            f,
            "; {}",
            plural(
                self.unknown_class,
                "road of unknown class",
                "roads of unknown class"
            )
        )?;
        write!(
            f,
            "\nturn lanes: {}",
            plural(
                self.turn_lanes,
                "road with turn:lanes",
                "roads with turn:lanes"
            )
        )
    }

    /// The ignored and ambiguous cases, naming their first few roads or
    /// `all` of them.
    fn write_problems(&self, f: &mut fmt::Formatter<'_>, all: bool) -> fmt::Result {
        // A warning when more than 5% of a kind point to bad data.
        let line =
            |f: &mut fmt::Formatter<'_>, list: &[Named], total: usize, what: &str, warn: bool| {
                if list.is_empty() {
                    return Ok(());
                }
                let warning = if warn && list.len() * 20 > total {
                    "warning: "
                } else {
                    ""
                };
                write!(
                    f,
                    "\n{warning}{} of {total} {what}: {}",
                    list.len(),
                    first_few(list, all)
                )
            };
        let undirected = [&self.undirected_lowest[..], &self.undirected_all_way[..]].concat();
        line(
            f,
            &undirected,
            self.sign_tags,
            &format!(
                "signs at junctions have no direction ({} applied to the lowest road class there, \
                 {} to every approach as all-way)",
                self.undirected_lowest.len(),
                self.undirected_all_way.len()
            ),
            true,
        )?;
        line(
            f,
            &self.ignored_signs,
            self.sign_tags,
            "signs ignored (not within 30 m of a junction)",
            true,
        )?;
        line(
            f,
            &self.ignored_turn_lanes,
            self.turn_lanes + self.ignored_turn_lanes.len(),
            "turn:lanes ignored (lane count differs from `lanes`, or unknown value)",
            true,
        )?;
        line(
            f,
            &self.clamped_lanes,
            self.clamped_lanes.len(),
            &format!("lane counts clamped to {MAX_LANES}"),
            false,
        )?;
        // Expected in towns: pedestrian crossings.
        line(
            f,
            &self.ignored_signals,
            self.signal_tags,
            "signals not applied (mid-block, as at pedestrian crossings, or not near a junction)",
            false,
        )?;
        let lists = [
            &undirected,
            &self.ignored_signs,
            &self.ignored_turn_lanes,
            &self.clamped_lanes,
            &self.ignored_signals,
        ];
        let distinct = |l: &Vec<Named>| l.iter().map(|n| n.way).collect::<HashSet<_>>().len();
        if !all && lists.into_iter().any(|l| distinct(l) > FEW) {
            write!(f, "\n(run with --verbose to list every road)")?;
        }
        Ok(())
    }
}

/// Whether a way is part of a priority road, as `(by priority_road, by a
/// traffic_sign=DE:306 sign)`.
fn priority(tags: &[(String, String)]) -> (bool, bool) {
    let by_tag = matches!(
        tag(tags, "priority_road"),
        Some("designated" | "yes_unposted")
    );
    // `DE:306`, also in lists such as `DE:306;DE:1020-30` or `DE:306,…`.
    let by_sign = tag(tags, "traffic_sign").is_some_and(|v| {
        v.split([';', ',']).any(|sign| {
            sign.trim()
                .strip_prefix("DE:306")
                .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_digit()))
        })
    });
    (by_tag, by_sign)
}

/// The class of a drivable `highway` value; a link takes its parent's.
fn road_class(highway: &str) -> RoadClass {
    match highway.trim_end_matches("_link") {
        "motorway" => RoadClass::Motorway,
        "trunk" => RoadClass::Trunk,
        "primary" => RoadClass::Primary,
        "secondary" => RoadClass::Secondary,
        "tertiary" => RoadClass::Tertiary,
        "unclassified" => RoadClass::Unclassified,
        "residential" => RoadClass::Residential,
        "living_street" => RoadClass::LivingStreet,
        _ => RoadClass::Unknown,
    }
}

/// A road's `turn:lanes` in one direction, lane 0 (rightmost) first, if
/// tagged; a lane with an unknown value is `None`.
fn turn_lanes(
    tags: &[(String, String)],
    forward: bool,
    oneway: bool,
) -> Option<Vec<Option<Turns>>> {
    let key = if forward {
        "turn:lanes:forward"
    } else {
        "turn:lanes:backward"
    };
    let value = tag(tags, key).or_else(|| tag(tags, "turn:lanes").filter(|_| oneway))?;
    // Listed left to right.
    let lanes = value.split('|').rev().map(|lane| {
        lane.split(';')
            .map(|turn| match turn.trim() {
                "left" | "slight_left" | "sharp_left" => Some(Turns::LEFT),
                "right" | "slight_right" | "sharp_right" => Some(Turns::RIGHT),
                "through" | "merge_to_left" | "merge_to_right" | "none" | "" => {
                    Some(Turns::THROUGH)
                }
                "reverse" => Some(Turns::U_TURN),
                _ => None,
            })
            .try_fold(Turns(0), |turns, turn| Some(turns | turn?))
    });
    Some(lanes.collect())
}

/// A road's attributes from its way's tags in one direction, for
/// [`import_with_report`] to complete with its ends and geometry. Clamps
/// lanes and drops inconsistent turn lanes, and reports both.
fn attributes(way: &Way, highway: &str, forward: bool, oneway: bool, report: &mut Report) -> Road {
    let name = ["name", "ref"]
        .into_iter()
        .find_map(|k| tag(&way.tags, k).filter(|v| !v.is_empty()))
        .map(str::to_owned);
    let name = name.as_ref();
    let mut lanes = lanes(highway, &way.tags, forward, oneway);
    if lanes > MAX_LANES {
        lanes = MAX_LANES;
        report.clamped_lanes.push(Named::of(way.id, name));
    }
    let turn_lanes = turn_lanes(&way.tags, forward, oneway).map_or_else(Vec::new, |turns| {
        match turns.into_iter().collect::<Option<Vec<_>>>() {
            Some(turns) if turns.len() == usize::from(lanes) => {
                report.turn_lanes += 1;
                turns
            }
            _ => {
                report.ignored_turn_lanes.push(Named::of(way.id, name));
                vec![]
            }
        }
    });
    Road {
        from_node: NodeId(0),
        to_node: NodeId(0),
        geometry: vec![],
        speed_limit_mps: speed_limit_mps(highway, &way.tags, forward),
        lanes,
        name: name.cloned(),
        osm: None,
        roundabout: is_roundabout(&way.tags),
        class: road_class(highway),
        priority_road: priority(&way.tags) != (false, false),
        end_control: EndControl::None,
        turn_lanes,
    }
}

/// A road of the import with where it comes from: its way, its OSM nodes
/// in way order, and whether it drives them forward.
type Part<'a> = (&'a Way, &'a [i64], bool);

/// Sets each road's [`Road::end_control`] from the signs and signals on
/// nodes (ADR-0005 §3), and drops signals from short links between
/// signalled junctions.
fn resolve_controls(
    network: &mut Network,
    parts: &[Part<'_>],
    data: &OsmData,
    node_ids: &HashMap<i64, NodeId>,
    project: &impl Fn((f64, f64)) -> Point,
    report: &mut Report,
) {
    let roads = &network.roads;
    let (incoming, outgoing, junction) = topology(network);
    // Roads through each tagged node that isn't a network node.
    let mut through: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, (_, nodes, _)) in parts.iter().enumerate() {
        for id in &nodes[1..nodes.len() - 1] {
            if data.node_tags.contains_key(id) {
                through.entry(*id).or_default().push(i);
            }
        }
    }
    let named = |r: usize| Named::of(parts[r].0.id, roads[r].name.as_ref());
    // The road and distance whose end at a junction lies `d` meters ahead on
    // road `r`, following on where the way merely continues (a way boundary)
    // until out of reach.
    let junction_ahead = |mut r: usize, mut d: f64| {
        // Bounded, in case roads of zero length form a cycle.
        for _ in 0..roads.len() {
            let end = roads[r].to_node.0 as usize;
            if d > SIGN_REACH_M {
                return None;
            }
            if junction[end] {
                return Some((r, d));
            }
            let from = roads[r].from_node;
            let mut on = outgoing[end].iter().filter(|&&o| roads[o].to_node != from);
            match (on.next(), on.next()) {
                (Some(&next), None) => (r, d) = (next, d + road_length_m(&roads[next])),
                _ => return None,
            }
        }
        None
    };

    let mut tagged: Vec<_> = data
        .node_tags
        .iter()
        .filter_map(|(&id, tags)| control(tags).map(|c| (id, c, tags)))
        .collect();
    tagged.sort_unstable_by_key(|&(id, ..)| id);
    let mut set = vec![EndControl::None; roads.len()];
    for (id, c, tags) in tagged {
        let signal = c == EndControl::Signals;
        let direction = direction(tags, signal);
        let along = |r: &usize| direction.is_none_or(|d| parts[*r].2 == d);
        let (applied, seen_on) = if let Some(node) = node_ids.get(&id) {
            let node = node.0 as usize;
            let seen_on = incoming[node]
                .first()
                .copied()
                .or_else(|| roads.iter().position(|r| r.from_node.0 as usize == node));
            let approaches: Vec<usize> = incoming[node].iter().copied().filter(along).collect();
            let applied = if !junction[node] {
                // A node joining two ways, often where a way is split at the
                // sign: as on a road's interior.
                let ends: Vec<_> = approaches
                    .iter()
                    .filter_map(|&r| junction_ahead(r, 0.0))
                    .collect();
                sign_on_road(&ends, !signal && direction.is_none())
            } else if approaches.is_empty() {
                vec![]
            } else if signal || direction.is_some() {
                approaches
            } else {
                undirected_junction_sign(roads, approaches, report, named)
            };
            (applied, seen_on)
        } else if let Some(on) = through.get(&id) {
            let ends: Vec<(usize, f64)> = on
                .iter()
                .filter(|r| along(r))
                .filter_map(|&r| junction_ahead(r, to_end(parts[r], id, data, project)))
                .collect();
            let undirected_sign = !signal && direction.is_none();
            (sign_on_road(&ends, undirected_sign), on.first().copied())
        } else {
            continue; // not on an imported road
        };
        let Some(seen_on) = seen_on else { continue };
        let (tags, ignored) = if signal {
            (&mut report.signal_tags, &mut report.ignored_signals)
        } else {
            (&mut report.sign_tags, &mut report.ignored_signs)
        };
        *tags += 1;
        if applied.is_empty() {
            ignored.push(named(seen_on));
        }
        for r in applied {
            set[r] = stronger(set[r], c);
        }
    }
    split_junctions(network, &mut set, report);
    for (r, c) in network.roads.iter_mut().zip(set) {
        r.end_control = c;
    }
}

/// Whether a sign or signal applies forward (`true`) or backward along its
/// way, if tagged.
fn direction(tags: &[(String, String)], signal: bool) -> Option<bool> {
    let keys: &[&str] = if signal {
        &["traffic_signals:direction", "direction"]
    } else {
        &["direction"]
    };
    keys.iter().find_map(|k| match tag(tags, k) {
        Some("forward") => Some(true),
        Some("backward") => Some(false),
        _ => None,
    })
}

/// Per node: its incoming roads, its outgoing roads, and whether it is a
/// junction. Junctions have three or more arms; a node joining two ways
/// isn't one.
fn topology(network: &Network) -> (Vec<Vec<usize>>, Vec<Vec<usize>>, Vec<bool>) {
    let mut incoming = vec![vec![]; network.nodes.len()];
    let mut outgoing = vec![vec![]; network.nodes.len()];
    let mut neighbors = vec![vec![]; network.nodes.len()];
    for (i, r) in network.roads.iter().enumerate() {
        incoming[r.to_node.0 as usize].push(i);
        outgoing[r.from_node.0 as usize].push(i);
        neighbors[r.to_node.0 as usize].push(r.from_node);
        neighbors[r.from_node.0 as usize].push(r.to_node);
    }
    let junction = neighbors
        .iter_mut()
        .map(|n| {
            n.sort_unstable_by_key(|id| id.0);
            n.dedup();
            n.len() >= 3
        })
        .collect();
    (incoming, outgoing, junction)
}

/// Distance along a road from its OSM node `id` to the road's end.
fn to_end(
    (_, nodes, forward): Part<'_>,
    id: i64,
    data: &OsmData,
    project: &impl Fn((f64, f64)) -> Point,
) -> f64 {
    let k = nodes.iter().position(|&n| n == id).unwrap_or(0);
    let span = if forward { &nodes[k..] } else { &nodes[..=k] };
    span.windows(2)
        .map(|w| {
            let (a, b) = (project(data.nodes[&w[0]]), project(data.nodes[&w[1]]));
            (b.x - a.x).hypot(b.y - a.y)
        })
        .sum()
}

/// The roads a sign or signal on a road's interior applies to, given the
/// `(road, distance to its end)` of the directions that reach a junction.
/// A sign without a direction applies to the nearer end, or to both in the
/// middle of a short link between two junctions.
fn sign_on_road(ends: &[(usize, f64)], undirected_sign: bool) -> Vec<usize> {
    if let [(a, da), (b, db)] = *ends
        && undirected_sign
    {
        let length = da + db;
        let middle = length < 2.0 * SIGN_REACH_M && (da - db).abs() < length / 3.0;
        return if middle {
            vec![a, b]
        } else if da <= db {
            vec![a]
        } else {
            vec![b]
        };
    }
    ends.iter().map(|&(r, _)| r).collect()
}

/// The approaches a sign at a junction without a direction applies to: those
/// of the lowest class, or all of them if all are of one class (all-way).
fn undirected_junction_sign(
    roads: &[Road],
    approaches: Vec<usize>,
    report: &mut Report,
    named: impl Fn(usize) -> Named,
) -> Vec<usize> {
    let rank = |r: &usize| roads[*r].class.rank();
    let lowest = approaches.iter().map(rank).min();
    let all_way = approaches.iter().all(|r| Some(rank(r)) == lowest);
    let applied: Vec<_> = approaches
        .into_iter()
        .filter(|r| Some(rank(r)) == lowest)
        .collect();
    let list = if all_way {
        &mut report.undirected_all_way
    } else {
        &mut report.undirected_lowest
    };
    list.push(named(applied[0]));
    applied
}

/// Split junctions: short links between signalled junctions get no signals,
/// so traffic on them flows on. Counts signalled nodes and their groups.
fn split_junctions(network: &Network, set: &mut [EndControl], report: &mut Report) {
    let mut signalled = vec![false; network.nodes.len()];
    for (r, c) in network.roads.iter().zip(&*set) {
        if *c == EndControl::Signals {
            signalled[r.to_node.0 as usize] = true;
        }
    }
    report.signal_nodes = signalled.iter().filter(|&&s| s).count();
    let mut group: Vec<usize> = (0..network.nodes.len()).collect();
    for (i, r) in network.roads.iter().enumerate() {
        let (a, b) = (r.from_node.0 as usize, r.to_node.0 as usize);
        if signalled[a] && signalled[b] && road_length_m(r) < SIGN_REACH_M {
            if set[i] == EndControl::Signals {
                set[i] = EndControl::None;
                report.split_links += 1;
            }
            let (ra, rb) = (root(&mut group, a), root(&mut group, b));
            group[ra.max(rb)] = ra.min(rb);
        }
    }
    let mut size = vec![0usize; network.nodes.len()];
    for v in (0..size.len()).filter(|&v| signalled[v]) {
        size[root(&mut group, v)] += 1;
    }
    report.signal_clusters = size.iter().filter(|&&n| n >= 2).count();

    let mut approaches = vec![(0, 0); network.nodes.len()];
    for (r, c) in network.roads.iter().zip(&*set) {
        let a = &mut approaches[r.to_node.0 as usize];
        a.0 += usize::from(*c == EndControl::Signals);
        a.1 += 1;
    }
    for (v, a) in approaches.into_iter().enumerate() {
        if signalled[v] {
            *report.signal_approaches.entry(a).or_default() += 1;
        }
    }
}

/// The representative of `v`'s group (union-find with path halving).
fn root(group: &mut [usize], mut v: usize) -> usize {
    while group[v] != v {
        group[v] = group[group[v]];
        v = group[v];
    }
    v
}

/// The stronger of two controls on one road end: signals, then stop, then
/// give-way.
fn stronger(a: EndControl, b: EndControl) -> EndControl {
    let rank = |c| match c {
        EndControl::None => 0,
        EndControl::GiveWay => 1,
        EndControl::Stop => 2,
        EndControl::Signals => 3,
    };
    if rank(b) > rank(a) { b } else { a }
}

/// What [`keep_central_part`] dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Dropped {
    /// Directed roads.
    pub roads: usize,
    /// Their total length in meters, both directions counted.
    pub length_m: f64,
}

/// A road's length along its geometry, in meters.
#[must_use]
pub fn road_length_m(road: &Road) -> f64 {
    road.geometry
        .windows(2)
        .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
        .sum()
}

/// Distance from `p` to the segment `a`–`b`.
#[must_use]
pub fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a.x + t * dx - p.x).hypot(a.y + t * dy - p.y)
}

/// Keeps only the part of `network` linked by roads (in either direction)
/// to the road passing closest to the area center, the origin of
/// [`import`]'s coordinates. Drops islands that no trip can reach: streets
/// whose links run outside the area, or are not drivable.
///
/// shortcut: roads of the kept part that one-way streets make a trap
/// (enter, never leave) stay; routing (M2.4) decides which of them are
/// exits at the area edge.
#[must_use]
pub fn keep_central_part(network: Network) -> (Network, Dropped) {
    let center = Point { x: 0.0, y: 0.0 };
    let distance = |r: &Road| {
        r.geometry
            .windows(2)
            .map(|w| distance_to_segment(center, w[0], w[1]))
            .fold(f64::INFINITY, f64::min)
    };
    let Some(start) = network
        .roads
        .iter()
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
    else {
        return (network, Dropped::default());
    };

    let mut adjacent = vec![vec![]; network.nodes.len()];
    for r in &network.roads {
        adjacent[r.from_node.0 as usize].push(r.to_node.0 as usize);
        adjacent[r.to_node.0 as usize].push(r.from_node.0 as usize);
    }
    let mut kept_node = vec![false; network.nodes.len()];
    let mut stack = vec![start.from_node.0 as usize];
    while let Some(v) = stack.pop() {
        if !std::mem::replace(&mut kept_node[v], true) {
            stack.extend(&adjacent[v]);
        }
    }

    // Same order as before, so imports stay reproducible.
    let mut node_id = vec![None; network.nodes.len()];
    let mut kept = Network::default();
    for (i, node) in network.nodes.into_iter().enumerate() {
        if kept_node[i] {
            node_id[i] = Some(NodeId(index(kept.nodes.len())));
            kept.nodes.push(node);
        }
    }
    let mut road_id = vec![None; network.roads.len()];
    let mut dropped = Dropped::default();
    for (i, mut road) in network.roads.into_iter().enumerate() {
        // A road's ends lie in the same part.
        if let (Some(from), Some(to)) = (
            node_id[road.from_node.0 as usize],
            node_id[road.to_node.0 as usize],
        ) {
            (road.from_node, road.to_node) = (from, to);
            road_id[i] = Some(RoadId(index(kept.roads.len())));
            kept.roads.push(road);
        } else {
            dropped.roads += 1;
            dropped.length_m += road_length_m(&road);
        }
    }
    kept.connections = network
        .connections
        .iter()
        .filter_map(|c| {
            Some(Connection {
                from_road: road_id[c.from_road.0 as usize]?,
                to_road: road_id[c.to_road.0 as usize]?,
            })
        })
        .collect();
    (kept, dropped)
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
