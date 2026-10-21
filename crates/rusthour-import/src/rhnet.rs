//! The `.rhnet` map asset format (ADR-0002).
//!
//! A `.rhnet` file is a fixed little-endian header, the metadata and the
//! compressed network:
//!
//! | Offset | Size | Field |
//! | --- | --- | --- |
//! | 0 | 8 | Magic bytes `RHNET\0\0\0` |
//! | 8 | 4 | Container version, [`CONTAINER_VERSION`] (`u32`) |
//! | 12 | 4 | Network schema version, [`NETWORK_SCHEMA_VERSION`] (`u32`) |
//! | 16 | 4 | Metadata length in bytes (`u32`) |
//! | 20 | 4 | Payload length in bytes (`u32`) |
//! | 24 | 4 | Uncompressed payload length in bytes (`u32`) |
//! | 28 | 32 | SHA-256 of bytes `0..28` followed by bytes `60..` |
//! | 60 | … | [`Metadata`], postcard-encoded |
//! | … | … | Payload: the [`Network`], postcard-encoded, then LZ4-compressed |
//!
//! The hash field itself is left out of the SHA-256, not zeroed. The hash
//! detects corruption and is the content hash in cache names
//! (`<area>.<hash>.rhnet`, all 32 bytes as 64 lowercase hex digits). It covers
//! the header and metadata as well as the payload, so a changed attribution
//! also gets a new name.
//!
//! Both encoded parts use postcard's
//! [wire format](https://postcard.jamesmunns.com/wire-format): struct fields
//! in declaration order, strings as a varint length plus UTF-8, `Option` as a
//! 0/1 tag. Adding or reordering [`Metadata`] fields therefore bumps
//! [`CONTAINER_VERSION`]. The payload is the LZ4 *block* format without a size
//! prefix, not the frame format the `lz4` command line tool writes.
//!
//! Crate choices, all pure Rust and permissively licensed, so decoding works on
//! `wasm32-unknown-unknown`: [postcard](https://docs.rs/postcard) (compact,
//! deterministic serde format; f64 is stored as its raw bits),
//! [`lz4_flex`](https://docs.rs/lz4_flex) (no dependencies, safe-code mode,
//! fast decode) and [sha2](https://docs.rs/sha2).

use std::fmt;

use rusthour_api::{NETWORK_SCHEMA_VERSION, Network};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Version of the `.rhnet` container layout. Bump on any change to the header,
/// [`Metadata`], serialization or compression.
pub const CONTAINER_VERSION: u32 = 1;

/// The attribution that assets made from OSM data must carry under its license.
pub const OSM_ATTRIBUTION: &str = "© OpenStreetMap contributors";

const MAGIC: [u8; 8] = *b"RHNET\0\0\0";
const HASH: std::ops::Range<usize> = 28..60;
const HEADER_LEN: usize = HASH.end;
/// LZ4 expands at most ~255×; a larger claimed size is corrupt, and capping it
/// keeps a crafted header from triggering a huge allocation.
const MAX_LZ4_RATIO: usize = 256;
/// Real networks are far smaller; a larger claimed size is rejected before
/// allocating, so a crafted file can't make the browser allocate gigabytes.
const MAX_NETWORK_BYTES: usize = 256 << 20;

/// Where an asset comes from, stored right after the fixed header.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    /// The area's name in `data/areas.toml`, e.g. `bensheim-5km`.
    pub area: String,
    /// Date of the OSM extract as an ISO 8601 date, `YYYY-MM-DD`.
    pub source_date: String,
    /// Version of the importer that wrote the asset.
    pub importer_version: String,
    /// Version of `osm2streets` used, if any.
    pub osm2streets_version: Option<String>,
    /// Data attribution, shown verbatim alongside the map. Never empty for
    /// maps made from OSM data, which carry [`OSM_ATTRIBUTION`].
    pub attribution: String,
}

/// Why bytes could not be decoded as a `.rhnet` asset.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The bytes don't start with the `.rhnet` magic bytes.
    NotRhnet,
    /// The bytes look like an HTML page, e.g. a web server's error page.
    WebPage,
    /// The input ends before the fixed header does.
    TooShort {
        /// Actual length.
        actual: usize,
    },
    /// The container layout is from a different version.
    UnsupportedContainerVersion {
        /// Version in the file.
        found: u32,
        /// The only version this build reads.
        supported: u32,
    },
    /// The network types are from a different schema version.
    UnsupportedSchemaVersion {
        /// Version in the file.
        found: u32,
        /// The only version this build reads.
        supported: u32,
    },
    /// The file is shorter or longer than its header says it is.
    WrongLength {
        /// Length according to the header.
        expected: u64,
        /// Actual length.
        actual: usize,
    },
    /// The content doesn't match its SHA-256: the file is corrupt.
    HashMismatch,
    /// The hash matches but the content can't be decoded; names the part.
    Malformed(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mismatch = |f: &mut fmt::Formatter<'_>, what, found: u32, supported| {
            if found > supported {
                write!(
                    f,
                    "this map uses {what} version {found}, but this version of Rust Hour only \
                     reads version {supported}; update Rust Hour (in the browser, reload the \
                     page) to open it"
                )
            } else {
                write!(
                    f,
                    "this map uses {what} version {found}, which this version of Rust Hour no \
                     longer reads (it reads version {supported}); re-run the importer to update \
                     the map"
                )
            }
        };
        match *self {
            Self::NotRhnet => write!(f, "not a .rhnet map file (wrong magic bytes)"),
            Self::WebPage => write!(
                f,
                "this looks like a web page, not a .rhnet map file; check the map URL"
            ),
            Self::TooShort { actual: 0 } => {
                write!(f, "the map file is empty; check the map URL or path")
            }
            Self::TooShort { actual } => write!(
                f,
                "the .rhnet file is {actual} bytes long, shorter than the {HEADER_LEN}-byte \
                 header; it is truncated or corrupt, download it again"
            ),
            Self::UnsupportedContainerVersion { found, supported } => {
                mismatch(f, ".rhnet container", found, supported)
            }
            Self::UnsupportedSchemaVersion { found, supported } => {
                mismatch(f, "network schema", found, supported)
            }
            Self::WrongLength { expected, actual } => write!(
                f,
                "the .rhnet file is {actual} bytes long but its header says {expected}; \
                 it is truncated or corrupt, download it again"
            ),
            Self::HashMismatch => write!(
                f,
                "the .rhnet file is corrupt (SHA-256 mismatch); download it again"
            ),
            Self::Malformed(part) => write!(
                f,
                "the .rhnet file has a malformed {part} although its checksum is valid; the \
                 importer that wrote it may be broken, re-run the importer or report a bug"
            ),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Encodes metadata and a network as a `.rhnet` asset.
///
/// The output depends only on the arguments, so the same metadata and network
/// always give byte-identical files (ADR-0002). The network is not validated;
/// [`InProcess::with_network`](rusthour_api::InProcess::with_network) does
/// that when it is loaded.
///
/// # Panics
///
/// If the metadata or the compressed network exceeds 4 GiB.
#[must_use]
pub fn encode(metadata: &Metadata, network: &Network) -> Vec<u8> {
    let raw = postcard::to_allocvec(network).expect("networks always serialize");
    let metadata = postcard::to_allocvec(metadata).expect("metadata always serializes");
    container(&metadata, &lz4_flex::compress(&raw), raw.len())
}

/// Assembles a file from already encoded parts.
fn container(metadata: &[u8], payload: &[u8], uncompressed_len: usize) -> Vec<u8> {
    let len = |n: usize| u32::try_from(n).expect(".rhnet parts are limited to 4 GiB");
    let mut out = Vec::with_capacity(HEADER_LEN + metadata.len() + payload.len());
    out.extend_from_slice(&MAGIC);
    for field in [
        CONTAINER_VERSION,
        NETWORK_SCHEMA_VERSION,
        len(metadata.len()),
        len(payload.len()),
        len(uncompressed_len),
    ] {
        out.extend_from_slice(&field.to_le_bytes());
    }
    out.extend_from_slice(&[0; 32]);
    out.extend_from_slice(metadata);
    out.extend_from_slice(payload);
    let hash = hash(&out);
    out[HASH].copy_from_slice(&hash);
    out
}

/// SHA-256 of a file, skipping its hash field.
fn hash(file: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(&file[..HASH.start])
        .chain_update(&file[HASH.end..])
        .finalize()
        .into()
}

/// Decodes a `.rhnet` asset.
///
/// The input is untrusted: any bytes give an error rather than a panic or a
/// wrong network. The network itself is not validated; pass it to
/// [`InProcess::with_network`](rusthour_api::InProcess::with_network).
///
/// # Example
///
/// Loading a map in a viewer:
///
/// ```
/// use rusthour_api::{Config, InProcess, Network};
/// use rusthour_import::rhnet::{self, Metadata, OSM_ATTRIBUTION};
///
/// # let metadata = Metadata {
/// #     area: "bensheim-5km".into(),
/// #     source_date: "2026-10-01".into(),
/// #     attribution: OSM_ATTRIBUTION.into(),
/// #     ..Metadata::default()
/// # };
/// # let bytes = rhnet::encode(&metadata, &Network::default());
/// let (metadata, network) = rhnet::decode(&bytes)?;
/// let sim = InProcess::with_network(Config::default(), 42, network)?;
/// println!("{} ({}), {}", metadata.area, metadata.source_date, metadata.attribution);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// See [`DecodeError`].
pub fn decode(bytes: &[u8]) -> Result<(Metadata, Network), DecodeError> {
    if bytes.get(..MAGIC.len()) != Some(&MAGIC[..]) {
        return Err(if MAGIC.starts_with(bytes) {
            DecodeError::TooShort {
                actual: bytes.len(),
            }
        } else if bytes.starts_with(b"<") {
            DecodeError::WebPage
        } else {
            DecodeError::NotRhnet
        });
    }
    let field = |i: usize| {
        let at = MAGIC.len() + 4 * i;
        Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    };
    // Versions first, so a newer file gets a version error, not a parse error.
    let short = DecodeError::TooShort {
        actual: bytes.len(),
    };
    let found = field(0).ok_or(short.clone())?;
    if found != CONTAINER_VERSION {
        return Err(DecodeError::UnsupportedContainerVersion {
            found,
            supported: CONTAINER_VERSION,
        });
    }
    let found = field(1).ok_or(short.clone())?;
    if found != NETWORK_SCHEMA_VERSION {
        return Err(DecodeError::UnsupportedSchemaVersion {
            found,
            supported: NETWORK_SCHEMA_VERSION,
        });
    }
    if bytes.len() < HEADER_LEN {
        return Err(short);
    }
    // Complete header, so the fields are present.
    let [metadata_len, payload_len, uncompressed_len] =
        [2, 3, 4].map(|i| field(i).unwrap_or_default() as usize);
    let expected = HEADER_LEN as u64 + metadata_len as u64 + payload_len as u64;
    if bytes.len() as u64 != expected {
        return Err(DecodeError::WrongLength {
            expected,
            actual: bytes.len(),
        });
    }
    if hash(bytes)[..] != bytes[HASH] {
        return Err(DecodeError::HashMismatch);
    }

    let (metadata, payload) = bytes[HEADER_LEN..].split_at(metadata_len);
    let metadata = from_postcard(metadata).ok_or(DecodeError::Malformed("metadata"))?;
    let compression = DecodeError::Malformed("compressed network");
    if uncompressed_len
        > payload_len
            .saturating_mul(MAX_LZ4_RATIO)
            .min(MAX_NETWORK_BYTES)
    {
        return Err(compression);
    }
    let raw = lz4_flex::decompress(payload, uncompressed_len).map_err(|_| compression.clone())?;
    if raw.len() != uncompressed_len {
        return Err(compression);
    }
    let network = from_postcard(&raw).ok_or(DecodeError::Malformed("network"))?;
    Ok((metadata, network))
}

/// Deserializes exactly `bytes`; trailing bytes are an error.
fn from_postcard<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Option<T> {
    match postcard::take_from_bytes(bytes) {
        Ok((value, [])) => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Arbitrary bytes, and arbitrary content behind a valid header and
        /// hash (anyone can compute one), never panic.
        #[test]
        fn decode_never_panics(
            bytes in prop::collection::vec(any::<u8>(), 0..300),
            metadata_len in 0..300usize,
            uncompressed_len in prop_oneof![
                0..2000usize,
                Just(MAX_NETWORK_BYTES + 1),
                Just(u32::MAX as usize),
            ],
        ) {
            let _ = decode(&bytes);
            let (metadata, payload) = bytes.split_at(metadata_len.min(bytes.len()));
            let _ = decode(&container(metadata, payload, uncompressed_len));
        }
    }
}
