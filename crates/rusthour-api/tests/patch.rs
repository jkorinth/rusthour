//! Network patches (M5.1, ADR-0008): serialization, resolution by stable
//! keys, and what applying them does to a simulation.

use proptest::prelude::*;
use rusthour_api::{
    Config, Connection, Edit, EndControl, InProcess, MapRef, Network, Node, NodeId, NodeRef,
    OsmWayPart, Patch, PatchError, Point, Road, RoadClass, RoadId, RoadRef, Simulation, Unresolved,
    VehicleSnapshot, apply,
};

/// OSM id of grid node `i`.
fn osm_node(i: usize) -> i64 {
    1000 + i64::try_from(i).unwrap()
}

/// `k`×`k` nodes 200 m apart joined by two-way streets, every movement but
/// U-turns allowed. Row `r` is OSM way `100 + r`, column `c` way `200 + c`,
/// so a way has several roads per direction.
fn grid(k: usize) -> Network {
    #[allow(clippy::cast_precision_loss)]
    let at = |i: usize| Point {
        x: (i % k) as f64 * 200.0,
        y: (i / k) as f64 * 200.0,
    };
    let mut roads = vec![];
    for a in 0..k * k {
        let right = (a % k + 1 < k).then(|| (a + 1, 100 + a / k));
        let up = (a / k + 1 < k).then(|| (a + k, 200 + a % k));
        for (b, way) in right.into_iter().chain(up) {
            for (from, to) in [(a, b), (b, a)] {
                roads.push(Road {
                    from_node: NodeId(u32::try_from(from).unwrap()),
                    to_node: NodeId(u32::try_from(to).unwrap()),
                    geometry: vec![at(from), at(to)],
                    speed_limit_mps: 13.9,
                    lanes: 1,
                    name: None,
                    osm: Some(OsmWayPart {
                        way: i64::try_from(way).unwrap(),
                        from_node: osm_node(from),
                        to_node: osm_node(to),
                    }),
                    roundabout: false,
                    class: RoadClass::Residential,
                    priority_road: false,
                    end_control: EndControl::None,
                    turn_lanes: vec![],
                });
            }
        }
    }
    let id = |i: usize| RoadId(u32::try_from(i).unwrap());
    let mut connections = vec![];
    for (i, a) in roads.iter().enumerate() {
        for (j, b) in roads.iter().enumerate() {
            if a.to_node == b.from_node && b.to_node != a.from_node {
                connections.push(Connection {
                    from_road: id(i),
                    to_road: id(j),
                });
            }
        }
    }
    Network {
        nodes: (0..k * k)
            .map(|i| Node {
                position: at(i),
                osm_node: Some(osm_node(i)),
            })
            .collect(),
        roads,
        connections,
    }
}

/// The same network with its nodes and roads listed in another order.
fn permuted(net: &Network, node_order: &[usize], road_order: &[usize]) -> Network {
    let inverse = |order: &[usize]| {
        let mut inv = vec![0u32; order.len()];
        for (new, &old) in order.iter().enumerate() {
            inv[old] = u32::try_from(new).unwrap();
        }
        inv
    };
    let (node_at, road_at) = (inverse(node_order), inverse(road_order));
    Network {
        nodes: node_order.iter().map(|&i| net.nodes[i].clone()).collect(),
        roads: (road_order.iter())
            .map(|&i| Road {
                from_node: NodeId(node_at[net.roads[i].from_node.0 as usize]),
                to_node: NodeId(node_at[net.roads[i].to_node.0 as usize]),
                ..net.roads[i].clone()
            })
            .collect(),
        connections: (net.connections.iter())
            .map(|c| Connection {
                from_road: RoadId(road_at[c.from_road.0 as usize]),
                to_road: RoadId(road_at[c.to_road.0 as usize]),
            })
            .collect(),
    }
}

fn key(net: &Network, road: usize) -> RoadRef {
    RoadRef::Osm(net.roads[road].osm.unwrap())
}

fn patch(edits: Vec<Edit>) -> Vec<Patch> {
    vec![Patch { map: None, edits }]
}

fn road_ref() -> impl Strategy<Value = RoadRef> {
    prop_oneof![
        (any::<i64>(), any::<i64>(), any::<i64>()).prop_map(|(way, from_node, to_node)| {
            RoadRef::Osm(OsmWayPart {
                way,
                from_node,
                to_node,
            })
        }),
        ("[a-z0-9-]{1,12}", any::<u16>()).prop_map(|(edit, n)| RoadRef::Created { edit, n }),
    ]
}

fn edit() -> impl Strategy<Value = Edit> {
    let id = "\\PC{1,16}";
    let control = prop::sample::select(vec![
        EndControl::None,
        EndControl::GiveWay,
        EndControl::Stop,
        EndControl::Signals,
    ]);
    let node = prop_oneof![
        any::<i64>().prop_map(|node| NodeRef::Osm { node }),
        ("[a-z0-9-]{1,12}", any::<u16>()).prop_map(|(edit, n)| NodeRef::Created { edit, n }),
    ];
    prop_oneof![
        (id, node, control.clone()).prop_map(|(id, junction, control)| Edit::JunctionControl {
            id,
            junction,
            control
        }),
        (id, road_ref(), control).prop_map(|(id, road, control)| Edit::ArmControl {
            id,
            road,
            control
        }),
        (id, road_ref(), any::<u8>()).prop_map(|(id, road, lanes)| Edit::Lanes { id, road, lanes }),
        (id, road_ref(), "\\PC*").prop_map(|(id, road, turn_lanes)| Edit::TurnLanes {
            id,
            road,
            turn_lanes
        }),
        (id, road_ref(), -1e300..1e300).prop_map(|(id, road, kmh)| Edit::SpeedLimit {
            id,
            road,
            kmh
        }),
        (id, road_ref()).prop_map(|(id, road)| Edit::Close { id, road }),
        (id, road_ref()).prop_map(|(id, road)| Edit::Reopen { id, road }),
    ]
}

proptest! {
    #[test]
    fn patches_round_trip_through_toml_and_json(
        map in proptest::option::of(("\\PC*", "\\PC*")),
        edits in prop::collection::vec(edit(), 0..6),
    ) {
        let patch = Patch {
            map: map.map(|(area, source_date)| MapRef { area, source_date }),
            edits,
        };
        let toml = toml::to_string(&patch).unwrap();
        prop_assert_eq!(&toml::from_str::<Patch>(&toml).unwrap(), &patch, "{}", toml);
        let json = serde_json::to_string(&patch).unwrap();
        prop_assert_eq!(&serde_json::from_str::<Patch>(&json).unwrap(), &patch);
    }

    /// A key either resolves to exactly the road it names, on any listing
    /// order of the network, or the patch fails and changes nothing.
    #[test]
    fn a_patch_edits_exactly_the_addressed_road_or_fails(
        (k, road, target, nodes, roads) in (2usize..5).prop_flat_map(|k| {
            let roads = 4 * k * (k - 1);
            (
                Just(k),
                0..roads,
                proptest::option::of((99i64..=103, 999i64..=1000 + 25, 999i64..=1000 + 25)),
                Just((0..k * k).collect::<Vec<_>>()).prop_shuffle(),
                Just((0..roads).collect::<Vec<_>>()).prop_shuffle(),
            )
        }),
    ) {
        let base = grid(k);
        // An existing key, or one made of plausible but arbitrary ids.
        let target = target.map_or_else(
            || key(&base, road),
            |(way, from_node, to_node)| RoadRef::Osm(OsmWayPart { way, from_node, to_node }),
        );
        let edits = patch(vec![Edit::SpeedLimit { id: "slow".into(), road: target.clone(), kmh: 18.0 }]);
        let exists = (0..base.roads.len()).any(|r| key(&base, r) == target);
        for net in [base.clone(), permuted(&base, &nodes, &roads)] {
            match apply(&net, &MapRef::default(), &edits) {
                Ok((out, _, warnings)) => {
                    prop_assert!(exists && warnings.is_empty());
                    for (r, (a, b)) in net.roads.iter().zip(&out.roads).enumerate() {
                        if key(&net, r) == target {
                            prop_assert!((b.speed_limit_mps - 5.0).abs() < 1e-9);
                        } else {
                            prop_assert_eq!(a, b);
                        }
                    }
                    prop_assert_eq!(&out.connections, &net.connections);
                }
                Err(errors) => {
                    prop_assert!(!exists);
                    let [PatchError::Unresolved { why: Unresolved::NotFound { near }, .. }] =
                        &errors[..]
                    else {
                        panic!("{errors:?}");
                    };
                    prop_assert!(!near.contains(&target));
                }
            }
        }
    }
}

fn sim(net: &Network, patches: &[Patch], seed: u64) -> InProcess {
    let (net, _, _) = apply(net, &MapRef::default(), patches).unwrap();
    let config = Config {
        target_vehicles: 60,
        ..Config::default()
    };
    InProcess::with_network(config, seed, net).unwrap()
}

/// Vehicles seen on `road` in snapshots every 10 s over `steps`, and the
/// highest speed seen on its second half, after entering vehicles braked.
fn watch(sim: &mut InProcess, road: usize, steps: u64) -> (usize, f32) {
    let mut snap = VehicleSnapshot::default();
    let (mut seen, mut top) = (0, 0.0f32);
    for _ in 0..steps / 100 {
        sim.step(100);
        sim.snapshot_into(&mut snap);
        for i in 0..snap.len() {
            if snap.road[i].0 as usize == road {
                seen += 1;
                if snap.offset_m[i] > 100.0 {
                    top = top.max(snap.speed_mps[i]);
                }
            }
        }
    }
    (seen, top)
}

#[test]
fn closing_a_road_routes_traffic_around_it() {
    let net = grid(3);
    let road = 0;
    let close = patch(vec![Edit::Close {
        id: "close-0".into(),
        road: key(&net, road),
    }]);
    let (before, _) = watch(&mut sim(&net, &[], 7), road, 6000);
    let mut closed = sim(&net, &close, 7);
    let (after, _) = watch(&mut closed, road, 6000);
    assert!(before > 0, "the road carries traffic before");
    assert_eq!(after, 0);
    assert!(closed.status().arrived > 50, "{:?}", closed.status());
    assert!(
        closed
            .network()
            .connections
            .iter()
            .all(|c| c.from_road.0 != 0 && c.to_road.0 != 0)
    );
}

#[test]
fn a_speed_limit_changes_the_free_flow_speed() {
    let net = grid(3);
    let road = 0;
    let slow = patch(vec![Edit::SpeedLimit {
        id: "slow".into(),
        road: key(&net, road),
        kmh: 10.0,
    }]);
    let (_, fast_top) = watch(&mut sim(&net, &[], 7), road, 6000);
    let mut slowed = sim(&net, &slow, 7);
    let (_, slow_top) = watch(&mut slowed, road, 6000);
    let limit = 10.0 / 3.6;
    assert!((slowed.network().roads[road].speed_limit_mps - f64::from(limit)).abs() < 1e-6);
    assert!(fast_top > 2.0 * limit, "{fast_top}");
    assert!(
        slow_top > 0.5 * limit && slow_top < 1.25 * limit,
        "{slow_top}"
    );
}

#[test]
fn stop_signs_on_a_junction_change_its_control() {
    let net = grid(3);
    let centre = NodeRef::Osm { node: osm_node(4) };
    let stop = patch(vec![Edit::JunctionControl {
        id: "all-way-stop".into(),
        junction: centre,
        control: EndControl::Stop,
    }]);
    let arms: Vec<usize> = (0..net.roads.len())
        .filter(|&r| net.roads[r].to_node == NodeId(4))
        .collect();
    let prior = |sim: &InProcess| {
        sim.inspect_road(RoadId(u32::try_from(arms[0]).unwrap()))
            .unwrap()
            .movements[0]
            .prior_s
    };
    let (plain, stopped) = (sim(&net, &[], 7), sim(&net, &stop, 7));
    assert_eq!(arms.len(), 4);
    for (r, road) in stopped.network().roads.iter().enumerate() {
        let want = if arms.contains(&r) {
            EndControl::Stop
        } else {
            EndControl::None
        };
        assert_eq!(road.end_control, want);
    }
    assert!(prior(&stopped) > prior(&plain), "routing sees the stop");
}

#[test]
fn an_unresolved_key_fails_with_suggestions_and_applies_nothing() {
    let net = grid(3);
    // Way 100 (row 0) from node 1000 to 1002, as if node 1001 were new.
    let gone = RoadRef::Osm(OsmWayPart {
        way: 100,
        from_node: 1000,
        to_node: 1002,
    });
    let edits = patch(vec![
        Edit::SpeedLimit {
            id: "slow".into(),
            road: key(&net, 0),
            kmh: 30.0,
        },
        Edit::Close {
            id: "close-row".into(),
            road: gone.clone(),
        },
        Edit::JunctionControl {
            id: "stop".into(),
            junction: NodeRef::Osm { node: 5 },
            control: EndControl::Stop,
        },
    ]);
    let errors = apply(&net, &MapRef::default(), &edits).unwrap_err();
    assert_eq!(errors.len(), 2, "all failures, in edit order: {errors:?}");
    let PatchError::Unresolved {
        edit,
        why: Unresolved::NotFound { near },
        ..
    } = &errors[0]
    else {
        panic!("{errors:?}");
    };
    assert_eq!(edit, "close-row");
    let row0 = |from, to| {
        RoadRef::Osm(OsmWayPart {
            way: 100,
            from_node: from,
            to_node: to,
        })
    };
    assert!(near.contains(&row0(1000, 1001)) && near.contains(&row0(1001, 1002)));
    assert!(near.is_sorted());
    let message = errors[0].to_string();
    assert!(
        message.contains("`close-row`") && message.contains("way 100 from 1000 to 1002"),
        "{message}"
    );
    assert!(message.contains("way 100 from 1001 to 1002"), "{message}");
    assert!(errors[1].to_string().contains("node 5"), "{}", errors[1]);
}

#[test]
fn the_same_seed_and_patch_give_the_same_hash() {
    let net = grid(3);
    let edits = patch(vec![
        Edit::Close {
            id: "close".into(),
            road: key(&net, 3),
        },
        Edit::Lanes {
            id: "wide".into(),
            road: key(&net, 5),
            lanes: 2,
        },
    ]);
    let hash = |patches: &[Patch]| {
        let mut s = sim(&net, patches, 42);
        s.step(3000);
        s.state_hash()
    };
    assert_eq!(hash(&edits), hash(&edits));
    assert_ne!(hash(&edits), hash(&[]));
}

#[test]
fn later_edits_win_with_a_warning_and_lanes_drop_stale_turn_lanes() {
    let net = grid(2);
    let edits = patch(vec![
        Edit::TurnLanes {
            id: "turns".into(),
            road: key(&net, 0),
            turn_lanes: "left|through;right".into(),
        },
        Edit::Lanes {
            id: "three".into(),
            road: key(&net, 0),
            lanes: 3,
        },
        Edit::Close {
            id: "close".into(),
            road: key(&net, 1),
        },
        Edit::Reopen {
            id: "reopen".into(),
            road: key(&net, 1),
        },
    ]);
    let (out, _, warnings) = apply(&net, &MapRef::default(), &edits).unwrap();
    assert_eq!((out.roads[0].lanes, out.roads[0].turn_lanes.len()), (3, 0));
    assert_eq!(out.connections, net.connections, "reopened");
    let text: Vec<String> = warnings.iter().map(ToString::to_string).collect();
    assert_eq!(text.len(), 3, "{text:?}");
    assert!(text[0].contains("`three` overrides `turns`"), "{text:?}");
    assert!(text[1].contains("turn lanes"), "{text:?}");
    assert!(text[2].contains("`reopen` overrides `close`"), "{text:?}");
}

#[test]
fn a_closure_that_cuts_off_roads_warns() {
    let net = grid(3);
    // Both roads out of corner node 0: the roads into it become dead ends.
    let out_of_0: Vec<usize> = (0..net.roads.len())
        .filter(|&r| net.roads[r].from_node == NodeId(0))
        .collect();
    let edits = patch(
        (out_of_0.iter())
            .map(|&r| Edit::Close {
                id: format!("close-{r}"),
                road: key(&net, r),
            })
            .collect(),
    );
    let (_, _, warnings) = apply(&net, &MapRef::default(), &edits).unwrap();
    let [warning] = &warnings[..] else {
        panic!("{warnings:?}")
    };
    let text = warning.to_string();
    assert!(text.contains("cuts off 2 roads"), "{text}");
    assert!(text.contains("way 100 from 1001 to 1000"), "{text}");
}

#[test]
fn a_hand_written_toml_patch_parses_and_typos_are_errors() {
    let text = r#"
        [map]
        area = "bensheim-5km"
        source_date = "2026-11-10"

        [[edit]]
        kind = "speed_limit"
        id = "slow-b47"
        road = { way = 198045888, from_node = 15837308, to_node = 1573379680 }
        kmh = 30

        [[edit]]
        kind = "junction_control"
        id = "stop-1"
        junction = { node = 15837308 }
        control = "stop"
    "#;
    let patch: Patch = toml::from_str(text).unwrap();
    assert_eq!(patch.edits.len(), 2);
    assert_eq!(
        patch.edits[0],
        Edit::SpeedLimit {
            id: "slow-b47".into(),
            road: RoadRef::Osm(OsmWayPart {
                way: 198_045_888,
                from_node: 15_837_308,
                to_node: 1_573_379_680
            }),
            kmh: 30.0,
        }
    );
    let typo = text.replace("kmh = 30", "kmh = 30\nlane = 2");
    let err = toml::from_str::<Patch>(&typo).unwrap_err().to_string();
    assert!(err.contains("lane"), "{err}");
}

#[test]
fn a_map_with_a_duplicate_key_cannot_be_patched_or_loaded() {
    let mut net = grid(2);
    net.roads[1].osm = net.roads[0].osm;
    let errors = apply(&net, &MapRef::default(), &[]).unwrap_err();
    assert!(matches!(
        errors[..],
        [PatchError::Map(
            rusthour_api::NetworkError::DuplicateOsmRoad { .. }
        )]
    ));
    assert!(InProcess::with_network(Config::default(), 1, net).is_err());
}
