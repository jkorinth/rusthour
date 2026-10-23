# Rust Hour

*A traffic simulator where every hour is rush hour.*

Rust Hour simulates individual vehicles on real OpenStreetMap road networks,
with different vehicle types and driver personalities. Watch it interactively
in the browser or natively, or run deterministic batch simulations from the
command line and compare scenarios statistically.

> **Status:** milestone M1, a map viewer. Real OpenStreetMap areas load in
> an interactive viewer, in the browser
> ([try it](https://jkorinth.github.io/rusthour/)) or natively, and an
> importer builds the maps. There is no traffic yet: vehicles arrive with M2.
> See [ROADMAP.md](ROADMAP.md) for what's planned and
> [AGENTS.md](AGENTS.md) for how work is done.

## Quick start

Requires [rustup](https://rustup.rs). The toolchain version and the wasm
target are pinned in `rust-toolchain.toml` and installed automatically. The
first build takes several minutes and a few GB of disk.

Maps aren't in the repository: download a `.rhnet` file from the newest
`assets-<date>` pre-release on the
[Releases](https://github.com/jkorinth/rusthour/releases) page (named
`<area>.<sha256>.rhnet`), or import one yourself (see "Map data" below).

```sh
# Step an empty simulation (no map or vehicles yet) and print its status as
# JSON: a smoke test of the simulation core
cargo run -p rusthour-cli -- run --seed 42 --steps 1000

# Open the viewer on a downloaded or imported map (without --map: a demo town
# grid)
cargo run -p rusthour-viz --release -- --map bensheim-5km.rhnet

# Import a map, from the repository root (see "Importing an area yourself")
cargo run -p rusthour-import --features osm -- bensheim-5km \
    --input bensheim-5km.osm.pbf --output bensheim-5km.rhnet

# Tests
cargo nextest run --workspace --all-features   # or: cargo test --workspace --all-features
```

## Viewer

Natively, `cargo run -p rusthour-viz --release -- --map <file.rhnet>` opens a
window showing that map (see "Importing an area yourself" below); without
`--map` it shows a demo town grid. On Linux it needs a running X11 or Wayland
session; no extra system packages are needed to build it.

In the browser, build with [Trunk](https://trunkrs.dev)
(`cargo install --locked trunk`):

```sh
cd crates/rusthour-viz
trunk serve --release   # then open http://127.0.0.1:8080
trunk build --release   # static site in crates/rusthour-viz/dist/
```

The browser needs WebGPU or WebGL2 (any current Firefox, Chrome, Edge or
Safari). It loads the maps listed in `maps/manifest.json` next to the page:
`bensheim-5km` by default; pick another in the map selector at the top
left, or link to one with `?map=<area>` (e.g. `?map=bensheim-1km`, also on
the site's version page). Without that list (as with a plain `trunk serve`), or for
an unknown area, it shows the demo town grid and says why.

Controls: drag to pan (left button, Shift + left or middle button; one finger on
touch screens), scroll or pinch to zoom, arrow keys or WASD to pan,
`+`/`-` to zoom, `Home`, `F` or `0` to fit the map, `?` for the list of
shortcuts. `F3` shows frame times (median and 95th percentile over the last
10 s), the startup time, the cost of a simulation step and the map's size.

Traffic runs as soon as a map is shown, about one vehicle per 50 m of road
(change the number in the controls; that starts over). `Space` plays or
pauses, `.` (or "Step") advances one 0.1 s step. The speed slider goes from
0.1× to 1000× real time; when the computer can't keep up, the readout shows
the speed reached, e.g. "100× (running at 37×)". "Reset" starts over with the
same seed, so the same traffic plays again. Below are the simulated time and
the vehicles driving and waiting to enter the network.

The viewer embeds egui's default fonts. Their licenses (SIL OFL 1.1, Ubuntu
Font Licence 1.0, MIT, Bitstream Vera) are in
[`crates/rusthour-viz/licenses/`](crates/rusthour-viz/licenses/) and must
accompany every copy of the viewer; the web build copies them into `dist/`.

### Web viewer

The viewer is online at <https://jkorinth.github.io/rusthour/>, with a
dropdown to switch versions: `main (development)`, the unreleased build redeployed on every push to
`main`, and every milestone release after v0.1.0 (each release attaches its
build as `rusthour-web-<tag>.zip`). Links are shareable (`?v=v0.2.0`), and each
version also works on its own, e.g.
<https://jkorinth.github.io/rusthour/main/> or `…/rusthour/v0.2.0/`.
One-time repository setting: Settings → Pages → Source: "GitHub Actions".

If a release's web build fails, re-run the `web-build` job of its
release-please run (or build it and `gh release upload <tag> <zip> --clobber`
by hand), then Actions → Pages → Run workflow. Every deploy unpacks all
releases; GitHub Pages caps a site at 1 GB.

To try a pull request's web build before it merges, download the
`rusthour-web-<sha>` artifact from the PR's CI run (kept 7 days), unzip it
and serve the folder: `python3 -m http.server`, then open
<http://localhost:8000>. Opening `index.html` directly does not work, as
browsers won't load WebAssembly from `file://`.

## Map data

Sample maps (the areas in [`data/areas.toml`](data/areas.toml)) are not
committed. They are built by the **assets** workflow: on GitHub, open
*Actions → assets → Run workflow* and optionally list area names
(comma-separated; empty builds all). It cuts each area from Geofabrik's Hesse
extract, imports each as a `.rhnet` map and publishes both as an
`assets-<date>` pre-release on the
[Releases](https://github.com/jkorinth/rusthour/releases) page, then opens a
PR that records the build in [`data/assets.toml`](data/assets.toml). PRs
opened by a workflow don't start CI: close and reopen that PR to run the
checks.

The browser viewer gets its maps from the GitHub Pages site: every deploy
bundles, per area, the newest recorded map into each version's `maps/`
folder (so a run for some areas keeps the others), and each release's web
build keeps the maps of its own commit. Maps are rebuilt
by running the assets workflow again (merge its PR, then rerun the Pages
workflow). That is needed after a `NETWORK_SCHEMA_VERSION` bump: the Pages
deploy skips maps of another schema, and the viewer then shows its demo grid.
A map whose release was deleted is skipped with a warning; a map whose
SHA-256 doesn't match fails the deploy.

Map data © OpenStreetMap contributors, available under the
[ODbL](https://www.openstreetmap.org/copyright); see
[`data/LICENSE`](data/LICENSE).

### Importing an area yourself

Most users won't need this: the assets workflow above publishes ready-cut
extracts and `.rhnet` maps. To import an area from
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

The importer keeps only the part of the network connected to the road
nearest the area's center, and reports what it dropped: streets whose links
lie outside the area form islands no trip can reach. Dropping more than 5% of
the roads gives a warning; more than half is an error (the center may lie on
an island) unless you pass `--allow-drop`.

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
