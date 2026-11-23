# ADR-0008: Stable ids for network edits (M5.0)

- **Status:** accepted
- **Date:** 2026-11-23
- **Roadmap:** M5.0; sets the interfaces for M5.1 (patches), M5.3
  (crossroads to roundabout), M5.4 (edit in the viewer) and M5.5 (scenario
  file)
- **Builds on:** ADR-0002 (`.rhnet` assets, the importer), ADR-0004 §2
  (determinism) and ADR-0005 §3 (signs resolved per road end)

## Context

M5 edits networks: change a junction's controller, a road's lanes or speed
limit, close a road, turn a crossroads into a roundabout. Edits are saved as
patches (M5.1) in scenario files (M5.5) and replayed on every run, often on
a map imported later than the patch was written. `NodeId` and `RoadId` are
positions in `Network`'s lists: any change to the OSM data, the importer or
`keep_central_part` reorders them, and a patch addressed by them would
silently edit some other road. The roadmap rules that out: a target that no
longer resolves must fail loudly and never match a nearby item.

Since schema 2 (M1.2) the network carries its OSM sources: `Node::osm_node`,
and `Road::osm`, an `OsmWayPart { way, from_node, to_node }`, the stretch of
a way between two of its nodes in travel direction. The importer
(`osm.rs`):

- splits each way at its ends, at nodes shared with another drivable way and
  at nodes a way revisits; interior vertices are geometry, not nodes;
- never merges roads across nodes, so a way boundary where exactly one other
  way continues stays a node of degree 2;
- splits a closed way, or a segment whose two ends another segment of the
  same way already has, at its middle vertex (`split_segment`), so no two
  roads of a way share their end nodes;
- drops a second road with an existing `(way, from, to)` (`refs.insert`);
  the property test `random_extracts_import_to_valid_networks` asserts the
  triple is unique and names the road's end nodes;
- cuts ways at the area's bbox and drops islands (`keep_central_part`).

### Measured on bensheim-5km

The three `assets-*` builds that ship bensheim-5km extracts (OSM data of
2026-10-20, 10-26 and 11-10), each imported with `main`'s importer and
`keep_central_part`; the shipped schema-4 map (`assets-2026-11-11`) equals
a fresh import of its extract.

| Quantity | Value |
| --- | ---: |
| Roads, nodes, movements | 3,000, 1,369, 4,936 |
| Roads or nodes without an OSM id | 0 |
| Duplicate `(way, from, to)` keys; duplicate OSM node ids | 0; 0 |
| Road parts dropped by the importer as duplicate keys | 0 |
| Closed ways split at their middle vertex (`split_segment`) | 1 (way 8139071) |
| Roads dropped by `keep_central_part` (not addressable) | 34 |
| Way and direction pairs with more than one road | 481 of 2,060, holding 1,421 roads (47%) |
| Nodes by degree (distinct neighbours) 1 / 2 / 3 / 4 / 6 | 244 / 409 / 624 / 91 / 1 |
| Degree-2 nodes at a way boundary; inside one way | 404; 5 |
| Nodes merged away (degree-2 nodes the importer removed) | 0 |
| OSM vertices on kept ways that are not nodes | about 3,990 |
| Roundabout roads; roads shorter than 30 m (split links etc.) | 41; 731 |

Three weeks of OSM edits (10-20 to 11-10):

| Change | Count |
| --- | ---: |
| Road keys gone or new; node keys gone or new | 0; 0 |
| `RoadId`s that changed | 0 |
| Roads with the same key and changed geometry | 19 (length change −3.5 m to +3.7 m) |
| Roads with the same key and changed attributes | 3 |
| Nodes moved, more than 1 m; most | 8, 5; 3.7 m |

The geometry changes are one mapper's realignment of the B 47 /
Ampèrestraße junction. Three weeks show no split or merged way, so the
churn over months is not measured here; the rules below don't depend on it
being small, only on it being detected.

## Decision

### 1. Keys

A **node** is addressed by its OSM node id, and a **road** by its
`OsmWayPart` triple `(way, from_node, to_node)`: the key the network already
carries. Edit-created items get keys scoped to the edit that made them (§4).

```rust
// rusthour-api, new module `refs.rs`.
pub type EditId = String;            // unique within a scenario

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NodeRef {
    Osm { node: i64 },
    Created { edit: EditId, n: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RoadRef {
    Osm(OsmWayPart),                 // { way, from_node, to_node }
    Created { edit: EditId, n: u16 },
}
```

`OsmWayPart` gains `Hash, PartialOrd, Ord` (field order: way, from, to).
In TOML a road reads `{ way = 4514502, from_node = 15837308, to_node =
15837309 }` and a created one `{ edit = "roundabout-1", n = 2 }`.

- **The key is a label, not a position or a shape.** It doesn't change when
  an edit shortens a road, moves its end to another node, or changes its
  lanes; nor when OSM moves a node or reverses a way's node order (the
  triple is in travel direction, not way order).
- **Way id alone** is ambiguous for 47% of the roads, so it never resolves a
  road.
- **(way, direction, ordinal) is rejected:** a new junction on a way, or a
  new vertex where a closed way is split, shifts the ordinal of every later
  road of that way, which would then name a different stretch without any
  error. That is the failure this ADR exists to prevent, and 47% of the
  roads are exposed to it. The triple instead stops resolving for exactly
  the roads whose ends changed.
- **Geometric matching** (nearest road, map matching) is ruled out by the
  roadmap.
- **Hand-built networks** (tests, the viewer's demo town) may fill
  `osm_node` and `osm` with negative ids, the OSM convention for objects
  not in the database. An item without a key cannot be targeted: the viewer
  disables editing it and says why.
- **Validation:** `Network` validation rejects two nodes with the same
  `osm_node` and two roads with the same `osm` key (new `NetworkError`
  variants; the enum is `#[non_exhaustive]`). The importer can't produce
  them (measured and property-tested), so a duplicate means a damaged or
  hand-made network, and keys never need an "ambiguous" outcome.

### 2. Resolution

`Refs` maps keys to the dense ids of one network and back. Built once per
load and kept up to date by the patch applier; lookups use a `HashMap` and
are exact, iteration for output is in key order.

```rust
pub struct Refs { /* key → id maps, id → key lists, deleted keys */ }

impl Refs {
    pub fn new(network: &Network) -> Self;   // network already validated
    pub fn road(&self, key: &RoadRef) -> Result<RoadId, Unresolved>;
    pub fn node(&self, key: &NodeRef) -> Result<NodeId, Unresolved>;
    pub fn road_ref(&self, road: RoadId) -> Option<&RoadRef>; // None: no key
    pub fn node_ref(&self, node: NodeId) -> Option<&NodeRef>;
}

#[non_exhaustive]
pub enum Unresolved {
    /// No item has this key. `near` lists roads of the same way and roads
    /// sharing an end node with the key, in key order, for the message only.
    NotFound { near: Vec<RoadRef> },
    /// An earlier edit in the scenario removed it.
    Deleted { by: EditId },
}
```

- **Exact match or error.** A key resolves only to the item with that exact
  key. `near` is a hint for the person fixing the patch; it is never applied,
  not even when it has a single entry.
- **All targets, all at once.** Applying a scenario resolves every target
  of every edit and reports every failure, in edit order, before anything
  runs: one re-import can break several edits, and the user should fix them
  in one pass. A run never starts with some edits left out. The CLI exits
  with an error listing them; the viewer (M5.4) shows the list and may offer
  to drop those edits, which changes the saved scenario visibly, never in
  memory only.
- **Message shape** (owned by the UX designer, M5.1): edit id, what it
  targets, the key, the road's name when the patch stored it, and the hint:

  ```text
  error: edit `slow-b47` targets road way 198045888 from 15837308 to
    1573379680 ("B 47"), which this map doesn't have; roads of way
    198045888 here: from 15837308 to 99000001, from 99000001 to 1573379680
    (the way gained a junction since the patch was made?)
  ```

  (Node 99000001 is made up for the example.)

- **Map provenance.** A patch records the map it was made on: the `.rhnet`
  metadata's `area` and `source_date`. If the loaded map differs in either,
  the applier warns once ("patch made on bensheim-5km of 2026-10-20, applied
  to the map of 2026-11-10; targets matched by OSM ids") and resolves by key
  as always. A patch made on bensheim-1km thus works on bensheim-5km, whose
  area contains it.

### 3. What OSM and importer changes do

| Change | Effect on keys | Result |
| --- | --- | --- |
| Node moved, vertex added or moved, tags changed | none | resolves; the edit applies to the current geometry and attributes |
| Way reversed (node order, `oneway=-1`) | none | resolves |
| New junction on a way (side street added) | the road through it splits into two keys | `NotFound`, both halves as hints |
| Way split upstream into two ways | roads on the part that gets the new way id, and the road at the split if it was no junction | `NotFound`, hints |
| Ways merged upstream | the absorbed way's roads get the surviving way id | `NotFound`, hints |
| Way deleted and redrawn (new id) | all its roads | `NotFound`, hints by shared end nodes |
| Junction node replaced (new id) | the node and every road ending there | `NotFound` |
| Road now outside the bbox or dropped as an island | gone | `NotFound` |
| Vertex added to a closed way split by `split_segment` | the middle vertex may move, re-keying that way's roads | `NotFound`, hints |

So an OSM change makes an edit fail exactly where the edited stretch
changed its ends, and never moves it to another stretch. Following splits
automatically (a chain of degree-2 nodes from `from` to `to`) is not done:
whether an edit of the whole old stretch still holds is for the user to
decide.

**Importer rules** that keep this true, binding on any future importer
(including an `osm2streets` swap, ADR-0002):

- Every node and road it emits carries its OSM key; keys are unique; the
  same input gives the same keys.
- **No merging across degree-2 nodes** while keys are single way parts.
  Today 409 of 1,369 nodes (30%) have degree 2 and none is merged. A
  simplification that merges them must first change `Road::osm` to the
  list of parts in travel order (schema bump); each part's key then resolves
  to the merged road, for edits that apply to a whole road.
- Changing how roads are split (e.g. no longer splitting at a way boundary)
  re-keys roads, so it is a breaking change of the importer and its notes
  say so.

### 4. Composing patches

- **Order:** a scenario's patches apply in the order listed, and each
  patch's edits in file order. Each edit resolves its targets on the
  network as the edits before it left it, so later edits can target what
  earlier ones created.
- **Edit ids:** every edit has an `EditId`, unique across the scenario (a
  duplicate is an error). The viewer generates them (`<kind>-<n>`) and the
  user may rename them.
- **Created items** are keyed `Created { edit, n }`. The creating edit
  numbers them `0..` by a rule that depends only on its own fields and the
  keys of the items it touches, never on `RoadId`s or on list order.
- **An edit records what its numbering depends on.** If those inputs
  differ on the map at hand, the edit fails rather than renumbers. Example:
  the roundabout edit stores the keys of the node's arms; a new arm after a
  re-import makes it fail with `ArmsChanged`, because the ring numbering
  would otherwise shift under the edits that target ring roads. More
  generally an edit never silently covers items it didn't name.
- **Deleted items** stay in `Refs` as `Deleted { by }`, so a later edit that
  targets them fails with the name of the edit that removed them, not with
  `NotFound`.
- **Conflicts:** two edits that set the same attribute of the same item are
  allowed; the later one wins and the applier warns ("`lanes-2` overrides
  `lanes-1` on road …"). Creating a key that exists is an error.
- **No undo:** removing an edit means removing it from the patch; networks
  are always rebuilt from the imported map plus the full patch list.

### 5. Determinism

- Resolution is a pure function of the network and the key; `HashMap`s are
  used for lookups only, and every list that is reported or applied (hints,
  errors, warnings) is sorted by key or kept in edit order.
- The applier's output network keeps untouched items in their original
  relative order, drops deleted ones, and appends created ones in edit order
  and then by `n`. The same map and patches give the same `Network`, so the
  core sees plain data and the golden hash rules of ADR-0004 apply
  unchanged; `rusthour-core` is not touched.
- Because edits address keys, a map whose lists are only reordered gets
  the same edits on the same keyed items. M5.1 tests this with a property
  that permutes a network's nodes and roads and applies one patch to both.

### 6. Asset format

The `.rhnet` format already carries everything the keys need; nothing is
added. `NETWORK_SCHEMA_VERSION` stays 4 and the assets workflow is not
re-run. Created keys live in `Refs`, not in `Network`, because patched
networks are never stored as assets. The new validation of §1 rejects no
shipped map (measured).

### 7. API surface for M5.1

`rusthour-api` gains `refs.rs` with `NodeRef`, `RoadRef`, `EditId`, `Refs`
and `Unresolved` as above, the two `NetworkError` variants, and the ordering
derives on `OsmWayPart`. M5.1 builds on them:

```rust
// M5.1's shape; M5.1 owns the names and the edit kinds.
pub struct Patch { pub map: MapRef /* area, source_date */, pub edits: Vec<Edit> }
pub struct Edit { pub id: EditId, pub kind: EditKind /* targets as NodeRef/RoadRef */ }
pub fn apply(base: &Network, patches: &[Patch])
    -> Result<(Network, Refs, Vec<PatchWarning>), Vec<PatchError>>;
// PatchError: { edit: EditId, target: Target, why: Unresolved }, plus
// DuplicateEditId, KeyExists, ArmsChanged and edit-specific checks.
```

A frontend keeps the `Refs` with the patched network: the viewer (M5.4)
turns a clicked `RoadId` into a `RoadRef` with `road_ref` when it saves an
edit, and `--road`/`?road=` keep taking `RoadId`s.

### 8. What M5.3 (crossroads to roundabout) needs

- **Target:** the junction's `NodeRef`. The edit stores the keys of the
  node's roads (its arms) and fails with `ArmsChanged` if they differ.
- **Arms keep their keys:** approach and exit roads are shortened to the
  ring and re-attached to ring nodes, but their `RoadRef`s don't change, so
  edits on them before or after the roundabout still resolve.
- **Created items:** one ring node per arm and one ring road between
  consecutive ring nodes. Ring node `n = 0` is at the arm whose smallest
  road key is lowest; numbering then runs counter-clockwise (the driving
  direction in right-hand traffic), using the arm order of ADR-0005 §1
  (pseudo-angles, no `atan2`). Ring road `n` runs from ring node `n` to
  `n + 1`.
- **Deleted:** the original node, as `Deleted { by }`; a controller edit
  aimed at it after the conversion fails naming the roundabout edit.
- **Order matters and is the user's:** an edit before the conversion that
  changes the node's controller is overwritten (warned as a conflict); one
  after it fails with `Deleted`.

## Alternatives considered

- **List-position ids pinned to a map hash:** exact for one asset, useless
  after any re-import, which is the case M5 needs.
- **(way, direction, ordinal):** shorter keys, but shifts silently (§1).
- **Following splits automatically:** convenient, but decides for the user
  whether an edit of the old stretch fits both new roads.
- **Storing created keys in `Network` (a `key` field on `Road`/`Node`):**
  needs a schema bump and asset rebuild for data that is never stored.
- **Synthesised negative OSM ids for created items:** no schema change
  either, but ids would collide across edits and hand-built networks, and
  errors couldn't name the creating edit.
- **A lenient mode that skips unresolved edits:** results of an A/B study
  would silently mix scenarios; dropping edits is an explicit, saved change
  in the viewer instead.

## Consequences

- Patches survive re-imports wherever the edited stretch keeps its end
  nodes, which over three weeks of Bensheim edits was every road; where it
  doesn't, the run fails before it starts and says which edits to fix.
- No asset rebuild, no core change, no golden-hash change.
- Future importer work is constrained: no degree-2 merging without the
  part-list schema change, and splitting rules are part of the key
  contract.
- How often keys break over months is not measured; the assets workflow
  can report gone and new keys against the previous build when it next
  rebuilds (backlog).

## Addendum (M5.1): patches as built

The decisions M5.1 took where the ADR left room.

- **Where it lives:** `rusthour-api` (`refs.rs`, `patch.rs`). `apply` is a
  pure function from the api `Network` to a new one, because the keys live
  only on api types; `rusthour-core` is unchanged and the golden hash with
  no patches stays as it was. The core's `Router` is only called to find
  roads that closures cut off (below).
- **Signature:** `apply(base: &Network, map: &MapRef, patches: &[Patch])
  -> Result<(Network, Refs, Vec<PatchWarning>), Vec<PatchError>>`. `map`
  is the loaded asset's area and extract date, for the provenance warning.
- **Format:** a patch is an optional `[map]` table (`area`,
  `source_date`) and a list of `[[edit]]` tables, each with `kind`, `id`
  and the kind's fields:

  ```toml
  [[edit]]
  kind = "speed_limit"
  id = "slow-b47"
  road = { way = 198045888, from_node = 15837308, to_node = 1573379680 }
  kmh = 30
  ```

  `Edit` is one internally tagged enum whose variants each carry `id`,
  rather than a struct with a flattened kind: serde can't reject unknown
  fields through `flatten`, and for hand-written files a misspelt optional
  field must be an error, not silently ignored. `Edit` is
  `#[non_exhaustive]`; M5.3 adds a `roundabout` variant.
- **Edit kinds:**

  | `kind` | Fields | Effect |
  | --- | --- | --- |
  | `junction_control` | `junction`, `control` | `end_control` of every road into the node: all-way stop or give-way, signals, or `none` (right of way by class) |
  | `arm_control` | `road`, `control` | `end_control` of that road |
  | `lanes` | `road`, `lanes` (1–16) | lane count; turn lanes kept if the count is unchanged, else dropped with a warning |
  | `turn_lanes` | `road`, `turn_lanes` | turn lanes as an OSM `turn:lanes` string (left to right, `\|` and `;`), and the lane count from it |
  | `speed_limit` | `road`, `kmh` (1–250) | speed limit |
  | `close`, `reopen` | `road` | one direction of a road |

  `control` is `none`, `give_way`, `stop` or `signals`. `turn_lanes` is
  parsed by `Turns::parse_osm_lanes`, which the importer now shares, so a
  patch accepts exactly the values a map does.
- **Closing a road** keeps the road, its id and its key, and removes every
  movement into and out of it; its `end_control` becomes `none`, so a
  signal plan has no phase for it. The router's SCC filter then leaves it
  out of demand and routes, and routing priors, landmarks, lane sets and
  signal plans are rebuilt from the patched network when the engine is
  built, as for any map. A closed road is not *deleted*: `reopen` restores
  its movements of the base map (except those to or from roads still
  closed). Roads that were routable before and are not after, other than
  the closed ones, give one `CutOff` warning naming the closing edits.
- **Conflicts:** the attributes are control, lanes (set by `lanes` and
  `turn_lanes`), speed limit and closure (set by `close` and `reopen`); a
  second edit of the same attribute of the same road wins with an
  `Overrides` warning. A `junction_control` counts per arm.
- **All failures together:** every edit is resolved and checked, across
  all patches, and errors are collected in edit order (unresolved targets,
  out-of-range values, duplicate edit ids) before anything is returned.
  No M5.1 edit creates or removes keys, so this equals resolving each edit
  on the state the previous ones left. `Refs::delete_node` (crate-private)
  and `Unresolved::Deleted` are ready for M5.3's roundabout edit.
- **Provenance:** one warning per distinct `[map]` that differs from the
  loaded map; a patch without `[map]` gets none.
- **Messages:** `PatchError` and `PatchWarning` implement `Display` in the
  §2 shape. The road's name is not stored in M5.1 patches (a TOML comment
  serves hand-written files); M5.4 may add an optional `name` when the
  viewer writes patches. Hints list keys only.
- **Validation:** `Network` validation (and `apply`) rejects duplicate OSM
  keys with `NetworkError::DuplicateOsmNode` and `DuplicateOsmRoad`.
- **CLI:** `rusthour run --map <file> --patch <file> [--patch <file>…]`
  takes TOML, or JSON for a `.json` file, applied in order. Warnings go to
  stderr as `warning: …`; any `PatchError` prints `error: …` per failure
  and exits with 2; an unreadable or malformed patch file exits with 1.
- **Demo (sanity check, no statistics):** bensheim-5km, 5,000 vehicles,
  seed 42, 72,000 steps (2 simulated hours), release build:

  | Run | Arrived | Removed as stuck | Roads with removals | Wall time |
  | --- | ---: | ---: | ---: | ---: |
  | No patch | 11,341 | 37 | 29 | 68 s |
  | 30 km/h on all 146 roads of Berliner Ring | 11,157 | 49 | 35 | 69 s |
  | Rodensteinstraße closed (way 42376313, both directions) | 11,484 | 101 | 61 | 67 s |

  The closure warns that it cuts off 6 roads behind it. The hash without
  patches equals the one of `main`.
