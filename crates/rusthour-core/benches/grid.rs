//! A synthetic grid network shared by the benchmarks and the allocation test.

use rusthour_core::network::{Network, Road};

/// An `n × n` grid of nodes 100 m apart, two one-lane roads (one per
/// direction) between neighbours, and every movement except U-turns.
///
/// # Panics
/// If `n` is too large for `u32` ids.
#[must_use]
pub fn grid(n: u32) -> Network {
    let mut net = Network::default();
    for y in 0..n {
        for x in 0..n {
            net.nodes.push([f64::from(x) * 100.0, f64::from(y) * 100.0]);
        }
    }
    let add = |net: &mut Network, from: u32, to: u32| {
        let (a, b) = (net.nodes[from as usize], net.nodes[to as usize]);
        let dir = [(b[0] - a[0]) / 100.0, (b[1] - a[1]) / 100.0];
        net.roads.push(Road {
            from,
            to,
            length_m: 100.0,
            speed_limit_mps: 13.9,
            lanes: 1,
            start_dir: dir,
            end_dir: dir,
            roundabout: false,
        });
    };
    for y in 0..n {
        for x in 0..n {
            let i = y * n + x;
            if x + 1 < n {
                add(&mut net, i, i + 1);
                add(&mut net, i + 1, i);
            }
            if y + 1 < n {
                add(&mut net, i, i + n);
                add(&mut net, i + n, i);
            }
        }
    }
    for (a, ra) in net.roads.iter().enumerate() {
        for (b, rb) in net.roads.iter().enumerate() {
            if ra.to == rb.from && ra.from != rb.to {
                net.connections
                    .push((u32::try_from(a).unwrap(), u32::try_from(b).unwrap()));
            }
        }
    }
    net
}
