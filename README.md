# Rust Hour

*A traffic simulator where every hour is rush hour.*

Rust Hour simulates individual vehicles on real OpenStreetMap road networks,
with different vehicle types and driver personalities. Watch it interactively
in the browser or natively, or run deterministic batch simulations from the
command line and compare scenarios statistically.

> **Status:** early scaffolding (milestone M0). See [ROADMAP.md](ROADMAP.md)
> for what's planned and [AGENTS.md](AGENTS.md) for how work is done.

## Quick start

Requires [rustup](https://rustup.rs). The toolchain version and the wasm
target are pinned in `rust-toolchain.toml` and installed automatically.

```sh
# Run a headless simulation and print its status as JSON
cargo run -p rusthour-cli -- run --seed 42 --steps 1000

# Run the (placeholder) viewer
cargo run -p rusthour-viz

# Tests
cargo nextest run --workspace     # or: cargo test --workspace
```

## Crates

| Crate | Role |
| --- | --- |
| `rusthour-core` | Deterministic simulation engine; no I/O; builds for wasm32 |
| `rusthour-api` | The standard interface (`Simulation` facade, protocol types); later also a remote service |
| `rusthour-viz` | Interactive viewer, native and browser |
| `rusthour-cli` | `rusthour` binary: headless runs, statistics, comparisons |

Frontends depend only on `rusthour-api`, never on `rusthour-core` directly
(see [ADR-0001](docs/adr/0001-crate-split.md)).

## Development

Install the local tools once:

```sh
cargo install --locked cargo-nextest cargo-deny cargo-shear typos-cli taplo-cli
# plus lefthook (https://lefthook.dev), then:
lefthook install
```

Hooks check formatting, clippy, spelling, TOML formatting and the
Conventional Commit message format. CI additionally runs cargo-deny,
cargo-shear, markdownlint, actionlint, zizmor and a wasm32 build.

All commits follow [Conventional Commits](https://www.conventionalcommits.org);
releases are cut per milestone by
[release-please](https://github.com/googleapis/release-please).

## License

[GPL-3.0](LICENSE)
