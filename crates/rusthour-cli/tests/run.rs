//! The `run` command's output, as the M1 trial and scripts consume it.

use std::process::Command;

fn run(args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_rusthour"))
        .arg("run")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn run_prints_the_state_hash_and_the_counts() {
    let a = run(&["--steps", "20"]);
    assert_eq!(a["protocol_version"], 3);
    assert_eq!(a["tick"], 20);
    assert!(a["state_hash"].is_u64());
    assert_eq!(a["vehicles"], 0);
    assert_eq!(a["waiting"], 0);
    assert_eq!(
        (a["arrived"].as_u64(), a["removed_stuck"].as_u64()),
        (Some(0), Some(0))
    );
    assert!(a["removed_per_arrival"].is_null(), "no arrivals is not 0%");
    assert!(a.get("stuck_roads").is_none(), "only with --report-stuck");
    assert_eq!(a, run(&["--steps", "20"]), "same seed, same output");
}

#[test]
fn a_missing_map_is_a_clear_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_rusthour"))
        .args(["run", "--map", "/nonexistent.rhnet"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read /nonexistent.rhnet"));
}

#[test]
fn vehicles_without_a_map_is_an_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_rusthour"))
        .args(["run", "--vehicles", "5"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--map"));
}

/// A map of four 15 m roads in a ring, which gridlocks under load.
fn ring_map(dir: &std::path::Path) -> std::path::PathBuf {
    use rusthour_api::{Connection, Network, Node, NodeId, OsmWayPart, Point, Road, RoadId};
    let p = |i: f64| Point {
        x: 15.0 * i,
        y: 0.0,
    };
    let road = |i: u32| Road {
        from_node: NodeId(i),
        to_node: NodeId((i + 1) % 4),
        geometry: vec![p(f64::from(i)), p(f64::from(i + 1))],
        speed_limit_mps: 10.0,
        lanes: 1,
        name: Some(format!("Ring {i}")),
        osm: Some(OsmWayPart {
            way: 700 + i64::from(i),
            from_node: 1,
            to_node: 2,
        }),
        roundabout: true,
        class: rusthour_api::RoadClass::Unknown,
        priority_road: false,
        end_control: rusthour_api::EndControl::None,
        turn_lanes: vec![],
    };
    let network = Network {
        nodes: (0..4)
            .map(|i| Node {
                position: p(f64::from(i)),
                osm_node: None,
            })
            .collect(),
        roads: (0..4).map(road).collect(),
        connections: (0..4)
            .map(|i| Connection {
                from_road: RoadId(i),
                to_road: RoadId((i + 1) % 4),
            })
            .collect(),
    };
    let meta = rusthour_import::rhnet::Metadata::default();
    let path = dir.join("ring.rhnet");
    std::fs::write(&path, rusthour_import::rhnet::encode(&meta, &network)).unwrap();
    path
}

#[test]
fn report_stuck_lists_the_roads_with_removals() {
    let dir = std::env::temp_dir().join(format!("rusthour-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let map = ring_map(&dir);
    let out = run(&[
        "--map",
        map.to_str().unwrap(),
        "--vehicles",
        "40",
        "--steps",
        "30000",
        "--report-stuck",
    ]);
    let removed = out["removed_stuck"].as_u64().unwrap();
    assert!(removed > 0, "the ring should gridlock: {out}");
    assert!(out["removed_per_arrival"].is_f64());
    assert_eq!(out["stuck_roads_total"], 4);
    let roads = out["stuck_roads"].as_array().unwrap();
    assert!(!roads.is_empty() && roads.len() <= 10);
    let sum: u64 = roads.iter().map(|r| r["removed"].as_u64().unwrap()).sum();
    assert_eq!(sum, removed, "four roads, all listed");
    let first = &roads[0];
    assert!(first["name"].as_str().unwrap().starts_with("Ring "));
    assert!(first["osm_way"].as_i64().unwrap() >= 700);
    assert_eq!(first["roundabout"], true);
    std::fs::remove_dir_all(dir).ok();
}
