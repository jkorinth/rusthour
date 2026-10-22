# Rust Hour — Project Brief & Roadmap

*A traffic simulator where every hour is rush hour.*

> **This is the central planning document.** Agents claim, update and complete
> items here. Process rules live in [AGENTS.md](AGENTS.md).

## 1. Vision

A microscopic traffic simulator built on real OpenStreetMap road networks,
with two faces:

- **Interactive ("anthill") mode** — watch thousands of vehicles move through a
  city in the browser or natively; speed time up or down, change parameters
  live, edit intersections and see the effect immediately.
- **Analysis mode (CLI)** — run deterministic, multi-seed batch simulations of
  full days and compare scenarios statistically, e.g. *"replacing this
  crossroads with a roundabout changes mean travel time by X ± Y"*.

What makes it different from existing tools (e.g. A/B Street, which uses a
discrete-event model): **continuous car-following physics with heterogeneous
driver personalities**, explicit time-of-day/weekday demand, and rigorous A/B
statistics.

## 2. Core concepts

| Concept | Approach |
| --- | --- |
| Map | OSM → `osm2streets` → simplified lane-level network in our own compact format |
| Car-following | Intelligent Driver Model (IDM) |
| Lane changing | MOBIL (politeness factor per driver) |
| Drivers | Parameter profiles: aggressive, normal, cautious, erratic (noise on parameters) |
| Vehicles | Classes: car, van, truck, bus, (bike later) — length, accel, braking, top speed |
| Demand | Agents with activity chains (home → work → shop → home); departure times from time-of-day × weekday profiles; through-traffic via boundary/highway nodes |
| Routing | A* first, contraction hierarchies (`fast_paths`) when needed; periodic rerouting on observed travel times |
| Intersections | Pluggable controllers: priority/yield, stop, traffic signal, roundabout (gap acceptance) |
| Time | Fixed timestep (default 0.1 s, configurable), seeded RNG → fully deterministic |
| Stats | Travel time, delay, throughput (vehicles/day), queue lengths, per-edge flows |

## 3. Architecture

Cargo workspace, five crates:

```text
rusthour-core          pure simulation; no I/O, no rendering, no async;
   ▲                   compiles to wasm32
   │
rusthour-api           the standard interface: protocol types (serde),
   ▲                   `Simulation` facade, in-process client;
   │                   `server` feature runs core as a remote service
   │
   ├── rusthour-import map assets: `.rhnet` format read/write (all targets);
   │      ▲            `osm` feature + `rusthour-import` binary: OSM →
   │      │            network via osm2streets (native, offline)
   │      │ (format only)
   ├── rusthour-viz    interactive frontend (native + wasm)
   └── rusthour-cli    headless batch runs, statistics, scenario comparison
```

- `rusthour-viz` and `rusthour-cli` talk **only** to `rusthour-api`, never to `rusthour-core`
  directly. This guarantees anything the frontends can do, a remote client can
  do too. They may use `rusthour-import` to read map assets, but never its
  `osm` feature.
- Map data flow (ADR-0002): fetch an OSM extract for a named area in
  `data/areas.toml` → `rusthour-import` writes a compressed `.rhnet` asset →
  frontends load the bytes (file on native, HTTP in the browser, cached by
  content-hashed name) and pass them through `rusthour-api` to the core. The
  core never does I/O.
- The `server` feature of `rusthour-api` (HTTP + WebSocket) is native-only so the
  wasm build stays lean.
- `rusthour-core` modules are organised so features can be built in parallel:
  `network`, `routing`, `vehicle` (IDM kinematics), `behavior` (MOBIL, driver
  profiles), `control` (intersection controllers), `demand`, `scenario`,
  `stats`.

Architectural decisions are recorded as ADRs in `docs/adr/`.

## 4. Releases

Each milestone ships as one release via **release-please** (pre-1.0, minor
bump per milestone: M0 → `v0.1.0`, M1 → `v0.2.0`, …). The release PR is merged
only when the milestone's acceptance criteria and user trial are done.

## 5. How to use this roadmap

- Each item has an ID (`M<milestone>.<n>`), a status and an owner.
- Status: `todo` · `claimed` · `in-review` · `done` · `blocked`.
- Claim an item by setting status `claimed` and owner to your branch name in
  your first commit/PR. Edit **only your own item's line** to avoid merge
  conflicts.
- New work discovered along the way goes into **Backlog** (§7), not into an
  active milestone, unless the architect agrees.

## 6. Milestones

### M0 — Scaffolding → `v0.1.0`

Goal: an empty but fully wired project. Commits go straight to `main` during
M0; PRs are mandatory from the M0 release onward.

| ID | Item | Status | Owner |
| --- | --- | --- | --- |
| M0.1 | Cargo workspace with `rusthour-core`, `rusthour-api`, `rusthour-viz`, `rusthour-cli`; `rust-toolchain.toml`; workspace lints in root `Cargo.toml` | done | main |
| M0.2 | Lint tooling configured (see AGENTS.md §4): rustfmt, clippy, cargo-deny, cargo-shear, typos, taplo, markdownlint, actionlint, zizmor | done | main |
| M0.3 | Local hooks via lefthook (fmt, clippy, typos, commit-msg check) | done | main |
| M0.4 | CI: lint, test (cargo-nextest), wasm32 check for `rusthour-core` + `rusthour-viz`, dependency-direction check | done | main |
| M0.5 | Conventional commits enforced: PR-title and per-commit checks (rebase merge) + commit-msg hook | done | main |
| M0.6 | release-please (Rust, cargo-workspace plugin, pre-major settings) | done | main |
| M0.7 | PR template, issue templates (feature, bug/regression), CODEOWNERS-style role notes | done | main |
| M0.8 | Determinism harness: helper that runs N steps and returns a state hash | done | main |
| M0.9 | README: build, run, test; ADR-0001 (crate split & dependency rules) | done | main |

**Acceptance:** `main` is green; `cargo build` native and wasm32 succeed; a
test PR with a non-conventional title is rejected; release-please opens a
release PR.

### M1 — Map → `v0.2.0`

| ID | Item | Status | Owner |
| --- | --- | --- | --- |
| M1.1 | ADR: viz framework choice (Bevy vs macroquad vs eframe), decided by architect + UX | done | docs/M1.1-viz-framework |
| M1.2 | `rusthour-import` crate: OSM extract → network via our own `osmpbf`-based importer (lanes, intersections, speed limits; ADR-0002), behind the `osm` feature, with a `rusthour-import` binary taking an area name from `data/areas.toml`. Carry OSM source ids into the network types and asset (OSM node id for intersections; way id plus endpoint node ids for roads, since the importer splits ways) so M7.0 needs no format bump or re-import | done | feat/M1.2-osm-importer |
| M1.3 | `.rhnet` asset format in `rusthour-import` (header, versioning, pure-Rust compression), readable on wasm32 without OSM parsing | done | feat/M1.3-rhnet-format |
| M1.4 | `rusthour-api`: network data types, load a validated network into the simulation (bytes are decoded by `rusthour-import`, ADR-0002), query geometry for rendering | done | feat/M1.4-network-types |
| M1.5 | `rusthour-viz`: render network, pan/zoom, native + browser | done | feat/M1.5-map-viewer |
| M1.6 | Sample maps (see below): `assets` workflow publishes OSM extracts per area in `assets-*` pre-releases, recorded in `data/assets.toml` | done | feat/M1.6-asset-workflow |
| M1.7 | `assets` workflow builds `.rhnet` files with `rusthour-import` (after M1.2), and a deploy job publishes the web viewer plus assets to GitHub Pages (needed for the M1 user trial; release assets can't be fetched cross-origin) | done | feat/M1.7-rhnet-assets |
| M1.8 | `rusthour-viz` loads a Bensheim `.rhnet` map: loading progress, in-page errors for wasm/map load failures, OSM attribution text, startup-time check (3 s native / 5 s browser on bensheim-5km per ADR-0003), frame-time measurement per ADR-0003, street-name labels incl. ä/ö/ü/ß | todo | |

**Sample areas (M1.6):** named, square boxes in
[`data/areas.toml`](data/areas.toml), selected by name in the importer
(e.g. `--area bensheim-5km`). All three are centered on the Bensheim
Marktplatz (49.6810 N, 8.6230 E): `bensheim-1km` for quick tests,
`bensheim-2km`, and `bensheim-5km` as the default sample map. New areas are
added there, not here. Extracts and assets are built by the `assets`
workflow and published as `assets-*` GitHub pre-releases, not committed; the
release notes record the source, its date and the exact commands, and
[`data/assets.toml`](data/assets.toml) records each build (tag, source
timestamp, file hashes) so the import is reproducible. `.rhnet` assets and
their deployment next to the web viewer follow in M1.7. OSM data is ODbL:
keep the attribution "© OpenStreetMap contributors" with the data and in the
viewer.

**Acceptance:** sample map renders natively and in the browser; import is
reproducible from the OSM extract.

### M2 — First traffic → `v0.3.0`

| ID | Item | Status | Owner |
| --- | --- | --- | --- |
| M2.1 | Fixed-timestep simulation loop, seeded RNG, sim clock | todo | |
| M2.2 | Vehicle state as structure-of-arrays; per-lane sorted vehicle lists | todo | |
| M2.3 | IDM car-following | todo | |
| M2.4 | Simple routing (A*) and random origin–destination spawning | todo | |
| M2.5 | Basic intersection passage (priority rule only) | todo | |
| M2.6 | `rusthour-api`: step, run/pause, time-scale, vehicle snapshot stream | todo | |
| M2.7 | `rusthour-viz`: animated vehicles, play/pause, speed slider (0.1×–100×+) | todo | |
| M2.8 | Benchmark (criterion): vehicles × steps/s, tracked in CI (non-gating) | todo | |

**Acceptance:** thousands of vehicles run in real time in the browser; same
seed ⇒ identical state hash.

> After M2 the core loop exists and M3–M6 workstreams can proceed **in
> parallel**.

### M3 — Lanes & intersections → `v0.4.0`

| ID | Item | Status | Owner |
| --- | --- | --- | --- |
| M3.1 | MOBIL lane changing (incentive + safety criteria, politeness) | todo | |
| M3.2 | Intersection controller trait + priority/yield and stop controllers | todo | |
| M3.3 | Traffic signal controller (fixed-time plans) | todo | |
| M3.4 | Turn lanes and lane selection ahead of intersections | todo | |
| M3.5 | Gridlock detection and recovery strategy (ADR) | todo | |

### M4 — Drivers & vehicles → `v0.5.0`

| ID | Item | Status | Owner |
| --- | --- | --- | --- |
| M4.1 | Driver profiles: aggressive, normal, cautious, erratic (IDM + MOBIL parameters, noise) | todo | |
| M4.2 | Vehicle classes: car, van, truck, bus | todo | |
| M4.3 | Fleet/driver mix configurable per scenario | todo | |
| M4.4 | `rusthour-viz`: live parameter panel, color by driver/vehicle type, inspect a vehicle | todo | |

### M5 — Demand & scenarios → `v0.6.0`

| ID | Item | Status | Owner |
| --- | --- | --- | --- |
| M5.1 | Scenario file format (TOML): map, demand, fleet mix, duration, seed(s) | todo | |
| M5.2 | Agents with activity chains (home, work, shop, school, leisure) | todo | |
| M5.3 | Time-of-day × weekday departure profiles | todo | |
| M5.4 | Boundary/highway through-traffic and network exits | todo | |
| M5.5 | Rerouting on observed travel times | todo | |
| M5.6 | `rusthour-viz`: clock & day display, jump to time, scenario picker | todo | |

### M6 — Statistics & batch CLI → `v0.7.0`

| ID | Item | Status | Owner |
| --- | --- | --- | --- |
| M6.1 | Metrics collection in `stats` (travel time, delay, throughput, queues, edge flows) with minimal hot-loop overhead | todo | |
| M6.2 | `rusthour-cli run`: headless, multi-seed, parallel (rayon) | todo | |
| M6.3 | Output: CSV and Parquet, plus summary JSON | todo | |
| M6.4 | `rusthour-cli compare A B`: means, confidence intervals, effect sizes | todo | |
| M6.5 | `rusthour-viz`: live stats overlay (heatmap of delay/flow) | todo | |

### M7 — Network editing & A/B studies → `v0.8.0`

| ID | Item | Status | Owner |
| --- | --- | --- | --- |
| M7.0 | ADR: stable ids for edits — patches must keep pointing at the same road/intersection after re-import or other edits, which list-position ids (`NodeId`, `RoadId`, road pairs) don't guarantee; likely derived from the OSM source ids carried since M1.2, mapped to dense indices at load time; an id that no longer resolves after a re-import must fail loudly, never match a nearby item | todo | |
| M7.1 | Network edit model (change controller, lanes, speed limits, close roads) as serializable patches, addressed by the stable ids from M7.0 | todo | |
| M7.2 | Roundabout controller with gap acceptance (driver-dependent) | todo | |
| M7.3 | Convert crossroads → roundabout edit, incl. geometry | todo | |
| M7.4 | `rusthour-viz`: click-to-edit intersections, save edits as patch | todo | |
| M7.5 | Scenario + patch → CLI comparison end-to-end | todo | |

**Acceptance:** a user can replace an intersection with a roundabout in the
viewer, save it, and get a statistical before/after report from the CLI.

### M8 — Remote service → `v0.9.0`

| ID | Item | Status | Owner |
| --- | --- | --- | --- |
| M8.1 | `rusthour-api` `server` feature: HTTP control + WebSocket snapshot stream | todo | |
| M8.2 | Remote client implementing the same `Simulation` facade | todo | |
| M8.3 | `rusthour-viz` can connect to a remote simulation | todo | |
| M8.4 | API versioning and compatibility test | todo | |

## 7. Backlog (unscheduled)

- Crashes and near misses (reaction delay, perception error, distraction,
  time-to-collision metrics) — explicitly deferred.
- Pedestrians, bikes, public transit.
- Parking search.
- Actuated/adaptive traffic signals.
- Multi-threaded core (spatial partitioning) and wasm threads.
- Calibration against real counts.
- **Outlook: import in the browser.** Pick any area on a map in the viewer and
  simulate it, running the `osm` pipeline of `rusthour-import` compiled to
  wasm (osm2streets already runs in the browser for its own demo). Needs OSM
  data fetched client-side, e.g. from the Overpass API, within its usage
  policy. Not planned yet; the offline preparation step stays the default.
- Pin GitHub Actions by commit SHA (currently pinned by version tag).
