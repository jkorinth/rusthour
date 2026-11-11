# ADR-0005: Lanes and intersections (M3)

- **Status:** proposed
- **Date:** 2026-11-09
- **Roadmap:** M3.0; sets the interfaces for M3.1–M3.10
- **Builds on:** ADR-0004 and its addenda (M2.1b, M2.5b, M2 trial, M2.9,
  M2.10)

## Context

M2 runs one lane per road, decides right of way from the speed limit, then
the lane count, then right before left, and makes every movement at a node
conflict with every other. The M2 trial (docs/trials/M2.md) found heavy
congestion at the viewer's density, gridlocks caught only by the
stuck-vehicle net, a roundabout rule artefact on Berliner Ring, and
removals that nobody can see. The maintainer decided:

1. **turn costs in routing first**, a static penalty per movement plus
   the observed delay at the stop line, with A* kept exact;
2. **MOBIL in the European asymmetric variant**, zipper merging included;
3. **right of way from the map**: signs and signals, then
   `priority_road`, then `highway` class, then speed and lanes, then right
   before left, which needs a network schema bump;
4. **the M2 gridlock follow-ups**: exempting circulating heads from "no
   room", exposing the stuck-removal count, and re-measuring Berliner
   Ring.

The ADR-0004 rules are unchanged:

- structure of arrays, with no allocation after warm-up;
- Jacobi decisions from start-of-step state;
- one random stream per subsystem;
- floats limited to basic operations and `sqrt`, with no trigonometry;
- one enum per extension point (AGENTS.md §6);
- 1 ms per step at 5,000 vehicles.

## Decision

### 1. Movements (`network::movement`, M3.1)

A *movement* is a connection `(from_road, to_road)`. Its properties are
derived once at load, in core, from fields the network already has:

- **Turn type**, from `a = from.end_dir` and `b = to.start_dir`:
  - `UTurn` if `a·b < −0.9`;
  - `Straight` if `|a×b| ≤ 0.5` and `a·b > 0`;
  - otherwise `Left` if `a×b > 0` and `Right` if not (right-hand
    traffic).

  A `Left` becomes `LeftAcross` when the node has an incoming road
  opposite to `from` (`end_dir · a < −0.7`), i.e. when the turn crosses
  oncoming traffic.
- **Movement index:** the position in the router's CSR `succ` array. The
  engine finds it by scanning the successors of `from_road` (at most about
  6).
- **Conflicts (M3.3):**
  - *Arms:* the roads at a node are grouped into arms by the neighbor node
    they lead to or come from. A road and its twin form one arm; a one-way
    road is an arm of its own.
  - *Angle:* each arm has one angle, pointing away from the node: the
    outgoing road's `start_dir`, or `−end_dir` for an incoming-only arm.
    Arms are sorted counter-clockwise by pseudo-angle (quadrant, then
    `cross`; no `atan2`), with ties broken by the lowest road id.
  - *Points:* inside each arm the out-point comes before the in-point,
    counter-clockwise, so a bent arm with slightly different in and out
    angles cannot swap them. In right-hand traffic this makes a left turn
    cross the oncoming straight movement and a right turn cross nothing
    (checked on the four-arm case).
  - *Test:* two movements conflict if their chords cross, i.e. exactly one
    end of one chord lies strictly between the ends of the other, or if
    they merge. Two movements from the same lane never conflict.
  - *Merges:* in M3.3 two movements merge when they share a target
    *road*. M3.5b refines this to a shared target *lane*, so two lanes
    entering different lanes of one road no longer conflict.
  - Each node stores its conflicts as a bit matrix, computed once.

### 2. Turn costs (`routing`, M3.1, first)

The edge cost from road A to road B becomes
`turn_s(type) + delay_s(movement) + travel_s(B)`. Both new terms are
non-negative and the straight-line heuristic ignores them, so A* stays
exact. The router stores the edge costs in an array parallel to `succ`.

- **Static turn cost `turn_s`:** `Straight` 0 s, `Right` 2 s, `Left` 4 s,
  `LeftAcross` 8 s, `UTurn` 20 s (`shortcut:`; tuned in the M3 trial).
- **Observed delay `delay_s`:** a per-movement exponential moving average
  of the time the head stood held at the line (`go = false`). It is
  counted in a new vehicle column `held_ticks`, added to the movement's
  accumulator when the vehicle transfers, and reset after.
  - Every `COST_PERIOD_MS` (5 s), `Router::observe` moves the average
    towards the mean of the completed passages. If the current head has
    been held longer than that, it uses the head's wait so far instead, so
    a movement that stops flowing keeps getting costlier. This is the M2.9
    property that keeps routes out of gridlocks.
  - Weight 0.2, floored at 0.
  - With no samples, the average decays towards the movement's *prior*:
    4 s if it must yield (its from-road ranks below another incoming road
    at the node, §4), 6 s at a stop sign, `red²/(2·cycle)` at a signal
    (M3.4), otherwise 0.
  - The road estimate of M2.9 stays as it is: it measures how full the
    lane is, while `delay_s` captures what is specific to the movement.
- Router state that is hashed adds the delay averages and accumulators.
  The cost budget is unchanged: an edge costs one extra addition.

### 3. Road attributes from the map (schema 4, M3.2)

New fields on the api `Road`, with `#[serde(default)]` so JSON networks
still load; `.rhnet` maps are re-imported:

```rust
pub class: RoadClass,         // OSM `highway`; `_link` takes its parent's class;
                              // Unknown for hand-built roads
pub priority_road: bool,      // way has `priority_road=designated|yes_unposted`
pub end_control: EndControl,  // None | GiveWay | Stop | Signals, at this road's end
pub turn_lanes: Vec<Turns>,   // empty = unknown, else one per lane, rightmost (lane 0) first
// Turns: u8 bit set LEFT | THROUGH | RIGHT | U_TURN
```

- **Validation (`NetworkError`):**
  - `turn_lanes` must be empty or have `lanes` entries;
  - each `Turns` must be non-empty and use only known bits;
  - `lanes ≤ 16`, because lane sets are `u16` masks (new variant
    `TooManyLanes`).
- **`Node` is unchanged.** The importer resolves each sign to the end of
  the road it applies to, where the OSM geometry is available, so the core
  never has to guess a direction.
- **Importer (`osm.rs`):**
  - `read_pbf` keeps the tags of nodes with `highway=give_way|stop|traffic_signals`.
  - A tagged node *on a road* applies to that road if its vertex position
    along the polyline (not its coordinates) is within 30 m of the road's
    end. `direction` or `traffic_signals:direction`, when present, selects
    the road's direction. Without it, a sign applies to the direction whose
    end is nearer, or to both directions when the road is shorter than
    60 m and the sign sits in its middle; a signal applies to both.
  - Where a road ends at a node that is no junction and exactly one way
    continues (a way boundary, often where a way is split at the sign),
    the 30 m search continues along that way to the next junction (added
    in M3.2: Berliner Ring's give-way signs sit just before such splits).
  - A tagged node that is no junction (a way boundary or dead end) is
    treated like a vertex on a road's interior, at distance 0 from the
    roads ending there. It applies only where a junction lies within 30 m,
    so the 30 m rule can't turn it into a spurious stop (changed in M3.2;
    such nodes were ignored before).
  - Mid-block signals further than 30 m from an end are ignored: they are
    pedestrian crossings, and there are no pedestrians.
  - A tagged *junction node* applies a signal to all incoming roads. A
    sign there applies to the incoming roads of the lowest class at the
    node, or to all of them if every class is equal (all-way stop).
  - `turn:lanes[:forward|:backward]` is read left to right and reversed,
    so lane 0 comes first. `slight_*` and `sharp_*` count as left or
    right, `merge_to_*` and `none` as through, and `reverse` as `U_TURN`.
  - *Never fail a map:* a `turn:lanes` whose count disagrees with `lanes`
    is dropped, and lane counts above 16 are clamped to 16. Both are
    reported. A `Turns` bit for a movement that doesn't exist is no error:
    the core falls back to the default rule, and the importer warns.
  - *Split junctions:* a road shorter than 30 m between two signalled
    nodes never gets `Signals`. Traffic on that link flows on, so two
    independent plans can't trap it.
  - *Report:* the importer's summary adds counts of:
    - signals, stop signs and give-way signs applied;
    - signalled nodes, and clusters of them joined by short links;
    - mid-block signals ignored;
    - junction signs without a direction, and how they were resolved;
    - `turn:lanes` parsed and ignored, and lanes clamped;
    - `priority_road` ways, and roads of unknown class.

    Each ignored or ambiguous case lists its first few roads as name plus
    OSM way id, in the style of the existing dropped-roads warning, with a
    warning when more than 5% are ignored.
- **Core `Road`** gains `class` as a rank `u8` (0 means unknown),
  `priority_road`, `end_control` and `turn_lanes: Vec<u8>`.
  `Network::hash_into` includes them.
- `NETWORK_SCHEMA_VERSION` becomes 4. Old viewer releases keep their
  schema-3 maps, as for schema 3 (ADR-0004).
- **Rollout:** M3.2's PR merges together with, or after, the schema-4
  asset rebuild (the assets workflow and its `assets.toml` PR), so the
  Pages deploy never pairs a schema-4 viewer with schema-3 maps. The
  backlog fix for the schema-mismatch message ("not a Rust Hour map, or
  damaged") lands before M3.2.

### 4. Right of way (`control`, M3.3 and M3.4)

**Rank**, per road, compared lexicographically, highest first:
`(circulating, unsigned, priority_road, class, speed limit, lanes)`.
`unsigned` is false for a `GiveWay` or `Stop` end, and for a non-signal
road at a signal node. Right before left stays the fallback among equals.

Changes to the M2.5 rule:

- **Per-lane approaches:**
  - an approach is a *lane head*, and `Approach` gains `lane` and
    `to_lane`;
  - `Priority` arms are keyed by the incoming lane;
  - classes are per road.
- **Only conflicting movements yield** (the §1 matrix):
  - opposite straight movements no longer take turns, which addresses the
    congestion finding;
  - among equal ranks, a `LeftAcross` yields to the oncoming straight and
    right turns (StVO §9(3)) before the right-before-left and first-come
    rules apply.
- **Stop sign:** a stop head gets `go` only after it has stood
  (`v < 0.1 m/s`) within `2·s0` of the line. After that it behaves as
  give-way. A per-arm `stopped` flag is hashed.
  - An unstood stop head is not *competing*: no one yields to it, and the
    deadlock rule can't pick it.
  - The stop gate is applied last, after the deadlock rule, so it can't
    force an unstood stop head through at an empty node.
  - The same holds for signal arms that are not green: they don't compete
    and are gated last.
- **Kept from M2.5:** commitment (`go && !can_stop`), don't block the
  box, the per-class deadlock rule with first-come and the sticky winner,
  and the saturated tests.
- **Ring exemption (M3.8):** `blocked` is not set for a head on a
  roundabout road whose next road is also on the ring. Vehicles already
  circulating keep priority, and the transfer fallback (hold at the road
  end) covers a ring that is really full.

**Controllers:**

```rust
pub enum NodeControl {           // control.rs, the only shared lines
    Priority(priority::Priority),// signs, priority_road, class, RbL (M2.5, M3.3)
    Signal(signal::Signal),      // fixed-time plan (M3.4)
}
```

- **Selection:** a node gets `Signal` if any incoming road ends in
  `Signals`, otherwise `Priority`.
- **The `Controller` trait is unchanged.** Stop and give-way are inputs to
  `Priority`'s rank and stop logic, not separate controllers: a node with
  a stop sign on one arm only still needs the full rule.
- **Constructors:** `Priority::new(net, node, &incoming_lanes)` and
  `Signal::new(…)` precompute the conflict matrix.
- **`Signal` (M3.4):**
  - *Plan:* a default fixed-time plan derived from geometry. Signal arms
    are grouped into phases greedily in road-id order, with opposite arms
    (`dot < −0.7`) sharing a phase. Each phase has 25 s green, 3 s amber
    and 2 s all-red, with offset 0 and no green wave (`shortcut:`).
  - *Phase state:* the phase is a function of the tick, so nothing needs
    hashing except the inner `Priority`.
  - *Green:* the plan's green arms are passed through an inner `Priority`,
    which handles permissive left turns and non-signal arms. Non-signal
    arms are never ranked above signal arms.
  - *Amber:* `go` only if the vehicle can't stop.
- **Signals snapshot:** `Simulation::signals_into(&mut SignalSnapshot)`
  gives `(road, Green | Amber | Red, next_change_s: f32)` per signal arm,
  for the viewer and a tooltip such as "red, 12 s".
- **Not modelled:** clusters of signals spanning split junctions. Their
  internal links act as non-signal arms that yield, until the trial shows
  a need.

### 5. Lanes (`vehicle`, `network::lanes`, M3.5)

- **Lane sets per movement (`network::lanes`, at load):**
  - each movement gets `from_mask: u16`, the lanes of A that may take it;
  - *default rule* (when `turn_lanes` is empty, or names a movement that
    doesn't exist): `Right` uses lane 0, `Left`, `LeftAcross` and `UTurn`
    use lane n−1, and `Straight` uses all lanes;
  - with no straight movement, right turns get the right half of the lanes
    (rounded up) and left turns the left half;
  - one lane serves everything;
  - *with `turn_lanes`:* a lane serves the movements whose type matches
    its bits; a lane that matches nothing falls back to the default rule.
- **Entry lane on the target road:**
  - straight keeps the lane index, clamped to B's lane count; right turns
    enter lane 0; left turns and U-turns enter B's leftmost lane;
  - if B is shorter than 60 m and that lane doesn't serve the vehicle's
    following movement, the transfer picks the nearest lane that does.
    Short roads leave no room to change lanes.
- **Lane drops and zipper:** at a lane drop, both lanes map into the
  remaining lane. Their heads are separate approaches that share a target
  lane, so they conflict and alternate by first-come and the sticky winner.
  This is the zipper (StVO §7(4)) with no extra rule. MOBIL adds no
  "lane ends" incentive before it: drivers use both lanes to the end.
- **Store changes:**
  - `Vehicles::change_lane(id, lane)` inserts at
    `lane.partition_point(|s| pos[s] > pos_m)`;
  - it never reallocates, because `Vehicles::new` reserves each lane for
    `length / 4.5` vehicles, the physical maximum. That bound assumes a
    minimum vehicle length of 4.5 m, so M4 vehicle classes shorter than
    that (bikes, motorbikes) must revisit it;
  - lists stay sorted head first, but the "join at the back, leave at the
    front" rule of ADR-0004 now applies only to road transfers;
  - lane changes keep a lane's order: a vehicle never passes anyone within
    its own lane;
  - the insertion is O(lane).
- **Spawning** stays at lane 0, and MOBIL spreads vehicles out.
  `Router::observe` already averages over all lanes.
- **Before M3.7** the mapping only picks entry lanes; M3.7 enforces
  `from_mask` at the line.

### 6. MOBIL, European (`behavior::mobil`, M3.6)

- **New phase 2b**, *lane changes*, after route updates (2) and before
  intersection decisions (3). Decisions then see each lane's actual head,
  so a vehicle changing lane at the line never inherits another head's
  `go` or stop line. MOBIL computes its IDM terms on the spot from
  start-of-step state, not from the previous step's accelerations. Each
  vehicle is evaluated once per second, when
  `(tick + slot) % (1000 / dt_ms) == 0`.
- **Skipped** during a 3 s cooldown after a change, in the first 10 m of
  a road, and on roads shorter than 20 m. Followers on the previous road
  are not seen yet (`shortcut:`).
- **Decision:** from start-of-step state, using the Treiber–Kesting
  asymmetric criteria:
  - left: `ã_c − a_c + p(ã_n − a_n + ã_o − a_o) > Δa_th + Δa_bias`;
  - right: `ã_c − a_c + p(ã_n − a_n + ã_o − a_o) > Δa_th − Δa_bias`;
  - `n` is the new follower and `o` the old one; politeness sums both, as
    in the book;
  - safety: `ã_n ≥ −b_safe`, and gaps of at least `s0` to both the new
    leader and the new follower. IDM's 1 mm gap floor is not relied on.
- **Defaults:** `p = 0.2`, `Δa_th = 0.1`, `Δa_bias = 0.3` (keep-right
  bias), `b_safe = 4` m/s². They live in a per-vehicle `MobilParams`
  column, set at spawn, ready for M4 profiles.
- **Execution:** sequential, in `(LaneId, head first)` order. The safety
  check is repeated against the state as already changed this step, so
  two vehicles can't both take the same gap. This is deterministic.
- **No overtaking on the right (StVO §7):** on roads with a speed limit
  above 50 km/h (outside towns), phase 4 uses
  `min(a, a_IDM(leader in the lane to the left))` when that leader drives
  faster than 60 km/h and slower than the vehicle. Below that, which
  covers queued and slow traffic and all town roads, passing on the right
  is allowed, as in StVO §7(3).
- **Snapshot:**
  - two new columns, `lc_from: u8` and `lc_ticks: u16` (cooldown), both
    hashed;
  - `VehicleSnapshot` gains `lane_shift: Vec<f32>`, the remaining lateral
    offset in lanes, fading to 0 over 2 s;
  - `Placement::place` takes `lane: f32` (the lane plus its shift), so the
    viewer animates the change while the core stays instantaneous. That
    breaks the Rust API (not the wire protocol): the PR is `feat(api)!`
    with a `BREAKING CHANGE:` footer and updates the M2.7 call sites in
    `rusthour-viz`;
  - above 1×, the viewer draws the latest snapshot without the slide,
    because the 2 s fade is in simulated time.

### 7. Lane choice ahead of junctions (M3.7)

- **Route incentive in MOBIL:**
  - within `max(100 m, 3·v·T)` of the road end, a lane that doesn't serve
    the next movement in `from_mask` gets an incentive penalty that grows
    linearly to `2·b` at the line;
  - when the next road is shorter than 60 m, the penalty uses the movement
    after it;
  - politeness is waived for these mandatory changes; the safety check
    still applies.
- **At the line:** a head whose lane doesn't serve its movement gets no
  `go`.
- **Fallback re-plan:** after 10 s held there, the head re-plans to its
  destination with `Router::route_from_any`. That is a multi-source A*
  that starts from the movements its lane does serve, each with its edge
  cost. So no car waits forever for a gap that never comes, and the
  outcome stays deterministic. At most 4 fallback re-plans run per step;
  due heads past the cap retry next step (`shortcut:`, like the reroute
  cap).

### 8. Gridlock follow-ups (M3.8)

- The ring exemption (§4).
- **`Status`** gains `arrived: u32` and `removed_stuck: u32`, both
  cumulative. `Engine::teleported` is renamed `removed_stuck`.
- **CLI:** the `run` JSON prints both counts and `removed_per_arrival`,
  the gate below, so no script has to compute it.
- **Per-road report:**
  - `Engine` keeps a hashed per-road removal count.
  - The facade exposes it as `Simulation::stuck_by_road_into(&mut Vec<(RoadId, u32)>)`,
    so the viewer can mark hot roads too.
  - The CLI's `--report-stuck` adds `stuck_roads: [{road, name, osm_way, removed}]`
    to the JSON: the ten roads with the most removals. It gives the road id
    and OSM way id, because many roads have no name or share one. This is
    how Berliner Ring is measured.
- **Re-measure after M3.6 and M3.7:** two simulated hours on bensheim-5km,
  seed 42, at 4,000 and 5,000 vehicles, against the M2 trial table.
  - The gate: removals per arrival ≤ 5% at 5,000 (was 7.3%), **and**
    arrivals per hour at 5,000 not below M2.9's 4,545. The second half
    stops the gate from being met by moving fewer cars.
  - Reported without a gate: trip time at 4,000 (was 50–66 min) and
    Berliner Ring's rank in the report.
  - If the gate is missed, smarter recovery gets its own ADR addendum
    before the M3 release.

### 9. Viewer (M3.9)

- **Multi-lane roads:** drawn with dashed lane dividers. Twin roads are
  offset as in ADR-0004.
- **Lane changes:** vehicles slide by `lane_shift` at ≤ 1× (§6).
- **Signals:** a short mark across each signal arm's stop line. The
  colour is never the only cue, and each state has its own shape:
  - red: a solid bar;
  - amber: a short or dashed bar;
  - green: a thin outline, so it doesn't read as "no signal".
  - Above about 30×, signals are drawn in one neutral grey with no
    changes. The phases would otherwise strobe, which is noise and a
    photosensitivity risk (more than 3 flashes a second).
  - A tooltip shows the state and the seconds to the next change.
- **Signs, when zoomed in:** small stop and give-way marks at the road
  end and a `priority_road` mark, so importer mistakes show at a glance.
  Turn-lane arrows at the line, from `turn_lanes` or the default rule,
  are a nice-to-have.
- **Zoom and nodes** (the maintainer's M2 observations, from the backlog):
  - node discs are drawn at a capped screen size, or hidden beyond a zoom
    level;
  - maximum zoom makes a lane at most about 100 px wide, so a vehicle
    never fills the screen;
  - minimum zoom keeps the map in view;
  - a scale bar.
- **Counters:** "arrived" and "removed (stuck)" next to the existing
  counts, the latter tinted when above 0. Roads with removals are marked
  (a tint) from `stuck_by_road_into`.
- **Later:** hovering a vehicle to see its speed, lane and why it waits
  (red, yield, no room) is a nice-to-have for M3.10 or M4.4.

### Determinism, performance, allocation

- **Hashed state:** every new column and every new piece of controller or
  router state is hashed: `held_ticks`, `lc_from`, `lc_ticks`,
  `MobilParams`, delay averages, `stopped`, removal counts.
- **Floats:** turn types, pseudo-angles and conflicts use only `cross`,
  `dot` and comparisons.
- **Scratch buffers:** the lane-change candidate list and the per-lane
  approaches reuse engine scratch buffers.
- **Budget** at 5,000 vehicles, native, on top of today's 0.49 ms on the
  bench grid:

  | Part | Budget |
  | --- | --- |
  | MOBIL (500 evaluations per step at 1 Hz) | ≤ 0.10 ms |
  | No-right-overtaking IDM call (bench target; optimistic with much multi-lane traffic) | ≤ 0.03 ms |
  | Per-lane approaches and conflict bits | ≤ 0.05 ms |
  | Fallback re-plans (at most 4 per step) | within the 0.25 ms routing budget |
  | Delay accounting | ≤ 0.02 ms |
  | **Step total** | **≤ 1 ms** |

- **Benchmark (M3.6):** the M2.8 bench grid gains a variant with two lanes
  per road and signals at every second node, and the allocation test runs
  on it too.

### Work split

Engine edits are confined to named phase functions. Each PR adds its own
phase or hook, which keeps conflicts in `engine.rs` small.

| Wave | Items (parallel within a wave) | Depends on |
| --- | --- | --- |
| 1 | **M3.1** turn costs: `network/movement.rs` (turn types, movement index), router edge costs, `held_ticks`, delay averages; makes `control::priority::rank` public for the yield prior. **Merges first in M3** | ADR |
| 1 | **M3.2** schema 4: api `Road` fields and validation, importer (node tags, sign resolution, `priority_road`, class, `turn:lanes`), core `Road` fields, sample assets rebuilt | ADR |
| 1 | **M3.5a** store: `change_lane` with mid-lane insertion, proptests that lanes stay sorted and every slot is in exactly one lane | ADR |
| 1 | **M3.8a** ring exemption; `arrived` and `removed_stuck` in `Status`, the CLI and the viewer; per-road report | ADR |
| 2 | **M3.3** right of way v2: rank, per-lane approaches, conflict matrix, stop signs | M3.1, M3.2 |
| 2 | **M3.5b** `network/lanes.rs` lane sets and entry lanes; transfers use them | M3.1, M3.5a (turn lanes once M3.2 lands) |
| 2 | **M3.11a** wait-reason diagnostics and `inspect_*` queries (addendum M3.11) | M3.1, M3.8a |
| 3 | **M3.4** `control/signal.rs`, the signal prior in routing, `signals_into` | M3.2, M3.3 |
| 3 | **M3.6** MOBIL phase, European rules, `lane_shift`, multi-lane bench and allocation test | M3.5b |
| 4 | **M3.7** lane choice: route incentive, `from_mask` at the line, `route_from_any` | M3.3, M3.6 |
| 4 | **M3.9** viewer: lanes, lane shift, signal bars, counters | M3.4, M3.6, M3.8a |
| 4 | **M3.11b** viewer: picking, panels, follow, route line (addendum M3.11) | M3.11a; rebases with M3.9 |
| 5 | **M3.8b** re-measure (§8), then the **M3.10** user trial, which covers M3.11 | all |

**Serialization of shared edits:**

- **The schema PR (M3.2)** touches the importer, the api network types and
  the core network. It also touches every test helper that builds a core
  `Road` literal, by adding the new fields. Those helpers are in
  `routing.rs`, `priority.rs`, `engine/tests.rs`, `tests/*.rs` and
  `benches/grid.rs`. Wave-1 PRs that edit the same helpers rebase onto
  whichever merges first; it is a one-line conflict each. After merging,
  run the assets workflow and then the Pages deploy (ADR-0004, schema 3).
- **`vehicle.rs` and `engine.rs`:** M3.1 and M3.5a both edit
  `vehicle.rs`, and M3.1 and M3.8a both edit `engine.rs`. Whichever
  merges later rebases, as for the golden hash.
- **The golden hash** (`tests/determinism.rs`) is expected to change, in
  this order: M3.1, M3.8a, M3.2, M3.3, M3.5b, M3.6, M3.7. M3.4 changes it
  only if it adds a signal to the golden network. As in ADR-0004, whoever
  merges later rebases, regenerates the constant and adds one line to its
  comment; reviewers re-check only that line.
- **`rusthour-api`:**
  - M3.2 edits `network.rs`;
  - M3.8a edits `lib.rs` (`Status`);
  - M3.6 edits `snapshot.rs` and `placement.rs`;
  - M3.4 adds `signals_into` and its types in a new file, after M3.8a.
  - `PROTOCOL_VERSION` becomes 3 with M3.8a; later changes in M3 are
    additive and stay within version 3.

## Alternatives considered

- **Node-expanded routing graph for turn costs:** not needed. Roads are
  already the vertices, so a turn is an edge and its cost is an edge
  weight.
- **Only static turn penalties, or only observed delay:** static ones
  can't see congestion, and observed delay alone starts cold. A prior that
  decays into the measured value has neither problem.
- **Symmetric (US) MOBIL:** wrong for German roads. Keep-right and the
  ban on passing on the right are the maintainer's decision.
- **Continuous lateral movement (sublane):** kept for the maneuvering
  submodel (docs/ideas.md). An instantaneous change with a drawn shift is
  enough for M3.
- **Signs on `Node`:** the direction of a sign is only knowable with the
  OSM way geometry, so the importer resolves it once and the core reads a
  per-road `end_control`.
- **Right of way from `highway` class alone:** wrong where signs exist
  and wrong in 30 km/h zones. Signs and `priority_road` come first, and
  class is only a proxy for signs missing from the map.
- **Separate stop and give-way controllers:** a node mixes signed and
  unsigned arms, so one rule with a rank and a stop flag is simpler than
  combining controllers.
- **Conflicts from lane geometry inside the junction** (SUMO's internal
  lanes): nodes are points here. The cyclic chord test is exact for the
  topology we have.
- **Smarter gridlock recovery now:** measure after lanes, signals and
  turn costs first, since they remove the causes.

## Consequences

- `NETWORK_SCHEMA_VERSION` 4 (M3.2), with the assets rebuilt.
  `PROTOCOL_VERSION` 3 (M3.8a).
- The routing cost changes from the sum of road times to road times plus
  movement costs. M5.5 (imperfect knowledge) reuses both estimates.
- Multi-lane roads gain their real capacity, opposite through traffic no
  longer takes turns, and German priority roads behave as signed.
- Expected: higher throughput at the viewer density and fewer removals.
  The M3.8b numbers decide whether more recovery work is needed.
- Known limit: a priority road that bends at a junction ("abknickende
  Vorfahrt") is treated as two equal roads.
- The vehicle store's "sorted by construction" rule now includes
  mid-lane insertion. Lane order is still never sorted.
- **M3 is renumbered**, turn costs first. ADR-0004 and docs/trials/M2.md
  keep the old numbers, which map to the new ones as follows: MOBIL M3.1 →
  M3.6, stop and yield M3.2 → M3.3, signals M3.3 → M3.4, turn lanes M3.4 →
  M3.7, gridlock M3.5 → M3.8.

## Addendum (M3.11): select, inspect, follow

The maintainer approved M3.11: click a vehicle or road to inspect it, and
follow a vehicle with the camera. It is split into M3.11a (core and api
queries) and M3.11b (the viewer). M4.4 keeps the live parameter panel and
colouring by type.

### Diagnostics in the core (M3.11a)

- **Rule:** diagnostics are written by phases but never read by them, and
  they are not hashed. A test enforces this: it steps a scenario N ticks
  once with an `inspect_vehicle` and `inspect_road` call after every step
  and once without, and requires equal state hashes. M3.11a leaves the
  golden hash unchanged.
- **Stop cause:** the engine keeps a per-lane `stop_cause: Vec<StopCause>`
  next to the existing `stop` scratch. It is sized once like `stop`, one
  byte per lane, with no allocation. Phase 3 writes it at the single place
  `stop` is set, every step:
  - `NoRoom` when the approach is `blocked`;
  - `Junction` otherwise, until a later item can say more.

  M3.3 (`Yield`, `Stop`), M3.4 (`Red`) and M3.7 (`WrongLane`) each set
  their value at their own `go = false` site.
- **`Engine::wait_reason(slot)`** derives the reason from the state after
  the last step, checking in order:
  1. a lane head with `stop` set → `StopLine { cause }`, or `NoRoom`;
  2. a leader (same lane, or ahead across the road end as in phase 4)
     whose IDM interaction term `(s*/s)²` is at least 0.1 → `Following`;
  3. otherwise `FreeRoad`.
  - The cause was decided in phase 3, before integration, so the reason
    can be one step (0.1 s) old.
  - It is meaningful only after the first step, and after any future
    state restore once a step has run. Before that `stop` is all false
    and the answer is `Following` or `FreeRoad`, never uninitialised.
  - Standing time against the stuck limit is a separate field, since a
    vehicle can be both following and nearly stuck.
- **Exit ring:** a fixed ring of the last 256 exits, each a
  `(VehicleId, Gone, road, offset_m)`, allocated at construction and
  written at despawn. `inspect_vehicle` scans it when the id no longer
  names a live vehicle, so at high speed a reused slot still reports
  "arrived" or "removed". `Gone::Unknown` means older than the ring. It is
  unhashed and allocation-free after construction.

### API queries (M3.11a)

The types go in a new `rusthour-api/src/inspect.rs`. The `lib.rs` edit is
limited to the two trait methods and their `pub use` lines.

```rust
// Additions to `Simulation`.
fn inspect_vehicle(&self, id: VehicleId) -> Result<VehicleInfo, Gone>;
fn inspect_road(&self, road: RoadId) -> Option<RoadInfo>; // None: unknown id

#[non_exhaustive] pub enum Gone {
    Arrived { road: RoadId, offset_m: f32 },
    RemovedStuck { road: RoadId, offset_m: f32 },
    Unknown,                                          // older than the exit ring
}
#[non_exhaustive] pub struct VehicleInfo {
    pub id: VehicleId, pub road: RoadId, pub lane: u8,
    pub offset_m: f32, pub speed_mps: f32, pub accel_mps2: f32,
    pub trip_time_s: f32,
    pub route_ahead: Vec<RoadId>,                    // current road first
    pub driver: DriverInfo,                          // IDM now; MOBIL with M3.6; profile with M4.1
    pub wait: WaitReason,
    pub standing_s: f32, pub stuck_limit_s: f32,     // 0 = net off
}
#[non_exhaustive] pub enum WaitReason {
    FreeRoad,
    Following { leader: VehicleId, gap_m: f32 },
    StopLine { cause: StopCause },
    NoRoom { next_road: RoadId },
}
#[non_exhaustive] pub enum StopCause { Junction, Yield, Stop, Red, WrongLane }
#[non_exhaustive] pub struct RoadInfo {
    pub road: RoadId, pub name: Option<String>, pub class: RoadClass,
    pub lanes: u8, pub speed_limit_mps: f32, pub length_m: f32,
    pub vehicles: u32, pub mean_speed_mps: Option<f32>,
    pub removed_stuck: u32,                          // M3.8a's per-road count
    pub movements: Vec<MovementInfo>,
}
#[non_exhaustive] pub struct MovementInfo { pub to_road: RoadId, pub turn: TurnType,
                                            pub delay_s: f32, pub prior_s: f32 } // M3.1
```

- **Static and live data:** names, class, lanes and geometry come from the
  api's own `Network` copy. Live values come from read-only core
  accessors: `Engine::wait_reason`, the exit ring, and
  `Router::movement_delays(from)` for M3.1's averages and priors. The
  queries take `&self`, so they can't mutate anything.
- **Versioning:** the inspect types are not part of the wire protocol
  until M8. `#[non_exhaustive]` keeps growth safe for Rust callers now, as
  M3.3, M3.4 and M3.7 add `StopCause` values and M3.6 and M4.1 add driver
  fields. Wire compatibility for serde clients, including unknown variants,
  is decided and tested with M8.4. So there is **no `PROTOCOL_VERSION`
  bump**: no existing type changes. M3.8a's bump to 3 was right because it
  changed `Status`.
- **Cost:** one vehicle or one road per call. The viewer refreshes at
  about 5 Hz, so the `Vec`s allocated per call don't matter. An `_into`
  shape can come with M8 if a remote panel needs it.

### Viewer (M3.11b)

- **Picking** follows ADR-0003 §4: on the CPU, against data, with no egui
  types in the picking code.
  - Picking uses the positions as drawn (extrapolated, with lane shift)
    and the shifted twin-road lines, so a click lands on what the user
    sees.
  - The radius is 8 px for a mouse and 22 px for touch. Vehicles come
    before roads, and the nearest wins.
  - A repeated tap or click on the same spot cycles through the
    candidates within the radius, nearest first, which handles crowded
    roads without a list.
  - Selection happens on release without movement, so a pan drag never
    selects.
  - Both searches are linear scans: the snapshot for vehicles, the
    network polylines for roads (about 30k segments on bensheim-5km, only
    on a click).
  - Nice-to-have: highlight the candidate under the cursor before the
    click, with a pointer cursor.
- **Selection:**
  - the selected item is outlined, not only coloured (ADR-0003's
    accessibility minimum);
  - the panel has a visible ✕;
  - Esc closes the help window first and clears the selection second;
  - a click on empty map clears it too;
  - Reset, or a change of vehicle count, clears it, since slots mean
    something else afterwards.
- **Keys:**
  - **`C`** and the Follow button toggle follow, and the panel shows
    "following";
  - `F`, `Home` and `0` stay "fit map";
  - `C` is added to the Shortcuts list.
- **Follow:**
  - keeps the selected vehicle centred, at any speed, on its drawn
    position;
  - zooming while following zooms around the vehicle; dragging or panning
    by keys stops following;
  - above about 30× a hint under the speed readout says "follow works best
    below 30×", because the view jumps.
- **Gone vehicles:**
  - the panel says "reached its destination", or "removed after standing
    still 300 s (gridlock safety net)", with the last known road;
  - `Unknown` says "no longer on the map";
  - follow stops, and a marker stays at the last seen position (from the
    exit record, or cached from the previous inspect), because for a
    removal that position is the debugging information;
  - the panel never jumps to the vehicle that reuses the slot.
- **Route:** a dashed line from the vehicle along its route ahead, with a
  destination marker. The panel says "going to" plus the destination road's name.
- **Panel:**
  - top right, about 280 pt wide and collapsible, clear of the control
    strip, the attribution and the F3 window;
  - on narrow screens, a bottom sheet of at most 40% of the height, with a
    one-line summary when collapsed;
  - the text refreshes at about 5 Hz at any speed. `Following` uses
    hysteresis (in at 0.1, out at 0.05) so the reason doesn't flicker.
- **Vehicle panel wording:**

  | Reason | Text |
  | --- | --- |
  | `FreeRoad` | driving freely |
  | `Following` | following traffic, 12 m gap (moving); stuck in a queue (standing) |
  | `Junction` | waiting at a junction |
  | `Yield` | giving way |
  | `Stop` | stopped at a stop sign |
  | `Red` | red light, 12 s (from the signal snapshot) |
  | `WrongLane` | in the wrong lane for its turn, waiting for a gap |
  | `NoRoom` | waiting for space on Hauptstraße ahead |

  It also shows speed, road name and lane, trip time, standing time
  against the stuck limit when above 50%, and the driver parameters.
- **Road panel:** name, class, lanes, speed limit, mean speed, vehicles
  and removals. Each movement reads as "→ Hauptstraße (left): 12 s,
  expected 4 s", the delay against the prior.
- **Files:** the viewer code goes in a new `rusthour-viz/src/inspect.rs`,
  with only hooks in `main.rs` and `world.rs`.

### Work split and serialization

| Wave | Item | Depends on |
| --- | --- | --- |
| 2 (now) | **M3.11a** diagnostics, `wait_reason`, exit ring, `inspect_*` queries, the inspect-does-not-change-the-hash test | M3.1 (merged), M3.8a (per-road removals) |
| 4 | **M3.11b** viewer: picking, panels, follow, route line | M3.11a |

Whoever merges second rebases:

- **M3.3** rewrites `decide()`, where `stop_cause` is written. M3.11a adds
  its write at the single place `stop` is set; M3.3 keeps a cause at each
  `go = false` site.
- **M3.4** also edits `routing.rs` (`Router::movement_delays`).
- **M3.9** edits the viewer; M3.11b is confined to `inspect.rs` plus
  hooks.

The M3 user trial (M3.10) runs after M3.11 and covers it.

## Addendum (M3.3): right of way v2 as built

Three points differ from §1 and §4. The milestone review decides whether
they stand.

- **Impatience:** among heads of equal rank whose movements conflict, once
  either has been arriving for 10 s (`PATIENCE_MS`, a `shortcut:` to tune
  in the M3 trial), the earlier arrival goes, ahead of the left-turn and
  right-before-left rules.
  - *Why:* when only conflicting movements yield, waiting cycles are rarer,
    so the deadlock rule fires less often. A steady stream from the right
    then starves a movement for good. In the saturated 4-way test, a left
    turner merging into the same road as the stream from its right waited
    31 s, against the 20 s limit. With the rule, every arm is served
    within 16–18 s.
  - Higher ranks are unaffected: a major road may still hold a minor one.
- **Conflicts without a bit matrix:** `Priority::new(net, incoming,
  outgoing, patience)` stores one position per road end on the circle
  around the node: per incoming lane in its arm, and per outgoing road.
  The positions follow §1's order. Arms with exactly equal angles also put
  out-points first. The chord test is then four comparisons per pair of
  approaches, so a matrix would add memory without saving work. A target
  road that is not at the node conflicts with everything.
- **`to_lane` is left for M3.5b.** `Approach` gains only `lane`. A target
  lane means something only once M3.5b maps entry lanes, and merges are
  per target road until then (§1).

## Addendum (M3.4): signals as built

These points differ from §2 and §4. The milestone review decides whether
they stand.

- **Signalled on every approach:** on bensheim-5km only 7 of 48 signalled
  junctions have a signal tagged on every approach; most have 1 of 2, 1 of
  3 or 2 of 3. So a node with a signal on any approach is signalled on all
  of them (the architect's suggestion). The exception is a road shorter
  than 30 m from another signalled node: a split-junction link, as in the
  importer (§3). It is a non-signal arm, flows on and yields.
- **Ranks at a signal node:** signal arms rank equal, whatever their signs,
  `priority_road` or class, because a signal overrides them (StVO §37).
  Non-signal arms rank below. So a permissive left turn yields to oncoming
  traffic even from the major road. §4 had kept the road ranks.
- **Amber and red arms are not passed to the inner `Priority`** at all,
  rather than passed as non-competing. Their wait at red then doesn't count
  as an arrival, so impatience can't let a left turner that waited at red
  go ahead of the oncoming stream when green starts. Amber: `go` only if the
  head can't stop and the next road has room. Red: never `go`, even for a
  head released on green; the integration's stop-line clamp holds it.
- **Phases:** grouped as in §4, by opposite arms only. A node whose signal
  arms are all opposite, or that has only one, gets one phase: 25 s green,
  3 s amber and 2 s all-red.
- **Stop cause:** `Red` covers amber too, and comes before `NoRoom`.
- **Routing prior:** `red = cycle − green`, so amber counts as red: 10.2 s
  per signal arm at a two-phase node. A non-signal arm at a signal node gets
  the 4 s yield prior.
- **Snapshot:** `SignalSnapshot { tick, road, state, next_change_s }` as
  parallel columns like `VehicleSnapshot`, with `SignalState` in
  `rusthour-api/src/signal.rs`. It reports the lights the next step
  applies.

## Addendum (M3.6): MOBIL as built

These points differ from, or add to, §6 and the benchmark note. The
milestone review decides whether they stand.

- **The ban on passing on the right also holds inside MOBIL**, not only in
  phase 4. Without it, politeness made a slow car on a 100 km/h road pull
  left so the faster car behind could pass it on the right. So on a road
  outside towns, a change to the left gives the old follower no gain if it
  may not pass the changing car on the right, and a change to the right
  gives the changing car no gain if it may not then pass its old leader.
  Politeness still sums both followers.
- **Lane sets:** MOBIL never moves a vehicle from a lane that serves its
  next movement (`from_mask`, M3.5b) to one that doesn't. Before M3.7 this
  keeps keep-right from pulling left turners off their lane. M3.7's route
  incentive replaces it.
- **Leaders:** the IDM terms use the nearest vehicle in the lane, or the
  tail across the road end as in phase 4. Stop lines are left out on both
  sides of the choice. Phase 4 and MOBIL see only left-lane leaders on the
  same road.
- **"Outside towns"** is a speed limit above 14 m/s, so 13.9 m/s (50 km/h)
  maps count as town.
- **`lc_ticks`** counts steps since the last change, saturating at
  `u16::MAX` ("none yet"); the 3 s cooldown and the 2 s fade both read it.
  Entering a road sets `lc_from` to the new lane, so the shift ends there.
- **`MobilParams`** are the defaults at spawn; `NewVehicle` gains no field
  until M4 profiles need one. `DriverInfo` doesn't show them yet.
- **Bench grid:** the multi-lane variant runs at 70 km/h, so the ban's code
  path is timed; at the M2.8 grid's 50 km/h it would never run.
