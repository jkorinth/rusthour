# ADR-0002: Map asset pipeline and the importer crate

- **Status:** proposed
- **Date:** 2026-10-21

## Context

Simulations run on road networks derived from OpenStreetMap. Parsing and
simplifying raw OSM data is slow and needs heavy native dependencies
(`osm2streets`), while the browser build must start fast and the core must
stay free of I/O (ADR-0001). Areas are defined by name in `data/areas.toml`.

## Decision

Map preparation is an **offline step** handled by a fifth crate,
`rusthour-import`:

1. **Fetch** an OSM extract for a named area (e.g. cut the area's `bbox` from
   a regional Geofabrik `.osm.pbf` with osmium). Fetching is separate from
   importing so the source file, date and command can be recorded in
   `data/<area>/README.md`.
2. **Import** (feature `osm`, native-only, plus a `rusthour-import` binary):
   run `osm2streets` on the extract, simplify to the network data types
   defined in `rusthour-api`, and write one asset per area.
3. **Asset format** (`.rhnet`, always compiled, wasm-compatible): a small
   header (magic bytes, format version, area name, source date, importer and
   `osm2streets` versions, payload hash) followed by the serialized,
   compressed network. Serialization and compression use pure-Rust crates
   only, so decoding works on `wasm32-unknown-unknown`; the exact choices are
   made in M1.3.
4. **Load**: frontends obtain the bytes (file on native; HTTP in the
   browser), decode them with `rusthour-import`, and hand the network to the
   simulation through `rusthour-api`. The core only ever receives in-memory
   data.
5. **Cache**: native assets are their own cache; a format-version mismatch
   means re-import. Browser assets are served under content-hashed names
   (e.g. `bensheim-5km.<hash>.rhnet`) as immutable, so the HTTP cache applies;
   the Cache API or IndexedDB can be added for offline use later.

Dependencies: `rusthour-import` depends on `rusthour-api`, not on
`rusthour-core`. `rusthour-viz` and `rusthour-cli` may depend on
`rusthour-import` without the `osm` feature, so `osm2streets` never enters the
browser build.

Small sample assets (the Bensheim areas) are committed or attached to
releases, so users can run the simulator without running the importer. OSM
data is ODbL: assets and the viewer carry "© OpenStreetMap contributors".

## Consequences

- Fast startup everywhere; no OSM parsing at runtime.
- The network data types live in `rusthour-api`, which makes them part of the
  public, versioned interface; format changes bump the `.rhnet` version.
- The importer can evolve (new OSM tags, better simplification) without
  touching the core, as long as the network types stay stable.
- Importing arbitrary areas in the browser is not supported for now. Because
  the `osm` pipeline is isolated in one feature, it can later be compiled to
  wasm (ROADMAP backlog: "Outlook: import in the browser").
