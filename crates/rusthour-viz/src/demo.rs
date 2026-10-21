//! A small hand-built town grid, shown until `.rhnet` maps load (M1.6).

use rusthour_api::{Network, Node, NodeId, Point, Road};

use crate::MapSource;
use crate::camera::LANE_WIDTH_M;

/// Block size in meters.
const BLOCK_M: f64 = 120.0;
const COLUMNS: u32 = 5;
const ROWS: u32 = 4;
/// Urban speed limit, 50 km/h.
const SPEED_MPS: f64 = 50.0 / 3.6;

/// A 5 × 4 grid of two-way streets; the middle east–west street has two
/// lanes per direction, all others one.
pub fn map() -> MapSource {
    let id = |col: u32, row: u32| NodeId(row * COLUMNS + col);
    let pos = |n: NodeId| Point {
        x: f64::from(n.0 % COLUMNS) * BLOCK_M,
        y: f64::from(n.0 / COLUMNS) * BLOCK_M,
    };
    let nodes = (0..COLUMNS * ROWS)
        .map(|i| Node {
            position: pos(NodeId(i)),
        })
        .collect();

    let mut roads = Vec::new();
    let mut street = |a: NodeId, b: NodeId, lanes: u8| {
        for (from, to) in [(a, b), (b, a)] {
            roads.push(one_way(from, to, pos(from), pos(to), lanes));
        }
    };
    for row in 0..ROWS {
        for col in 0..COLUMNS {
            if col + 1 < COLUMNS {
                street(
                    id(col, row),
                    id(col + 1, row),
                    if row == ROWS / 2 { 2 } else { 1 },
                );
            }
            if row + 1 < ROWS {
                street(id(col, row), id(col, row + 1), 1);
            }
        }
    }

    MapSource {
        network: Network {
            nodes,
            roads,
            connections: vec![],
        },
        attribution: "Demo network",
        attribution_url: None,
    }
}

/// One direction of a street: its centerline sits right of the street's
/// center line, as with right-hand traffic.
fn one_way(from: NodeId, to: NodeId, a: Point, b: Point, lanes: u8) -> Road {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = dx.hypot(dy);
    let offset = f64::from(lanes) * f64::from(LANE_WIDTH_M) / 2.0;
    let (ox, oy) = (dy / len * offset, -dx / len * offset);
    Road {
        from_node: from,
        to_node: to,
        geometry: vec![
            Point {
                x: a.x + ox,
                y: a.y + oy,
            },
            Point {
                x: b.x + ox,
                y: b.y + oy,
            },
        ],
        speed_limit_mps: SPEED_MPS,
        lanes,
    }
}
