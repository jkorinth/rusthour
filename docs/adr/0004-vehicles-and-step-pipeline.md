# ADR-0004: Vehicle state layout and step pipeline (M2)

- **Status:** proposed
- **Date:** 2026-10-22
- **Roadmap:** M2.0; sets the interfaces for M2.1–M2.8

## Context

M2 puts the first vehicles on the M1 network. Five core items (M2.1–M2.5)
and the API and viewer (M2.6, M2.7) should be built in parallel, so module
boundaries and signatures must be fixed up front. Constraints: AGENTS.md §6
(SoA, sorted per-lane lists, no per-vehicle hashmaps or allocations in the
step loop, wasm32, all randomness from the seed) and §7 (module ownership).
The core network has a lane *count* per road but no lane geometry; lane
changing is M3. bensheim-5km has about 3,000 roads; ADR-0003 sets 5,000
vehicles as the viewer's test scene.

## Decision

### 1. Vehicle state (`vehicle`, M2.2)

```rust
pub struct VehicleId { pub slot: u32, pub generation: u32 }

pub struct Vehicles {          // one entry per slot, alive or free
    generation: Vec<u32>,      // bumped on despawn
    road: Vec<u32>,            // NO_ROAD (u32::MAX) when the slot is free
    lane: Vec<u8>,             // 0 = rightmost
    pos_m: Vec<f64>,           // front bumper, from the road start
    speed_mps: Vec<f64>,       // >= 0
    accel_mps2: Vec<f64>,      // written by the acceleration phase
    params: Vec<IdmParams>,    // per vehicle (see below)
    route: Vec<Vec<u32>>,      // road ids, origin..=destination; reused per slot
    route_pos: Vec<u32>,       // index of `road` in `route`
    depart_tick: Vec<u64>,
    free: Vec<u32>,            // LIFO free list of slots
    lanes: Vec<VecDeque<u32>>, // per LaneId: slots, front (head) first
    lane_first: Vec<u32>,      // per road: its lane 0's LaneId (prefix sum)
}
```

- **f64 everywhere in the state.** Basic IEEE operations on f64 are
  correctly rounded and identical on x86_64 and wasm32, which runs f64
  natively. f32 has a 0.5 mm resolution at 4–8 km; a car creeping at
  0.1 m/s moves 1 cm per 0.1 s step, so f32 would lose about 5% of each
  increment, and gaps are differences of positions. Memory is no concern
  (about 100 bytes per vehicle). The snapshot (§3) uses f32.
- **Slots with generations and a free list.** Ids stay stable for the
  vehicle's life, so the viewer can follow and inspect it (M4.4) and lane
  lists never need index fixups. Despawn bumps the generation, resets the
  slot's fields to defaults and pushes the slot onto `free`; spawn pops it.
  Holes cost nothing: the hot loop walks lane lists, not slots.
- **Allocation: none in steady state after warm-up.** Columns grow only
  when the population exceeds its previous peak (amortized `Vec` growth);
  `route` keeps each slot's `Vec` and its capacity across reuse; each lane
  deque is reserved at construction for `length / (4.5 + 2) + 1` vehicles;
  phases use scratch buffers owned by the engine. M2.8 checks this with a
  counting allocator (e.g. `dhat`, safe code) after warm-up; a flat route
  pool replaces the per-slot `Vec`s if it finds growth.
- **IDM parameters per vehicle**, in one column of a small `Copy` struct
  owned by `vehicle::idm` (M2.3), since one formula reads them all
  together. M4 profiles and noise produce a per-vehicle value at spawn, so
  no profile table lookup is needed. IDM stays in `vehicle` as the module
  docs and roadmap §3 say; `behavior` gets MOBIL and profiles (M3, M4):

  ```rust
  pub struct IdmParams {
      pub desired_speed_factor: f64, // v0 = factor × road speed limit
      pub time_headway_s: f64,       // T
      pub max_accel_mps2: f64,       // a
      pub comfort_decel_mps2: f64,   // b
      pub min_gap_m: f64,            // s0
      pub length_m: f64,             // 4.5 m for every car in M2
      pub sqrt_ab: f64,              // √(a·b), computed once at spawn
  }
  ```

- **Lanes in M2:** every road gets `lanes` queues
  (`LaneId = lane_first[road] + lane`) so the layout is final, but M2 uses
  **lane 0 only**, the rightmost lane in the direction of travel: spawns,
  transfers and the snapshot all use lane 0. M3.1 (MOBIL) fills the other
  lanes; M3.4 adds lane choice before intersections.
- **Lane lists hold slot ids, head first** (descending `pos_m`); the
  leader of entry `i` is entry `i - 1`. They stay sorted **by construction,
  never by sorting**: in M2 vehicles enter at the tail (spawn at offset 0,
  transfer from the previous road) and leave at the head (transfer onward,
  arrival), so `VecDeque` push-back and pop-front are O(1); integration
  forbids overtaking within a lane (§2). M3 lane changes and M5 mid-road
  spawns insert at `partition_point` on position, an O(lane) rare event.

### 2. Step pipeline (`engine`, M2.1)

One step, `dt = dt_ms / 1000` (default `dt_ms = 100`). Phases read the state as of the step start
unless noted; the engine owns the order, the clock, the RNG streams, the
transfers and the arrivals.

| # | Phase | Owner | Interface |
| --- | --- | --- | --- |
| 1 | Spawn: new trips; insert pending trips if the entry gap allows | `demand`, `routing` | `RandomOd::spawn`, `Router::route` |
| 2 | Route updates | `routing` | none in M2 (M5.5) |
| 3 | Intersection decisions for lane heads near their road end | `control` | `Controller::decide` |
| 4 | Leaders and accelerations, lane by lane | `vehicle::idm` | `idm::acceleration` |
| 5 | Integration, head to tail per lane | `engine` | — |
| 6 | Road transfers | `engine` | `Vehicles` methods |
| 7 | Arrivals (free slots), then `tick += 1` | `engine` | `Vehicles` methods |

Signatures (bodies are the items' work; types may gain fields):

```rust
// demand (M2.4): random origin–destination pairs, population target.
pub struct DemandContext { pub tick: u64, pub dt_ms: u32, pub active: u32, pub waiting: u32 }
pub struct Trip { pub origin: u32, pub destination: u32 }
pub struct RandomOd { pub target_vehicles: u32 /* … */ }
impl RandomOd {
    pub fn spawn(&mut self, ctx: &DemandContext, routable: &[u32],
                 rng: &mut ChaCha8Rng, out: &mut Vec<Trip>);
}
// routing (M2.4): A* over the road graph (roads = vertices,
// connections = edges, cost = length / speed limit).
impl Router {
    pub fn new(net: &Network) -> Self;   // CSR graph, largest SCC
    pub fn routable(&self) -> &[u32];    // roads in that SCC, ascending
    pub fn route(&mut self, net: &Network, trip: Trip, out: &mut Vec<u32>) -> bool;
}
// control (M2.5): one controller per node; approaches in from_road order.
pub struct Approach { pub from_road: u32, pub to_road: u32, pub slot: u32,
                      pub dist_m: f64, pub speed_mps: f64,
                      pub can_stop: bool }   // dist ≥ v²/(2b)
pub trait Controller {
    fn decide(&mut self, net: &Network, tick: u64, approaches: &[Approach],
              go: &mut [bool]);
    fn hash_into(&self, h: &mut StateHasher);
}
pub enum NodeControl { Priority(priority::Priority) } // in control.rs
// vehicle::idm (M2.3): δ = 4, computed as squares, no `powf`.
pub fn acceleration(p: &IdmParams, v0: f64, v: f64, leader: Option<Leader>) -> f64;
pub struct Leader { pub gap_m: f64, pub speed_mps: f64 }
```

- **Spawn:** trips that can't enter yet wait in a FIFO per origin road. A
  trip enters with its front at offset 0 and speed `v = min(v0, v_tail)`
  (`v0` if the lane is empty) once the gap to the lane's tail is at least
  `s0 + v·T`. A vehicle approaching from upstream may then brake hard; the
  −9 m/s² clamp and the transfer rule below bound that.
- **Routing (M2.4):** `Router::new` finds strongly connected components
  with an iterative Tarjan (no recursion: wasm stack) and keeps the largest,
  ties to the lowest minimum road id; it reports the number and share of
  roads excluded, which the api shows on load. OD pairs come from
  `routable()`, so every trip has a route and the M1.11 one-way traps are
  avoided. Roads crossing the area edge fall out, so **M2 traffic is a
  closed system** with no through traffic (M5.4). A* ties break by road id.
  Budget: ≤ 0.25 ms of routing per step at 5,000 vehicles (about 2.5 new
  trips per step with 200 s trips), measured separately in M2.8; if it is
  exceeded, a per-step route cap makes trips wait.
- **Leaders across road ends:** the head's leader is found by walking its
  route up to `50 m + v²/b` ahead: if `control` said stop, a standing
  obstacle at the road end (the stop line); otherwise the tail of the next
  route road that has one, with the gap summed over the roads in between.
  The destination road's end is free road (the vehicle arrives there).
- **Approaches** are lane heads within that horizon of their road end,
  grouped by node and sorted by `from_road`. M2.5's priority rule:
  - *Road attributes PR first (M2.5a):* `start_dir`/`end_dir` unit vectors
    on the core `Road`, computed by `rusthour-api` from the geometry, and
    `roundabout: bool` on the api and core `Road`, set by the importer from
    `junction=roundabout|circular` (already read for one-way).
    `NETWORK_SCHEMA_VERSION` becomes 3 and the sample assets are rebuilt.
    Older viewer versions keep working: each release's web build bundles
    the maps of its own commit's schema (`assets.py bundle` at release),
    `pages.py` deploys those builds unchanged, and main's deploy skips maps
    of another schema; until the rebuild, main shows the demo grid.
  - *Class:* circulating (from a roundabout road) > higher speed limit >
    more lanes. A class-higher approach is never held for a lower one.
  - *Arriving:* an approach is arriving if `dist ≤ 3 s · v + 2·s0`; this
    is the time-to-line test, and the `2·s0` covers heads that IDM stops at
    just over `s0` from the line.
  - *Yielding:* A yields if an arriving approach B at the node is of a
    higher class, or of the same class and from A's right:
    `cross(end_dir_A, end_dir_B) > 0.5` (30°–150°; right-hand traffic).
    With `|cross| ≤ 0.5` (opposite or merging approaches) the two conflict
    and the one that arrived first goes; all movements at a node conflict
    (nodes are points; M3.2 refines this).
  - *Deadlock, per class:* among the approaches that yield to no higher
    class, if every one yields to an equal, the one whose head became
    arriving earliest goes, ties by a rotating pointer per node. Both are
    `Priority` state and hashed. A busy major road holds the minors, but
    they resolve as soon as it has a gap.
  - *Commitment:* a vehicle is **released** only when it has `go` and can
    no longer stop (`go && !can_stop`); it keeps `go` until it leaves the
    road, so it never overshoots a stop line it was told about too late.
    `Priority` keeps the released slot per incoming road. Before that point
    `go` is recomputed every step, so the class order always holds.
  - *Tests:* a saturated 4-way equal junction and a saturated 4-arm
    roundabout keep every approach discharging (each gets `go` at least
    once per simulated minute) and never lock up; a released vehicle is not
    stopped by a later major arrival.
  - Right-before-left can starve the left approach of a saturated
    T-junction; that is the real rule and accepted.
- **Integration:** ballistic with non-negative speed (Treiber & Kesting):
  if `v + a·dt < 0`, `x += -v²/(2a)` and `v = 0`; else `x += v·dt +
  a·dt²/2`, `v += a·dt`. Accelerations are clamped to ≥ −9 m/s². Head to
  tail, each position is clamped to `x ≤ x_leader,new − length_leader`, and
  then `v = min(v, (x_new − x_old) / dt)`, so no stale speed feeds the next
  step. Since leaders never move back, `x_new ≥ x_old` and `v ≥ 0` hold.
- **Transfers:** while `pos ≥ length`, subtract the length and move to the
  next route road (short roads can be crossed in one step). Entrants are
  collected in an engine scratch buffer, sorted by `(lane, pos desc, slot)`
  and pushed to the tails in that order. An entrant that would not fit
  behind the current tail (`pos > x_tail − length_tail`, e.g. at a merge)
  is **blocked**: it stays the head of its old road at `pos = length`,
  `v = 0`, and retries next step. Positions are never negative.
- **Determinism:**
  - Iterate lanes in `LaneId` order, vehicles head to tail. Phases 3 and 4
    read only start-of-step state (Jacobi style), so their results don't
    depend on iteration order and could later run in parallel.
  - No `HashMap`/`HashSet` in core: `crates/rusthour-core/clippy.toml`
    lists them in `disallowed-types` (M2.1). Sort floats with `total_cmp`.
  - **One `ChaCha8Rng` stream per subsystem**, same seed, different
    `set_stream`: `demand` = 1, `behavior` = 2 (M4 noise). A change in one
    subsystem doesn't shift another's draws, which keeps golden-hash churn
    local and gives A/B comparisons common random numbers (M6, M7). Integer
    sampling (`next_u32`/`next_u64`), floats from bits (`(u >> 11) as f64 *
    2⁻⁵³`).
  - Floats: only `+ - * /`, `sqrt`, `abs`, `min`/`max`, comparisons and
    `floor`. No `mul_add`, `powi`, `powf`, `exp`, `ln`, trigonometry in
    state updates; if ever needed, use the pure-Rust `libm` crate. NaN
    payloads differ between targets, so state must stay finite
    (`debug_assert!` in integration).
  - M2.1 adds a CI job that runs `tests/determinism.rs` on
    `wasm32-wasip1` under wasmtime and asserts the same golden hash, which
    proves native = wasm.
- **State hash:** `Vehicles::hash_into` feeds every column in slot order
  (free slots are reset, so whole columns hash without branches), then
  `free` and every lane list in `LaneId` order; the engine adds each RNG
  stream (seed, stream, word position), pending trips and each node's
  controller state. Hashing is O(state), so it leaves the per-frame
  `Status` (§3).
- **Extension points:** each controller (M3.2 stop, M3.3 signals, M7.2
  roundabout) is a submodule implementing `Controller`; the engine
  dispatches statically through `NodeControl`, whose variant list and
  `match` in `control.rs` are the only shared lines. This keeps `Engine:
  Clone` and avoids `Box<dyn>`. Demand models follow the same pattern when
  M5.2 adds the second one. AGENTS.md §6 and the `control` docs say so.
- **Clock:** stays `tick × dt_ms` from zero. A start time of day arrives
  with the scenario format in M5.1, its first consumer (M5.3, M5.6).
- **Configuration:** `Config` gains `target_vehicles: u32` (default 0, so
  existing callers see no traffic) until M5.1 scenarios replace it.

### 3. API surface (M2.6, M2.7)

```rust
// rusthour-api, serde, PROTOCOL_VERSION 2.
pub struct VehicleId(pub u64);       // generation << 32 | slot
#[derive(Default)]
pub struct VehicleSnapshot {         // SoA, in lane order
    pub tick: u64,
    pub id: Vec<VehicleId>,
    pub road: Vec<RoadId>,
    pub lane: Vec<u8>,
    pub offset_m: Vec<f32>,
    pub speed_mps: Vec<f32>,
    pub length_m: Vec<f32>,          // 4.5 in M2; vehicle classes in M4.2
}
pub trait Simulation {               // additions
    fn snapshot_into(&self, out: &mut VehicleSnapshot); // reuses buffers
    fn state_hash(&self) -> u64;     // on demand; leaves `Status`
}
pub struct Placement;                // from &Network, once
impl Placement { pub fn place(&self, road: RoadId, lane: u8, offset_m: f32,
                              lane_width_m: f32) -> (Point, [f32; 2]); }
pub struct Pacer;                    // see the contract below
impl Pacer {
    pub fn frame(&mut self, frame_s: f64, step_cost_s: f64) -> u64; // steps now
    pub fn achieved_scale(&self) -> f64;
    // set_scale, pause, resume, step_once
}
```

- **Road-relative on the wire, world x/y in `rusthour-api`.** The core
  stays rendering-free; `Placement` (per-road cumulative segment lengths and
  a binary search) turns a snapshot entry into a position and direction, for
  in-process and remote clients alike, and is smaller on the wire for M8.
- **`Placement` rules:** the offset is scaled by polyline length / core
  `length_m` (equal today; scaled, not clamped, if they ever differ), then
  clamped to `[0, length]`. Lanes lie right of the direction of travel,
  lane 0 outermost: offset to the right `((lanes − 1)/2 − lane) · w`,
  centered like the drawn road; if the road has a *twin* (another road whose
  geometry is this one reversed, as the importer makes for two-way ways),
  `(lanes − lane − 0.5) · w`, so the two directions don't overlap. M2.7
  draws twin roads offset the same way.
- **Motion between steps:** at time scales ≤ 1× the viewer extrapolates
  `offset + speed · Δt` with `Δt < dt`, clamped by `Placement`; above 1×
  several steps land in a frame and it draws the latest snapshot as is.
  Interpolating two snapshots was rejected (one step of latency, matching
  by id).
- `Status` drops `state_hash` and gains `vehicles: u32` (active) and
  `waiting: u32` (pending trips). The CLI's `run` output keeps printing the
  state hash (M2.6b), so the M1 trial's commands still work.
- **`Pacer` contract** (M2.6a; run/pause and time scale stay in the
  frontend per ADR-0003 §6, the facade stays step-based):
  - steps per frame are capped by a time budget (6 ms per frame in the
    browser, where single-threaded wasm stepping blocks painting); a backlog
    beyond the cap is dropped, never caught up (no spiral of death);
  - `achieved_scale` is a smoothed sim-seconds-per-real-second, so the
    viewer shows "100× (running at 37×)" when it falls below 90%;
  - resume after pause starts with an empty backlog (no burst); `step_once`
    while paused runs exactly one step;
  - the slider maps log-scale, `10^t` for `t ∈ [−1, 2]` and beyond;
  - the viewer takes one snapshot per frame, after stepping.
- **M2.7 acceptance** adds: reset with the same seed, a sim-clock readout
  (`tick × dt_ms`), active and waiting counts, and the achieved speed.
  Picking hit-tests placed positions in the viewer; no extra api.
- **Budget** (`dt_ms = 100`, i.e. 10 steps per simulated second): one step
  with 5,000 vehicles on bensheim-5km takes ≤ 1 ms natively (release),
  i.e. ≤ 200 ns per vehicle-step, routing included (≤ 0.25 ms); snapshot
  plus placement ≤ 0.5 ms. The M2 acceptance check: 5,000 vehicles at
  ≥ 10× real time (100 steps/s) in the browser on the ADR-0003 reference
  setup. 100× needs 1,000 steps/s, beyond one core at this size, which is
  why the `Pacer` caps and reports.

### 4. Work split

No separate interface PR: the signatures above are the contract. Every
module item is self-contained and tested in isolation; only M2.1 touches
`engine.rs`.

| Wave | Items (parallel within a wave) | Depends on |
| --- | --- | --- |
| 1 | **M2.3** `vehicle/idm.rs`: `IdmParams`, `acceleration`; tests: free road → v0, stop at a standing obstacle at ≥ s0, equilibrium gap. Tiny; **lands first** (adds `pub mod idm;`) | ADR |
| 1 | **M2.4** `routing` (CSR, iterative Tarjan, A*) and `demand` (`RandomOd`) | ADR |
| 1 | **M2.5a** road attributes: directions, `roundabout`, schema 3, importer, asset rebuild | ADR |
| 1 | **M2.6a** api `VehicleId`, `VehicleSnapshot`, `Placement`, `Pacer`, in **new files only** plus `mod`/`pub use` lines in `lib.rs` | ADR |
| 1 | **M2.1a** RNG streams, `clippy.toml`, wasm determinism CI job | ADR |
| 2 | **M2.2** `vehicle` store: slots, generations, free list, lane lists, `hash_into`; proptests for "lanes sorted, every live slot in exactly one lane". Starts in wave 1, rebases on M2.3 before review | M2.3 |
| 2 | **M2.5b** `control/priority.rs` and `Controller`, tests on hand-built `Approach` slices incl. the saturated cases | M2.5a |
| 3 | **M2.1b** pipeline phases, leader lookup, integration, transfers, arrivals; new golden hash with traffic | M2.1a, M2.2 (M2.4/M2.5b wired as they land) |
| 4 | **M2.6b** facade wiring, `Status`, CLI hash output; **M2.8** criterion bench in `rusthour-api/benches` on a generated grid (no I/O; the import crate would form a dev-dependency cycle), routing timed separately, allocation check | M2.1b, M2.4 |
| 5 | **M2.7** viewer: vehicle quads, play/pause, speed slider, controls above (can start on fake snapshots after M2.6a) | M2.6b |

Critical path: M2.3 → M2.2 → M2.1b → M2.6b → M2.7 → M2 trial.

**Serialization of shared edits:**

- *Golden hash:* changed by M2.1a, M2.5a and M2.1b, expected in that
  order. Whichever of them merges later rebases on `main`, regenerates
  the constant and adds a line to its comment; reviewers re-check only that
  line (a trivial rebase keeps verdicts).
- *`rusthour-api`:* M2.5a edits `network.rs`, M2.6a only new files and the
  `lib.rs` module lines, M2.6b `lib.rs` (`Simulation`, `Status`) after both.

## Alternatives considered

- **f32 state:** half the memory, but too coarse for small per-step
  increments a few km from the road start; no memory pressure justifies it.
- **Dense arrays with `swap_remove`:** no holes, but ids change on every
  despawn and lane lists need index fixups.
- **Re-sorting lane lists each step:** O(n log n) work for an order that
  only changes at the ends.
- **In-place (Gauss–Seidel) updates:** results would depend on iteration
  order; Jacobi costs one `accel` column.
- **One RNG stream:** simpler, but any new draw reshuffles all later ones
  and breaks common random numbers in A/B runs.
- **Trait objects (`Box<dyn Controller>`):** break `Clone` on `Engine`
  and add dynamic dispatch; the enum costs one shared line per controller.
  IDM gets no trait: it is one function, and profiles only change its
  parameters.
- **Lowest-id tie-break at nodes:** starves the other approaches as soon as
  the low-id road has a queue; first-come plus a rotating pointer is fair.
- **World x/y in the snapshot:** simpler for the viewer, but larger on the
  wire and no along-road extrapolation or clamping; the api computes it.
- **Time of day now:** no consumer before M5.

## Consequences

- The golden hash changes in M2.1a (RNG streams), M2.5a (road attributes)
  and M2.1b (traffic); each PR says so.
- `PROTOCOL_VERSION` becomes 2 in M2.6 (`Status` change, snapshot types);
  `NETWORK_SCHEMA_VERSION` becomes 3 in M2.5a, so `.rhnet` assets are
  rebuilt.
- M2 traffic is single-lane, a closed system, and conflicts inside nodes
  are not modelled; multi-lane roads have single-lane capacity until M3.
- `Controller` and `Approach` are the M3.2 interface: M3.2 adds the stop
  controller and per-node selection, not a new trait.
