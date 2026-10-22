//! `rusthour` — the Rust Hour command line.

use clap::{Parser, Subcommand};
use rusthour_api::{Config, InProcess, Simulation};

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
    },
}

fn main() {
    match Cli::parse().command {
        Command::Run { seed, steps, dt_ms } => {
            let mut sim = InProcess::new(
                Config {
                    dt_ms,
                    ..Config::default()
                },
                seed,
            );
            sim.step(steps);
            match serde_json::to_string_pretty(&sim.status()) {
                Ok(json) => println!("{json}"),
                Err(err) => {
                    eprintln!("error: could not serialize status: {err}");
                    std::process::exit(1);
                }
            }
        }
    }
}
