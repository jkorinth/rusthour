//! The `.rhnet` asset format (M1.3), as property tests.

use proptest::prelude::*;
use proptest::sample::Index;
use rusthour_api::{
    Connection, NETWORK_SCHEMA_VERSION, Network, Node, NodeId, OsmWayPart, Point, Road, RoadId,
};
use rusthour_import::rhnet::{DecodeError, Metadata, decode, encode};

/// Any network the format must carry, valid or not (validation is the API's
/// job). Floats cover everything but NaN, which `==` can't compare; sign of
/// zero, subnormals and infinities included.
fn network() -> impl Strategy<Value = Network> {
    use prop::num::f64::{INFINITE, NEGATIVE, NORMAL, POSITIVE, SUBNORMAL, ZERO};
    let float = POSITIVE | NEGATIVE | NORMAL | SUBNORMAL | ZERO | INFINITE;
    let point = (float, float).prop_map(|(x, y)| Point { x, y });
    let road = (
        any::<u32>(),
        any::<u32>(),
        prop::collection::vec(point.clone(), 0..6),
        float,
        any::<u8>(),
        any::<Option<String>>(),
        any::<Option<(i64, i64, i64)>>(),
    )
        .prop_map(
            |(from, to, geometry, speed_limit_mps, lanes, name, osm)| Road {
                from_node: NodeId(from),
                to_node: NodeId(to),
                geometry,
                speed_limit_mps,
                lanes,
                name,
                osm: osm.map(|(way, from_node, to_node)| OsmWayPart {
                    way,
                    from_node,
                    to_node,
                }),
            },
        );
    let connection = (any::<u32>(), any::<u32>()).prop_map(|(a, b)| Connection {
        from_road: RoadId(a),
        to_road: RoadId(b),
    });
    (
        prop::collection::vec(
            (point, any::<Option<i64>>())
                .prop_map(|(position, osm_node)| Node { position, osm_node }),
            0..30,
        ),
        prop::collection::vec(road, 0..40),
        prop::collection::vec(connection, 0..40),
    )
        .prop_map(|(nodes, roads, connections)| Network {
            nodes,
            roads,
            connections,
        })
}

fn metadata() -> impl Strategy<Value = Metadata> {
    (
        any::<String>(),
        any::<String>(),
        any::<String>(),
        any::<Option<String>>(),
        any::<String>(),
    )
        .prop_map(
            |(area, source_date, importer_version, osm2streets_version, attribution)| Metadata {
                area,
                source_date,
                importer_version,
                osm2streets_version,
                attribution,
            },
        )
}

proptest! {
    #[test]
    fn round_trip_is_lossless_and_deterministic(network in network(), metadata in metadata()) {
        let bytes = encode(&metadata, &network);
        prop_assert_eq!(&encode(&metadata.clone(), &network.clone()), &bytes);
        let (m, n) = decode(&bytes).unwrap();
        prop_assert_eq!(m, metadata);
        // `Debug` prints f64 exactly, including the sign of zero, which `==` ignores.
        prop_assert_eq!(format!("{n:?}"), format!("{network:?}"));
    }

    #[test]
    fn corrupt_or_truncated_files_are_rejected(
        network in network(),
        metadata in metadata(),
        at in any::<Index>(),
        flip in 1..=255u8,
        truncate in any::<bool>(),
    ) {
        let mut bytes = encode(&metadata, &network);
        let i = at.index(bytes.len());
        if truncate {
            bytes.truncate(i);
        } else {
            bytes[i] ^= flip;
        }
        prop_assert!(decode(&bytes).is_err());
    }
}

#[test]
fn version_errors_say_which_side_to_update() {
    let v = NETWORK_SCHEMA_VERSION;
    let file = |schema: u32| {
        let mut bytes = encode(&Metadata::default(), &Network::default());
        bytes[12..16].copy_from_slice(&schema.to_le_bytes());
        decode(&bytes).unwrap_err()
    };
    let newer = file(v + 1);
    assert_eq!(
        newer,
        DecodeError::UnsupportedSchemaVersion {
            found: v + 1,
            supported: v,
        }
    );
    assert_eq!(
        newer.to_string(),
        format!(
            "this map uses network schema version {}, but this version of Rust Hour only \
             reads version {v}; update Rust Hour (in the browser, reload the page) to open it",
            v + 1
        )
    );
    assert_eq!(
        file(v - 1).to_string(),
        format!(
            "this map uses network schema version {}, which this version of Rust Hour no \
             longer reads (it reads version {v}); re-run the importer to update the map",
            v - 1
        )
    );
}

#[test]
fn inputs_that_are_not_maps_are_named() {
    let bytes = encode(&Metadata::default(), &Network::default());
    assert_eq!(decode(b""), Err(DecodeError::TooShort { actual: 0 }));
    assert_eq!(
        decode(&bytes[..5]),
        Err(DecodeError::TooShort { actual: 5 })
    );
    assert_eq!(
        decode(&bytes[..20]),
        Err(DecodeError::TooShort { actual: 20 })
    );
    assert_eq!(decode(b"<!doctype html>"), Err(DecodeError::WebPage));
    assert_eq!(decode(b"PK\x03\x04"), Err(DecodeError::NotRhnet));
}
