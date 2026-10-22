//! The browser's map list, `maps/manifest.json`, written next to each
//! deployed viewer by the Pages deploy (M1.7):
//! `[{"area", "file", "sha256", "bytes", "source_date"}, …]`, possibly empty.

use serde::Deserialize;

/// The map shown when the page names none.
pub const DEFAULT_AREA: &str = "bensheim-5km";

/// One map in `maps/manifest.json`. `sha256` is not checked here: the
/// `.rhnet` decoder verifies the file's own hash.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManifestEntry {
    /// Area name, e.g. `bensheim-5km`.
    pub area: String,
    /// File name next to the manifest, e.g. `bensheim-5km.<sha256>.rhnet`.
    pub file: String,
    /// File size, for the progress bar.
    pub bytes: u64,
}

/// Parses `maps/manifest.json`. Each `file` must be `<area>.<sha256>.rhnet`,
/// so a damaged list can't point the viewer anywhere else.
pub fn parse_manifest(json: &str) -> Result<Vec<ManifestEntry>, String> {
    let damaged = || "The map list on this site is damaged.".to_owned();
    let entries: Vec<ManifestEntry> = serde_json::from_str(json).map_err(|_| damaged())?;
    let valid = |e: &ManifestEntry| {
        let area_ok = !e.area.is_empty()
            && e.area
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        let hash = e
            .file
            .strip_prefix(&e.area)
            .and_then(|f| f.strip_prefix('.'))
            .and_then(|f| f.strip_suffix(".rhnet"));
        area_ok && hash.is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    if entries.iter().all(valid) {
        Ok(entries)
    } else {
        Err(damaged())
    }
}

/// The `map` parameter of a page's query string (`?map=bensheim-2km`).
// shortcut: no percent-decoding; area names are plain ASCII (data/areas.toml).
pub fn requested_area(search: &str) -> Option<&str> {
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|kv| kv.strip_prefix("map="))
        .filter(|a| !a.is_empty())
}

/// Picks the map to load: the requested area, else [`DEFAULT_AREA`], else the
/// first one. `Err` says, for the user, why the demo network is shown instead.
pub fn choose<'a>(
    manifest: &'a [ManifestEntry],
    requested: Option<&str>,
) -> Result<&'a ManifestEntry, String> {
    let find = |area: &str| manifest.iter().find(|e| e.area == area);
    match requested {
        Some(area) => find(area).ok_or_else(|| {
            let known: Vec<&str> = manifest.iter().map(|e| e.area.as_str()).collect();
            if known.is_empty() {
                format!("There is no map “{area}” here; no maps are published yet.")
            } else {
                format!(
                    "There is no map “{area}” here. Available: {}.",
                    known.join(", ")
                )
            }
        }),
        None => find(DEFAULT_AREA)
            .or(manifest.first())
            .ok_or_else(|| "No maps are published here yet.".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"[
        {"area": "bensheim-1km", "file": "bensheim-1km.abababababababababababababababababababababababababababababababab.rhnet", "sha256": "abababababababababababababababababababababababababababababababab",
         "bytes": 1000, "source_date": "2026-10-01"},
        {"area": "bensheim-5km", "file": "bensheim-5km.cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd.rhnet", "sha256": "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
         "bytes": 250000, "source_date": "2026-10-01"}
    ]"#;

    #[test]
    fn manifest_picks_requested_then_default_then_first() {
        let m = parse_manifest(MANIFEST).unwrap();
        assert!(m[1].file.starts_with("bensheim-5km.cdcd"));
        assert_eq!(m[1].bytes, 250_000);
        assert_eq!(choose(&m, None).unwrap().area, "bensheim-5km");
        assert_eq!(
            choose(&m, Some("bensheim-1km")).unwrap().area,
            "bensheim-1km"
        );
        assert_eq!(choose(&m[..1], None).unwrap().area, "bensheim-1km");

        assert_eq!(
            requested_area("?map=bensheim-1km&x=1"),
            Some("bensheim-1km")
        );
        assert_eq!(requested_area("?x=1&map=a"), Some("a"));
        assert_eq!(requested_area(""), None);
        assert_eq!(requested_area("?map="), None);
    }

    #[test]
    fn empty_manifest_or_unknown_area_falls_back_with_a_note() {
        let empty = parse_manifest("[]").unwrap();
        assert!(choose(&empty, None).unwrap_err().contains("No maps"));
        let m = parse_manifest(MANIFEST).unwrap();
        let note = choose(&m, Some("atlantis")).unwrap_err();
        assert!(
            note.contains("atlantis") && note.contains("bensheim-5km"),
            "{note}"
        );
        assert!(parse_manifest("<html>404</html>").is_err());
        for file in [
            "../x.rhnet",
            "bensheim-5km.rhnet",
            "https://evil.example/x.rhnet",
        ] {
            let json = format!(r#"[{{"area": "bensheim-5km", "file": "{file}", "bytes": 1}}]"#);
            assert!(parse_manifest(&json).is_err(), "{file}");
        }
    }
}
