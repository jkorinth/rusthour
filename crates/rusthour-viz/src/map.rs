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

/// What to do about a map made for a newer viewer.
#[cfg(not(target_arch = "wasm32"))]
const NEWER_NEXT_STEP: &str = "Update Rust Hour to open it.";
#[cfg(target_arch = "wasm32")]
const NEWER_NEXT_STEP: &str = "Reload the page to get the latest viewer.";
/// What to do about a map made for an older viewer.
#[cfg(not(target_arch = "wasm32"))]
const OLDER_NEXT_STEP: &str = "Re-import it with rusthour-import to open it here.";
#[cfg(target_arch = "wasm32")]
const OLDER_NEXT_STEP: &str = "Reload the page; the maps on the server may still be being updated, so try again in a few \
     minutes.";

/// Decodes a `.rhnet` file into a map, or a message for the user: plain
/// words and a next step first, the decoder's details last.
pub fn from_rhnet(bytes: &[u8]) -> Result<MapSource, String> {
    use rusthour_import::rhnet::DecodeError;
    let (metadata, network) = rusthour_import::rhnet::decode(bytes).map_err(|e| match e {
        DecodeError::UnsupportedSchemaVersion { found, supported }
        | DecodeError::UnsupportedContainerVersion { found, supported } => {
            let what = if matches!(e, DecodeError::UnsupportedSchemaVersion { .. }) {
                "schema"
            } else {
                "container version"
            };
            let (age, next) = if found > supported {
                ("newer", NEWER_NEXT_STEP)
            } else {
                ("older", OLDER_NEXT_STEP)
            };
            format!(
                "This map is {age} than this viewer: it uses map {what} {found}, and this \
                 viewer reads {what} {supported}. {next}"
            )
        }
        _ => format!(
            "This is not a Rust Hour map, or it is damaged. {DECODE_NEXT_STEP}\n\nDetails: {e}."
        ),
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

    /// M2 trial (docs/trials/M2.md, UX finding 7): a map of another schema
    /// read "not a Rust Hour map, or damaged".
    #[test]
    fn other_schema_says_which_and_what_to_do() {
        let v = rusthour_api::NETWORK_SCHEMA_VERSION;
        let open = |schema: u32| {
            let mut bytes = rusthour_import::rhnet::encode(
                &rusthour_import::rhnet::Metadata::default(),
                &rusthour_api::Network::default(),
            );
            bytes[12..16].copy_from_slice(&schema.to_le_bytes());
            from_rhnet(&bytes).err().unwrap()
        };
        for (schema, word) in [(v - 1, "older"), (v + 1, "newer")] {
            let e = open(schema);
            assert!(!e.contains("damaged"), "{e}");
            assert!(e.contains(word), "{e}");
            assert!(e.contains(&format!("schema {schema}")), "{e}");
            assert!(e.contains(&format!("reads schema {v}")), "{e}");
        }
        assert!(open(v - 1).contains(OLDER_NEXT_STEP));
        assert!(open(v + 1).contains(NEWER_NEXT_STEP));
    }

    #[test]
    fn bad_bytes_give_the_decoders_message() {
        let e = from_rhnet(b"<!doctype html>").err().unwrap();
        assert!(e.contains("web page"), "{e}");
    }
}
