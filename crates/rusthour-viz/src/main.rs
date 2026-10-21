//! Rust Hour interactive viewer.
//!
//! Placeholder until the rendering framework is chosen (ROADMAP M1.1).

use rusthour_api::{Config, InProcess, Simulation};

fn main() {
    let mut sim = InProcess::new(Config::default(), 42);
    sim.step(1);
    let status = sim.status();
    println!(
        "rusthour-viz placeholder: tick {} (protocol v{})",
        status.tick, status.protocol_version
    );
}
