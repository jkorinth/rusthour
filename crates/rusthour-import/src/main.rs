//! `rusthour-import`: turns an OSM extract into a `.rhnet` map asset.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use rusthour_api::{Config, InProcess};
use rusthour_import::osm::{self, Bbox};
use rusthour_import::rhnet::{self, Metadata, OSM_ATTRIBUTION};

/// Import the drivable roads of an OSM extract, clipped to a named area, as a
/// `.rhnet` map.
#[derive(Parser)]
#[command(
    version,
    after_help = "Example:\n  rusthour-import bensheim-5km --input bensheim-5km.osm.pbf\n\n\
                  An existing output file is overwritten."
)]
struct Args {
    /// Area name from the areas file, e.g. `bensheim-5km`.
    area: String,
    /// OSM extract covering the area (`.osm.pbf`).
    #[arg(long, short)]
    input: PathBuf,
    /// Where to write the map [default: `<AREA>.rhnet`].
    #[arg(long, short)]
    output: Option<PathBuf>,
    /// The areas file.
    #[arg(long, default_value = "data/areas.toml")]
    areas: PathBuf,
    /// Write the map even if more than half of its roads are dropped (not
    /// connected to the road nearest the area's center).
    #[arg(long)]
    allow_drop: bool,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    let areas = std::fs::read_to_string(&args.areas).map_err(|e| {
        format!(
            "cannot read the areas file {}: {e}; run from the repository root or pass \
                 `--areas <file>`",
            args.areas.display()
        )
    })?;
    let bbox = Bbox::from_areas_toml(&areas, &args.area)
        .map_err(|e| format!("{}: {e}", args.areas.display()))?;
    let data = osm::read_pbf(&args.input).map_err(|e| {
        let hint = if matches!(e.kind(), osmpbf::ErrorKind::Io(_)) {
            ""
        } else {
            "; is it an .osm.pbf file? Convert other formats with `osmium cat <in> -o <out>.osm.pbf`"
        };
        format!("cannot read {}: {e}{hint}", args.input.display())
    })?;

    let (network, dropped) = osm::keep_central_part(osm::import(&data, bbox));
    if network.roads.is_empty() {
        let Bbox {
            south,
            west,
            north,
            east,
        } = bbox;
        return Err(format!(
            "no drivable roads of {} lie inside area `{}` (south {south}, west {west}, north \
             {north}, east {east}); does the extract cover it? Cut one from a regional \
             extract with `osmium extract -b {west},{south},{east},{north} -s complete_ways \
             <region>.osm.pbf -o {}.osm.pbf`",
            args.input.display(),
            args.area,
            args.area
        ));
    }
    if let Some(warning) = check_dropped(
        &args.area,
        network.roads.len(),
        dropped.roads,
        args.allow_drop,
    )? {
        eprintln!("warning: {warning}");
    }
    if let Err(e) = InProcess::with_network(Config::default(), 0, network.clone()) {
        return Err(format!(
            "the imported network is invalid ({e}); this is an importer bug, please report it"
        ));
    }

    let source_date = data.replication_timestamp.map_or_else(
        || {
            eprintln!(
                "warning: {} has no replication timestamp; recording the source date as `unknown`",
                args.input.display()
            );
            "unknown".to_owned()
        },
        iso_date,
    );
    let metadata = Metadata {
        area: args.area.clone(),
        source_date,
        importer_version: concat!("rusthour-import ", env!("CARGO_PKG_VERSION")).to_owned(),
        osm2streets_version: None,
        attribution: OSM_ATTRIBUTION.to_owned(),
    };
    let output = args
        .output
        .unwrap_or_else(|| PathBuf::from(format!("{}.rhnet", args.area)));
    std::fs::write(&output, rhnet::encode(&metadata, &network))
        .map_err(|e| format!("cannot write {}: {e}", output.display()))?;

    let length_m: f64 = network.roads.iter().map(osm::road_length_m).sum();
    let source = match data.replication_timestamp {
        Some(_) => format!("OSM data of {}", metadata.source_date),
        None => "OSM data date unknown".to_owned(),
    };
    println!(
        "{}: {} nodes, {} directed roads ({:.1} km, both directions counted), {} connections, \
         {source}\ndropped {} roads not connected to the area's center ({:.1} km)\nwrote {}",
        args.area,
        network.nodes.len(),
        network.roads.len(),
        length_m / 1000.0,
        network.connections.len(),
        dropped.roads,
        dropped.length_m / 1000.0,
        output.display()
    );
    Ok(())
}

/// Judges how much of the map [`osm::keep_central_part`] dropped: up to 5%
/// is normal (driveways and streets cut by the area edge), more is worth a
/// warning, more than half an error unless `allow`: the center likely sits
/// on an island.
fn check_dropped(
    area: &str,
    kept: usize,
    dropped: usize,
    allow: bool,
) -> Result<Option<String>, String> {
    let total = kept + dropped;
    let share = format!(
        "{dropped} of {total} roads ({}%) of area `{area}` are not connected to the road \
         nearest its center",
        dropped * 100 / total.max(1)
    );
    if dropped * 2 > total && !allow {
        Err(format!(
            "{share}, which may lie on an island; check the area's bbox in the areas file, \
             or pass `--allow-drop` to write the map anyway"
        ))
    } else if dropped * 20 > total {
        Ok(Some(format!(
            "{share} and were dropped; check that the area's bbox in the areas file \
             covers the streets you want"
        )))
    } else {
        Ok(None)
    }
}

/// `YYYY-MM-DD` (UTC) of a Unix timestamp in seconds.
fn iso_date(timestamp: i64) -> String {
    // Days to civil date, after Howard Hinnant's `civil_from_days`.
    let z = timestamp.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn dropping_much_of_the_map_warns_then_fails() {
        let check = |kept, dropped| super::check_dropped("x", kept, dropped, false);
        assert_eq!(check(100, 0), Ok(None));
        assert_eq!(check(95, 5), Ok(None));
        let warning = check(94, 6).unwrap().unwrap();
        assert!(warning.contains("6 of 100 roads (6%)"), "{warning}");
        assert!(check(50, 50).unwrap().is_some());
        let error = check(49, 51).unwrap_err();
        assert!(
            error.contains("51 of 100 roads (51%)") && error.contains("--allow-drop"),
            "{error}"
        );
        let allowed = super::check_dropped("x", 49, 51, true).unwrap().unwrap();
        assert!(allowed.contains("were dropped"), "{allowed}");
    }

    #[test]
    fn iso_dates() {
        assert_eq!(super::iso_date(0), "1970-01-01");
        assert_eq!(super::iso_date(951_782_400), "2000-02-29");
        assert_eq!(super::iso_date(1_790_812_800), "2026-10-01");
    }
}
