//! Rust Hour map assets (ADR-0002): the [`rhnet`] format, which reads and
//! writes networks on every target including `wasm32-unknown-unknown`, and,
//! with the native-only `osm` feature, the OSM importer ([`osm`]).

#[cfg(all(feature = "osm", target_arch = "wasm32"))]
compile_error!("the `osm` feature of rusthour-import is native-only (ADR-0002)");

#[cfg(feature = "osm")]
pub mod osm;
pub mod rhnet;
