//! `rusthour` — the Rust Hour command line.

use std::path::{Path, PathBuf};

use clap::{CommandFactory as _, Parser, Subcommand, error::ErrorKind};
use rusthour_api::{Config, DriverMix, FleetMix, InProcess, Network, Simulation};

/// Rust Hour traffic simulator, headless mode.
#[derive(Debug, Parser)]
#[command(name = "rusthour", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a simulation for a number of steps and print its status as JSON.
    ///
    /// `removed_per_arrival` is null when no vehicle arrived.
    Run {
        /// Random seed; the same seed always gives the same result.
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Number of fixed timesteps to simulate.
        #[arg(long, default_value_t = 1000)]
        steps: u64,
        /// Timestep length in milliseconds, 1 to 1000; the driver model is
        /// tuned for 100.
        #[arg(long, default_value_t = Config::default().dt_ms,
              value_parser = clap::value_parser!(u32).range(1..=1000))]
        dt_ms: u32,
        /// Vehicles to keep on the network (needs --map).
        #[arg(long, default_value_t = 0)]
        vehicles: u32,
        /// Road network to run on, a `.rhnet` file; default is an empty map.
        #[arg(long)]
        map: Option<PathBuf>,
        /// Add `stuck_roads` (the ten roads with the most stuck removals) and
        /// `stuck_roads_total` (how many roads had any).
        #[arg(long)]
        report_stuck: bool,
        /// Share of each driver profile; names left out get 0 and only the
        /// ratios matter [default: aggressive=15,normal=60,cautious=20,erratic=5]
        #[arg(long, value_name = "NAME=SHARE,...")]
        drivers: Option<DriverMix>,
        /// Share of each vehicle class, as for --drivers
        /// [default: car=85,van=8,truck=5,bus=2]
        #[arg(long, value_name = "NAME=SHARE,...")]
        fleet: Option<FleetMix>,
    },
}

fn main() {
    match Cli::parse().command {
        Command::Run {
            seed,
            steps,
            dt_ms,
            vehicles,
            map,
            report_stuck,
            drivers,
            fleet,
        } => {
            // Not clap's `requires`: its usage line lists every option given,
            // which made `--steps` look required.
            if vehicles > 0 && map.is_none() {
                let mut cmd = Cli::command();
                cmd.build();
                cmd.find_subcommand_mut("run")
                    .expect("defined above")
                    .error(
                        ErrorKind::MissingRequiredArgument,
                        "--vehicles needs a road network: add --map <file.rhnet>",
                    )
                    .exit();
            }
            // Parsing validated both mixes, so they normalise.
            let drivers = drivers.unwrap_or_default().normalised().expect("validated");
            let fleet = fleet.unwrap_or_default().normalised().expect("validated");
            let config = Config {
                dt_ms,
                target_vehicles: vehicles,
                profile_mix: drivers.weights().expect("validated"),
                class_mix: fleet.weights().expect("validated"),
                ..Config::default()
            };
            let mut sim = match map.map(|path| load(&path)) {
                None => InProcess::new(config, seed),
                Some(Ok(network)) => InProcess::with_network(config, seed, network)
                    .unwrap_or_else(|err| fail(&format!("invalid map: {err}"))),
                Some(Err(msg)) => fail(&msg),
            };
            sim.step(steps);
            // The hash is on demand, not part of `Status`; keep it in the output.
            let mut out = serde_json::to_value(sim.status()).expect("status is plain data");
            let status = sim.status();
            // null, not 0, when nothing arrived: a full gridlock must not look healthy.
            out["removed_per_arrival"] = (status.arrived > 0)
                .then(|| f64::from(status.removed_stuck) / f64::from(status.arrived))
                .into();
            if report_stuck {
                let (top, total) = stuck_roads(&sim);
                out["stuck_roads"] = top.into();
                out["stuck_roads_total"] = total.into();
            }
            out["drivers"] = serde_json::to_value(drivers).expect("plain data");
            out["fleet"] = serde_json::to_value(fleet).expect("plain data");
            out["state_hash"] = sim.state_hash().into();
            match serde_json::to_string_pretty(&out) {
                Ok(json) => println!("{json}"),
                Err(err) => fail(&format!("could not serialize status: {err}")),
            }
        }
    }
}

/// The ten roads with the most stuck removals. Name and OSM way tell roads
/// apart where many have no name or share one.
fn stuck_roads(sim: &impl Simulation) -> (Vec<serde_json::Value>, usize) {
    let mut counts = Vec::new();
    sim.stuck_by_road_into(&mut counts);
    counts.sort_by_key(|&(road, n)| (std::cmp::Reverse(n), road.0));
    let top = counts
        .iter()
        .take(10)
        .map(|&(id, removed)| {
            let road = &sim.network().roads[id.0 as usize];
            serde_json::json!({
                "road": id.0,
                "name": road.name,
                "osm_way": road.osm.map(|o| o.way),
                "roundabout": road.roundabout,
                "removed": removed,
            })
        })
        .collect();
    (top, counts.len())
}

fn load(path: &Path) -> Result<Network, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let (_, network) = rusthour_import::rhnet::decode(&bytes)
        .map_err(|e| format!("{} is not a usable map: {e}", path.display()))?;
    Ok(network)
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}
