//! Maps read from `.rhnet` bytes (ADR-0002).

use rusthour_api::Network;

/// Where OpenStreetMap's attribution links to.
pub const OSM_COPYRIGHT_URL: &str = "https://www.openstreetmap.org/copyright";

/// A network to show, with the attribution its source requires.
pub struct MapSource {
    /// The area's name, e.g. `bensheim-5km`.
    pub name: String,
    /// The network, in meters.
    pub network: Network,
    /// Shown at all times, e.g. "© OpenStreetMap contributors" for OSM maps.
    pub attribution: String,
    /// Where the attribution links to: the OSM copyright page for OSM maps.
    pub attribution_url: Option<&'static str>,
    /// Date of the map data, `YYYY-MM-DD`, if known.
    pub source_date: Option<String>,
}

/// What to do about a map that can't be decoded.
#[cfg(not(target_arch = "wasm32"))]
const DECODE_NEXT_STEP: &str = "Choose a .rhnet file made by rusthour-import.";
#[cfg(target_arch = "wasm32")]
const DECODE_NEXT_STEP: &str =
    "Reload the page; if this keeps happening, the map file on the server is damaged.";

/// Decodes a `.rhnet` file into a map, or a message for the user: plain
/// words and a next step first, the decoder's details last.
pub fn from_rhnet(bytes: &[u8]) -> Result<MapSource, String> {
    let (metadata, network) = rusthour_import::rhnet::decode(bytes).map_err(|e| {
        format!(
            "This is not a Rust Hour map, or it is damaged. {DECODE_NEXT_STEP}\n\nDetails: {e}."
        )
    })?;
    Ok(MapSource {
        name: metadata.area,
        network,
        attribution_url: metadata
            .attribution
            .contains("OpenStreetMap")
            .then_some(OSM_COPYRIGHT_URL),
        attribution: metadata.attribution,
        // The importer writes `unknown` for extracts without a timestamp.
        source_date: Some(metadata.source_date).filter(|d| !d.is_empty() && d != "unknown"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Found in the M1 user trial (docs/trials/M1.md, bug 1): maps with an
    /// unknown source date showed "data of unknown".
    #[test]
    fn regression_m1_trial_unknown_source_date_is_hidden() {
        let metadata = rusthour_import::rhnet::Metadata {
            source_date: "unknown".into(),
            ..Default::default()
        };
        let bytes = rusthour_import::rhnet::encode(&metadata, &rusthour_api::Network::default());
        assert_eq!(from_rhnet(&bytes).unwrap().source_date, None);
    }

    #[test]
    fn bad_bytes_give_the_decoders_message() {
        let e = from_rhnet(b"<!doctype html>").err().unwrap();
        assert!(e.contains("web page"), "{e}");
    }
}
