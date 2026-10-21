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

# Open the viewer (a demo town grid until it loads real maps, M1.8)
cargo run -p rusthour-viz --release

# Import a map (see "Importing maps" below)
cargo run -p rusthour-import --features osm -- bensheim-5km \
    --input bensheim-5km.osm.pbf --output bensheim-5km.rhnet

# Tests
cargo nextest run --workspace --all-features   # or: cargo test --workspace --all-features
```

## Viewer

Natively, `cargo run -p rusthour-viz --release` opens a window. On Linux it
needs a running X11 or Wayland session; no extra system packages are needed
to build it.

In the browser, build with [Trunk](https://trunkrs.dev)
(`cargo install --locked trunk`):

```sh
cd crates/rusthour-viz
trunk serve --release   # then open http://127.0.0.1:8080
trunk build --release   # static site in crates/rusthour-viz/dist/
```

The browser needs WebGPU or WebGL2 (any current Firefox, Chrome, Edge or
Safari).

Controls: drag to pan, scroll or pinch to zoom, arrow keys or WASD to pan,
`+`/`-` to zoom, `Home`, `F` or `0` to fit the map, `?` for the list of
shortcuts.

The viewer embeds egui's default fonts. Their licenses (SIL OFL 1.1, Ubuntu
Font Licence 1.0, MIT, Bitstream Vera) are in
[`crates/rusthour-viz/licenses/`](crates/rusthour-viz/licenses/) and must
accompany every copy of the viewer; the web build copies them into `dist/`.

## Map data

Sample maps (the areas in [`data/areas.toml`](data/areas.toml)) are not
committed. They are built by the **assets** workflow: on GitHub, open
*Actions → assets → Run workflow* and optionally list area names
(comma-separated; empty builds all). It cuts each area from Geofabrik's Hesse
extract and publishes the files as an `assets-<date>` pre-release on the
[Releases](https://github.com/jkorinth/rusthour/releases) page, then opens a
PR that records the build in [`data/assets.toml`](data/assets.toml).

Map data © OpenStreetMap contributors, available under the
[ODbL](https://www.openstreetmap.org/copyright); see
[`data/LICENSE`](data/LICENSE).

### Importing an area yourself

Most users won't need this: the assets workflow above publishes ready-cut
extracts, and will publish `.rhnet` maps (ROADMAP M1.7). To import an area from
`data/areas.toml` yourself, use one of those extracts or cut it from a regional
[Geofabrik](https://download.geofabrik.de) extract with
[osmium](https://osmcode.org/osmium-tool/). `areas.toml` stores the bbox as
`[south, west, north, east]`; osmium wants `west,south,east,north`:

```sh
# bensheim-5km: bbox = [49.6585, 8.5883, 49.7035, 8.6577]
osmium extract -b 8.5883,49.6585,8.6577,49.7035 --strategy complete_ways \
    hessen-latest.osm.pbf -o bensheim-5km.osm.pbf
cargo run -p rusthour-import --features osm -- bensheim-5km \
    --input bensheim-5km.osm.pbf --output bensheim-5km.rhnet
```

The importer stores the OpenStreetMap attribution in every `.rhnet` file;
anything that shows the map must display it.

## Crates

| Crate | Role |
| --- | --- |
| `rusthour-core` | Deterministic simulation engine; no I/O; builds for wasm32 |
| `rusthour-api` | The standard interface (`Simulation` facade, protocol types); later also a remote service |
| `rusthour-viz` | Interactive viewer, native and browser |
| `rusthour-cli` | `rusthour` binary: headless runs, statistics, comparisons |
| `rusthour-import` | Map assets: the `.rhnet` format (all targets, including wasm32); with feature `osm`, the native-only OSM importer and its `rusthour-import` binary ([ADR-0002](docs/adr/0002-map-asset-pipeline.md)) |

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
