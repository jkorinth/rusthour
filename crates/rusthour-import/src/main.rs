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

    let network = osm::import(&data, bbox);
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

    let length_m: f64 = network
        .roads
        .iter()
        .flat_map(|r| r.geometry.windows(2))
        .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
        .sum();
    let source = match data.replication_timestamp {
        Some(_) => format!("OSM data of {}", metadata.source_date),
        None => "OSM data date unknown".to_owned(),
    };
    println!(
        "{}: {} nodes, {} directed roads ({:.1} km, both directions counted), {} connections, \
         {source}\nwrote {}",
        args.area,
        network.nodes.len(),
        network.roads.len(),
        length_m / 1000.0,
        network.connections.len(),
        output.display()
    );
    Ok(())
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
    fn iso_dates() {
        assert_eq!(super::iso_date(0), "1970-01-01");
        assert_eq!(super::iso_date(951_782_400), "2000-02-29");
        assert_eq!(super::iso_date(1_790_812_800), "2026-10-01");
    }
}
