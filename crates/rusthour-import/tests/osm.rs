//! The OSM importer (M1.2).

use std::collections::HashSet;
use std::path::Path;

use proptest::prelude::*;
use proptest::sample::Index;
use rusthour_api::{Config, InProcess, Network, Point, Road, RoadId};
use rusthour_import::osm::{self, AreaError, Bbox, Member, MemberKind, OsmData, Relation, Way};
use rusthour_import::rhnet::{self, Metadata};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/crossroads.osm.pbf"
);

fn bbox(swne: &str) -> Bbox {
    Bbox::from_areas_toml(&format!("[areas.test]\nbbox = {swne}\n"), "test").unwrap()
}

/// Covers the whole fixture.
fn fixture_bbox() -> Bbox {
    bbox("[49.6790, 8.6180, 49.6810, 8.6240]")
}

fn fixture() -> OsmData {
    osm::read_pbf(Path::new(FIXTURE)).unwrap()
}

/// The road along OSM way `way` from OSM node `from` to `to`.
fn road(network: &Network, way: i64, from: i64, to: i64) -> Option<(RoadId, &Road)> {
    network.roads.iter().enumerate().find_map(|(i, r)| {
        let o = r.osm?;
        (o.way == way && o.from_node == from && o.to_node == to)
            .then(|| (RoadId(u32::try_from(i).unwrap()), r))
    })
}

fn connected(network: &Network, from: RoadId, to: RoadId) -> bool {
    network
        .connections
        .iter()
        .any(|c| c.from_road == from && c.to_road == to)
}

#[allow(clippy::cast_possible_truncation)]
fn kmh(r: &Road) -> i64 {
    (r.speed_limit_mps * 3.6).round() as i64
}

#[test]
fn crossroads_fixture_imports_as_tagged() {
    let data = fixture();
    // 2026-10-01T00:00:00Z
    assert_eq!(data.replication_timestamp, Some(1_790_812_800));
    let network = osm::import(&data, fixture_bbox());
    InProcess::with_network(Config::default(), 0, network.clone()).unwrap();

    // Ends 5, 2, 3, 7 and junctions 1, 4; node 6 is only geometry.
    let mut osm_nodes: Vec<_> = network.nodes.iter().map(|n| n.osm_node.unwrap()).collect();
    osm_nodes.sort_unstable();
    assert_eq!(osm_nodes, [1, 2, 3, 4, 5, 7]);
    // Four two-way ways and the one-way Schulgäßchen.
    assert_eq!(network.roads.len(), 9);
    // Node 1: 3 in × 4 out − 3 U-turns − the banned left turn; node 4: 2 × 2 − 2.
    assert_eq!(network.connections.len(), 10);

    let (_, schule) = road(&network, 102, 1, 3).unwrap();
    assert!(road(&network, 102, 3, 1).is_none());
    assert_eq!((schule.lanes, kmh(schule)), (1, 30));

    let (west_in, haupt) = road(&network, 100, 5, 1).unwrap();
    assert!(road(&network, 100, 1, 5).is_some());
    assert_eq!((haupt.lanes, kmh(haupt)), (1, 50));
    assert_eq!(haupt.name.as_deref(), Some("Hauptstraße"));

    let (_, mueller) = road(&network, 103, 4, 7).unwrap();
    assert_eq!(mueller.geometry.len(), 3);
    assert_eq!(kmh(mueller), 50, "DE:urban");
    let (to_dead_end, _) = road(&network, 103, 4, 7).unwrap();
    assert!(
        network
            .connections
            .iter()
            .all(|c| c.from_road != to_dead_end)
    );

    // no_left_turn from Hauptstraße (west) onto Bahnhofstraße (north).
    let (north, _) = road(&network, 101, 1, 2).unwrap();
    let (east, _) = road(&network, 104, 1, 4).unwrap();
    assert!(!connected(&network, west_in, north));
    assert!(connected(&network, west_in, east));

    // Names with umlauts and ß survive the asset format.
    let (_, network) = rhnet::decode(&rhnet::encode(&Metadata::default(), &network)).unwrap();
    let (_, schule) = road(&network, 102, 1, 3).unwrap();
    assert_eq!(schule.name.as_deref(), Some("Schulgäßchen"));
}

#[test]
fn ways_are_cut_at_the_area_edge() {
    // East edge between Müllerweg's nodes 6 (8.6224) and 7 (8.6236).
    let network = osm::import(&fixture(), bbox("[49.6790, 8.6180, 49.6810, 8.6230]"));
    let (to_cut, mueller) = road(&network, 103, 4, 6).unwrap();
    assert_eq!(mueller.geometry.len(), 2);
    assert!(network.nodes.iter().all(|n| n.osm_node != Some(7)));
    assert!(network.connections.iter().all(|c| c.from_road != to_cut));

    // x east, y north, origin at the box center (49.68, 8.6205).
    let (_, north) = road(&network, 101, 1, 2).unwrap();
    let end = north.geometry[1];
    assert!(end.x.abs() < 37.0 && end.x < -35.0, "{end:?}");
    assert!((end.y - 100.1).abs() < 0.1, "{end:?}");

    // The box is [south, west, north, east]; north before south is an error.
    let swapped = "[areas.test]\nbbox = [49.6810, 8.6180, 49.6790, 8.6240]\n";
    assert!(matches!(
        Bbox::from_areas_toml(swapped, "test"),
        Err(AreaError::BadBbox { .. })
    ));
    assert!(matches!(
        Bbox::from_areas_toml(swapped, "nowhere"),
        Err(AreaError::Unknown { known, .. }) if known == ["test"]
    ));
}

#[test]
fn imports_are_byte_identical() {
    let asset = || {
        rhnet::encode(
            &Metadata::default(),
            &osm::import(&fixture(), fixture_bbox()),
        )
    };
    assert_eq!(asset(), asset());
}

/// Random small extracts: nodes on a coarse grid (so positions collide) that
/// reaches past the area, ways with random and malformed tags and dangling
/// node references, and usually one turn restriction.
fn extract() -> impl Strategy<Value = OsmData> {
    let tag = |key: &'static str, values: &'static [&'static str]| {
        prop::option::of(prop::sample::select(values))
            .prop_map(move |v| v.map(|v| (key.to_owned(), v.to_owned())))
    };
    let tags = (
        prop::sample::select(
            &[
                "residential",
                "motorway",
                "trunk",
                "primary_link",
                "living_street",
                "footway",
                "service",
            ][..],
        )
        .prop_map(|v| Some(("highway".to_owned(), v.to_owned()))),
        tag("oneway", &["yes", "-1", "no", "reversible", "bogus"]),
        tag("junction", &["roundabout"]),
        tag("lanes", &["0", "1", "3", "255", "x"]),
        tag("lanes:forward", &["2", "0"]),
        tag(
            "maxspeed",
            &[
                "30",
                "20 mph",
                "DE:urban",
                "DE:zone:30",
                "walk",
                "none",
                "0",
                "-5",
                "abc",
            ],
        ),
        tag("maxspeed:backward", &["70", "NaN", "inf"]),
        tag("name", &["Müllerweg", ""]),
    )
        .prop_map(|t| {
            [t.0, t.1, t.2, t.3, t.4, t.5, t.6, t.7]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
        });
    let way = (prop::collection::vec(1i64..14, 1..7), tags);
    // One restriction, usually well formed, with its via node on the from-way.
    let restriction = (
        prop::sample::select(
            &[
                "no_left_turn",
                "no_u_turn",
                "only_straight_on",
                "only_right_turn",
                "bogus",
            ][..],
        ),
        any::<(Index, Index, Index)>(),
        0..3u8,
        prop::bool::weighted(0.85),
    );
    (
        prop::collection::vec((0..5u8, 0..5u8), 12),
        prop::collection::vec(way, 0..10),
        prop::option::weighted(0.9, restriction),
    )
        .prop_map(|(cells, ways, restriction)| {
            let ways: Vec<Way> = (100..)
                .zip(ways)
                .map(|(id, (nodes, tags))| Way { id, nodes, tags })
                .collect();
            let relations = restriction
                .filter(|_| !ways.is_empty())
                .map(|r| restriction_relation(&ways, r))
                .into_iter()
                .collect();
            OsmData {
                // Nodes 1..=12, about two thirds inside the area; ids 13 and up
                // are missing.
                nodes: (1..)
                    .zip(cells)
                    .map(|(id, (r, c))| {
                        let p = (
                            49.6796 + f64::from(r) * 0.0006,
                            8.6186 + f64::from(c) * 0.0008,
                        );
                        (id, p)
                    })
                    .collect(),
                ways,
                relations,
                replication_timestamp: None,
            }
        })
}

/// A restriction relation from a way of `ways`, via one of its nodes, to the
/// same way, another way through that node or any way.
fn restriction_relation(
    ways: &[Way],
    (value, (from, via, to), to_choice, well_formed): (&str, (Index, Index, Index), u8, bool),
) -> Relation {
    let from = from.get(ways);
    let via = *via.get(&from.nodes);
    // Same way (U-turns), another way through the via node
    // (turns), or any way (often unreachable).
    let through: Vec<_> = ways
        .iter()
        .filter(|w| w.id != from.id && w.nodes.contains(&via))
        .collect();
    let to = match to_choice {
        0 => from,
        1 if !through.is_empty() => *to.get(&through),
        _ => to.get(ways),
    };
    let member = |kind, id, role: &str| Member {
        kind,
        id,
        role: role.to_owned(),
    };
    let mut members = vec![
        member(MemberKind::Way, from.id, "from"),
        member(MemberKind::Node, via, "via"),
        member(MemberKind::Way, to.id, "to"),
    ];
    if !well_formed {
        members.push(member(MemberKind::Way, from.id, "to"));
    }
    Relation {
        id: 1,
        members,
        tags: vec![
            ("type".into(), "restriction".into()),
            ("restriction".into(), value.into()),
        ],
    }
}

/// Area for [`extract`]: the 4 × 4 lower-left cells of its 5 × 5 grid.
fn extract_bbox() -> Bbox {
    bbox("[49.6795, 8.6185, 49.6817, 8.6213]")
}

type Movement = (u32, u32);

fn movements(network: &Network) -> HashSet<Movement> {
    let set: HashSet<_> = network
        .connections
        .iter()
        .map(|c| (c.from_road.0, c.to_road.0))
        .collect();
    assert_eq!(set.len(), network.connections.len());
    set
}

/// The movements `relation` must remove from `base` (the import without it):
/// a well-formed `no_*` removes from-way → to-way at the via node unless both
/// are the same way (U-turns are never connected anyway); `only_*` removes
/// the other movements from the from-way there, unless the to-way can't be
/// reached there at all.
fn removed_by(relation: &Relation, base: &Network) -> HashSet<Movement> {
    let single = |role: &str, kind| {
        let found: Vec<_> = relation.members.iter().filter(|m| m.role == role).collect();
        match found[..] {
            [m] if m.kind == kind => Some(m.id),
            _ => None,
        }
    };
    let value = relation.tags[1].1.as_str();
    let (Some(from), Some(via), Some(to)) = (
        single("from", MemberKind::Way),
        single("via", MemberKind::Node),
        single("to", MemberKind::Way),
    ) else {
        return HashSet::new();
    };
    let road = |id: u32| &base.roads[id as usize];
    let on = |id: u32, way| road(id).osm.unwrap().way == way;
    let at_via: HashSet<Movement> = movements(base)
        .into_iter()
        .filter(|&(f, _)| {
            on(f, from) && base.nodes[road(f).to_node.0 as usize].osm_node == Some(via)
        })
        .collect();
    if value.starts_with("no_") && from != to {
        at_via.into_iter().filter(|&(_, t)| on(t, to)).collect()
    } else if value.starts_with("only_") && at_via.iter().any(|&(_, t)| on(t, to)) {
        at_via.into_iter().filter(|&(_, t)| !on(t, to)).collect()
    } else {
        HashSet::new()
    }
}

proptest! {
    #[test]
    fn random_extracts_import_to_valid_networks(data in extract()) {
        let network = osm::import(&data, extract_bbox());
        if let Err(e) = InProcess::with_network(Config::default(), 0, network.clone()) {
            prop_assert!(false, "invalid network: {e}");
        }

        // OSM refs identify roads (M7.0) and match the end nodes.
        let mut refs = HashSet::new();
        for r in &network.roads {
            let o = r.osm.unwrap();
            prop_assert!(refs.insert((o.way, o.from_node, o.to_node)), "duplicate {o:?}");
            prop_assert_eq!(network.nodes[r.from_node.0 as usize].osm_node, Some(o.from_node));
            prop_assert_eq!(network.nodes[r.to_node.0 as usize].osm_node, Some(o.to_node));
        }
        // No U-turn on a two-way way: its reverse ref is the same segment.
        let two_way = |way: i64| {
            let tags = &data.ways.iter().find(|w| w.id == way).unwrap().tags;
            let has = |k: &str, v: &[&str]| tags.iter().any(|(tk, tv)| tk == k && v.contains(&tv.as_str()));
            !has("oneway", &["yes", "-1", "reversible"])
                && !has("junction", &["roundabout"])
                && !has("highway", &["motorway"])
        };
        for &(f, t) in &movements(&network) {
            let (a, b) = (network.roads[f as usize].osm.unwrap(), network.roads[t as usize].osm.unwrap());
            let reverse = (b.way, b.from_node, b.to_node) == (a.way, a.to_node, a.from_node);
            prop_assert!(!reverse || !two_way(a.way), "U-turn {a:?} → {b:?}");
        }
        // The restriction removes exactly what it says.
        let base = osm::import(&OsmData { relations: vec![], ..data.clone() }, extract_bbox());
        prop_assert_eq!(&base.roads, &network.roads);
        let expected = data.relations.first().map(|r| removed_by(r, &base)).unwrap_or_default();
        let removed: HashSet<_> = movements(&base).difference(&movements(&network)).copied().collect();
        prop_assert!(movements(&network).is_subset(&movements(&base)));
        prop_assert_eq!(removed, expected);
    }
}

/// The OSM ref of the road passing closest to the area center, the origin.
fn central_ref(network: &Network) -> Option<(i64, i64, i64)> {
    let dist = |r: &Road| {
        r.geometry
            .windows(2)
            .map(|w| osm::distance_to_segment(Point { x: 0.0, y: 0.0 }, w[0], w[1]))
            .fold(f64::INFINITY, f64::min)
    };
    let r = network
        .roads
        .iter()
        .min_by(|a, b| dist(a).total_cmp(&dist(b)))?;
    r.osm.map(|o| (o.way, o.from_node, o.to_node))
}

proptest! {
    /// The kept part is connected, holds the road nearest the center, and
    /// keeps all of that road's part with its movements; the rest is counted.
    #[test]
    fn only_the_part_at_the_center_is_kept(data in extract()) {
        let full = osm::import(&data, extract_bbox());
        let (kept, dropped) = osm::keep_central_part(full.clone());
        if let Err(e) = InProcess::with_network(Config::default(), 0, kept.clone()) {
            prop_assert!(false, "invalid network: {e}");
        }
        prop_assert_eq!(kept.roads.len() + dropped.roads, full.roads.len());
        let length = |n: &Network| n.roads.iter().map(osm::road_length_m).sum::<f64>();
        prop_assert!((length(&kept) + dropped.length_m - length(&full)).abs() < 1e-6);
        prop_assert_eq!(central_ref(&kept), central_ref(&full));

        // One part: every node reaches every other along roads, either way.
        let mut seen = vec![false; kept.nodes.len()];
        let mut stack: Vec<usize> = (!kept.nodes.is_empty()).then_some(0).into_iter().collect();
        while let Some(v) = stack.pop() {
            if seen[v] { continue; }
            seen[v] = true;
            for r in &kept.roads {
                let (a, b) = (r.from_node.0 as usize, r.to_node.0 as usize);
                if a == v { stack.push(b); }
                if b == v { stack.push(a); }
            }
        }
        prop_assert!(seen.iter().all(|&s| s), "kept network is not connected");

        // Movements between kept roads survive unchanged.
        let key = |r: &Road| r.osm.map(|o| (o.way, o.from_node, o.to_node));
        let refs = |n: &Network, m: &HashSet<Movement>| -> HashSet<_> {
            m.iter().map(|&(f, t)| (key(&n.roads[f as usize]), key(&n.roads[t as usize]))).collect()
        };
        let kept_refs: HashSet<_> = kept.roads.iter().map(key).collect();
        let expected: HashSet<_> = refs(&full, &movements(&full))
            .into_iter()
            .filter(|(f, t)| kept_refs.contains(f) && kept_refs.contains(t))
            .collect();
        prop_assert_eq!(refs(&kept, &movements(&kept)), expected);
    }
}

#[test]
fn a_small_part_at_the_center_wins_over_a_bigger_one() {
    // A short street through the center, and a longer one 500 m east.
    let node = |id: i64, x: f64| (id, (49.68, 8.6205 + x / 72_000.0));
    let data = OsmData {
        nodes: [
            node(1, -10.0),
            node(2, 10.0),
            node(3, 500.0),
            node(4, 600.0),
            node(5, 700.0),
        ]
        .into_iter()
        .collect(),
        ways: [(10, vec![1, 2]), (11, vec![3, 4, 5])]
            .into_iter()
            .map(|(id, nodes)| Way {
                id,
                nodes,
                tags: vec![("highway".into(), "residential".into())],
            })
            .collect(),
        ..OsmData::default()
    };
    let full = osm::import(&data, bbox("[49.67, 8.60, 49.69, 8.64]"));
    // Both streets are two-way: two roads each.
    assert_eq!(full.roads.len(), 4);
    let (kept, dropped) = osm::keep_central_part(full);
    assert!(kept.roads.iter().all(|r| r.osm.unwrap().way == 10));
    assert_eq!((kept.roads.len(), dropped.roads), (2, 2));
    assert!(
        (dropped.length_m - 400.0).abs() < 2.0,
        "{}",
        dropped.length_m
    );
}

#[test]
fn german_speed_limit_values() {
    for (value, expected) in [
        ("DE:zone30", 30),
        ("DE:zone:30", 30),
        ("DE:zone:20", 20),
        ("DE:bicycle_road", 30),
        ("DE:urban", 50),
        ("20 mph", 32),
    ] {
        let data = OsmData {
            nodes: [(1, (49.68, 8.62)), (2, (49.681, 8.62))].into(),
            ways: vec![Way {
                id: 1,
                nodes: vec![1, 2],
                tags: vec![
                    ("highway".into(), "residential".into()),
                    ("maxspeed".into(), value.into()),
                ],
            }],
            ..OsmData::default()
        };
        let network = osm::import(&data, bbox("[49.6, 8.6, 49.7, 8.7]"));
        assert_eq!(kmh(&network.roads[0]), expected, "{value}");
    }
}
