//! `rusthour` — the Rust Hour command line.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use rusthour_api::{Config, InProcess, Network, Simulation};

/// Rust Hour traffic simulator, headless mode.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a simulation for a number of steps and print its status as JSON.
    Run {
        /// Random seed; the same seed always gives the same result.
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Number of fixed timesteps to simulate.
        #[arg(long, default_value_t = 1000)]
        steps: u64,
        /// Timestep length in milliseconds.
        #[arg(long, default_value_t = Config::default().dt_ms)]
        dt_ms: u32,
        /// Vehicles to keep on the network (needs --map).
        #[arg(long, default_value_t = 0, requires = "map")]
        vehicles: u32,
        /// Road network to run on, a `.rhnet` file; default is an empty map.
        #[arg(long)]
        map: Option<PathBuf>,
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
        } => {
            let config = Config {
                dt_ms,
                target_vehicles: vehicles,
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
            out["state_hash"] = sim.state_hash().into();
            match serde_json::to_string_pretty(&out) {
                Ok(json) => println!("{json}"),
                Err(err) => fail(&format!("could not serialize status: {err}")),
            }
        }
    }
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
