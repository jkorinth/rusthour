# ADR-0007: Vehicle classes (M4.2)

- **Status:** accepted
- **Date:** 2026-11-19
- **Roadmap:** M4.2; M4.3 makes the mix configurable, M4.4 colours by it
- **Builds on:** ADR-0004 §1 (per-vehicle `IdmParams`, `length_m`),
  ADR-0005 §5–§6 (lane capacity, MOBIL) and ADR-0006 (driver profiles)

## Context

Every vehicle is a 4.5 m car. ADR-0006 gave drivers a style; the vehicle
they drive is still the same. ADR-0004 already reads the length per vehicle
everywhere it matters: the gap to a leader is to the leader's rear, the
integration clamps each follower behind its leader's rear, a transfer needs
room for the entrant behind the tail's rear, "don't block the box" needs
room for the head's own length, and MOBIL's safety gap to the new follower
is measured from the changing vehicle's rear. So a class only has to set
the values; the step loop needs no new branch except a speed cap.

## Decision

### 1. The extension point (`vehicle::class`)

```rust
pub enum Class { Car, Van, Truck, Bus } // #[default] Car
pub struct Spec {                        // one const per class
    pub length_m: f64, pub width_m: f64,
    pub max_accel_mps2: f64, pub comfort_decel_mps2: f64, // caps
    pub headway_scale: f64, pub min_gap_scale: f64,        // scales
    pub max_speed_mps: f64,                                // cap on v0 and v
    pub keep_right_mps2: f64,                              // added to Δa_bias
}
impl Class {
    pub fn draw(rng) -> Class;              // the default mix (§4)
    pub fn spec(self) -> Spec;
    pub fn apply(self, d: Driver) -> Driver; // the class on top of a profile
}
```

- Each class is a submodule of `vehicle/class/` holding its `SPEC`; the
  `match` in `class.rs` is the only shared line (AGENTS.md §6).
- Classes belong to `vehicle` (the vehicle store and IDM live there);
  profiles stay in `behavior`. `apply` takes and returns ADR-0006's
  `Driver`.
- The store gains one column, `class: Vec<Class>` (one byte), and
  `NewVehicle` a `class` field. `Engine::spawn(route, params)` keeps its
  signature and spawns a car, as it spawns a normal driver.
- `IdmParams` gains `max_speed_mps` and a method
  `v0(limit) = min(desired_speed_factor × limit, max_speed_mps)`, which
  replaces the three places that computed `v0` by hand. Cars get infinity.
- Width is only drawn, so it is not a column: `Spec::width_m` is looked up
  by the api from the class.

### 2. Parameters

| Class | Share | Length (m) | Width (m) | a cap (m/s²) | b cap (m/s²) | T × | s0 × | Speed cap | Keep right (+Δa_bias) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Car | 85% | 4.5 | 1.8 | — | — | 1 | 1 | — | 0 |
| Van | 8% | 5.5 | 2.0 | — | — | 1 | 1 | — | 0 |
| Truck | 5% | 12.0 | 2.55 | 0.7 | 2.0 | 4/3 | 2 | 80 km/h | 0.3 m/s² |
| Bus | 2% | 12.0 | 2.55 | 1.0 | 1.5 | 1 | 1 | 80 km/h | 0.3 m/s² |

- **Car** is today's vehicle, so 85% of traffic is unchanged.
- **Truck:** Kesting, Treiber and Helbing (2009), table 2, give a typical
  truck `v0 = 85 km/h`, `T = 2.0 s`, `s0 = 4 m`, `a = 0.7 m/s²`,
  `b = 2.0 m/s²`, and 12 m length (5 m for cars), against `T = 1.5 s`,
  `s0 = 2 m`, `a = 1.4 m/s²` for their normal car. Treiber's IDM page
  (traffic-simulation.de) likewise uses truck `v0 = 80 km/h`, `T = 1.7 s`
  and `b = 2 m/s²`. The scales 4/3 and 2 turn our normal driver
  (`T = 1.5 s`, `s0 = 2 m`) into exactly table 2's truck.
- **Speed cap 80 km/h** for trucks over 3.5 t and buses: StVO §3(3) Nr. 2
  (80 km/h outside towns and on motorways; 60 km/h for trucks over 7.5 t on
  rural roads and 100 km/h for buses with a permit are not modelled,
  `shortcut:` until roads know their motorway status). EU law fits trucks
  with a 90 km/h speed limiter (Directive 92/6/EEC), so the cap is also
  where real trucks drive.
- **Bus:** a city bus with standing passengers. Karekla et al. (UCL)
  propose 1.0 m/s² as a passenger-safety limit for bus acceleration and
  braking, call more than 2.0 m/s² extremely dangerous for standing
  passengers, and measured at most 1.33 m/s² accelerating and 1.82 m/s²
  braking in service. So `a ≤ 1.0` and a comfortable `b ≤ 1.5`; IDM still
  brakes harder when it must. A bus driver is a professional, so `T` and
  `s0` keep the profile's values.
- **Dimensions:** EU Directive 96/53/EC allows 2.55 m width, 12 m for a
  rigid lorry and 13.5 m for a two-axle bus; the standard city bus is 12 m.
  Light commercial vans (up to 3.5 t) are about 5–6 m long and 2 m wide,
  and drive like cars, so the van differs only in its dimensions. Cars are
  4.5 m long (ADR-0004) and about 1.8 m wide, as the viewer draws them.
- **The mix** is a plausible town mix, not a measured one: about 5% heavy
  vehicles. M4.3 makes it a run setting and M6 calibrates demand.

### 3. Class and profile

The profile describes the driver, the class the vehicle; a trip draws both
independently, so 5% of trucks have an erratic driver.

- **Vehicle limits cap:** `a = min(a_profile, a_cap)`,
  `b = min(b_profile, b_cap)`, `v0 = min(factor × limit, speed cap)`. An
  aggressive driver can't make a truck accelerate like a car.
- **Driving style scales:** `T × headway_scale`, `s0 × min_gap_scale`, so
  an aggressive truck driver still follows closer than a cautious one.
- `√(a·b)` is recomputed after the caps. Erratic jitter is applied before
  the class, so the caps hold for jittered drivers too.
- **The speed cap is hard.** IDM brakes above `v0`, but an erratic driver's
  noise (ADR-0006 §4) could still push a truck a little over it. Phase 4
  therefore caps every acceleration at `(max_speed − v) / dt`, a speed
  governor: one `min` per vehicle-step, a no-op for cars (infinity).

### 4. Assignment at spawn

- A trip draws its class from the **Behavior stream** right after its
  profile, when it joins its origin's FIFO: one `next_u32`, mapped to 0–99
  as in ADR-0006 §3 and compared with the cumulative shares 85, 93, 98.
  The FIFO holds `(Trip, Profile, Class)`, hashed.
- The entry-gap check uses `class.apply(profile.base())`, so a truck waits
  for its longer gap.
- The Demand stream is untouched: the same seed gives the same trips.

### 5. Lanes and MOBIL

- **Trucks and buses keep right:** their `Δa_bias` rises by 0.3 m/s²
  (0.6 for a normal driver), so they pull left only for a clear gain and
  return sooner. The StVO §7(3c) ban on the leftmost of three or more lanes
  outside towns is not modelled (`shortcut:` until the trial shows trucks
  there).
- **A lane-change gap that scales with length:** MOBIL already requires
  `s0` from the new leader's rear to the vehicle's front and from the
  vehicle's rear to the new follower's front (ADR-0005 §6), so a 12 m truck
  needs a slot 7.5 m longer than a car, and its doubled `s0` adds 2 m at
  each end. The new follower's IDM reaction is measured to the truck's
  rear, so `b_safe` sees it too.
- The no-right-pass and cooperative-merge caps read the other vehicle's
  rear, so they already work for long vehicles.

### 6. Queues and space

- Every length check already reads the vehicle's own length (Context).
- **Lane capacity:** `Vehicles::new` reserves each lane for
  `length / 4.5` vehicles, the bumper-to-bumper maximum (ADR-0005 §5).
  Every class is at least 4.5 m long, so no lane queue can outgrow it.
- **Short roads:** a truck may be longer than the road it is on (the
  shortest links are about 10 m). Positions are front bumpers and the
  model has no "rear still on the previous road", so the truck just counts
  as on the new road; the room check at the previous node still needed its
  full length. `shortcut:` model occupancy of two roads if the viewer or
  the trial shows trucks overhanging junctions.

### 7. Buses

Only a class for now: buses draw random origin–destination trips like
every vehicle, with no stops, lines, timetables or depots. Bus stops and
lines are demand work for later (M6 and beyond).

### 8. Determinism and allocation

- One more draw per trip, from the Behavior stream in trip order. No new
  float operations beyond `min` and `×`.
- Hashed: the `class` column, `max_speed_mps` in `IdmParams`, the waiting
  trips' classes. The golden hash changes.
- No allocation: one more column that grows with the slots, and FIFO
  entries from 12 to 13 bytes (16 with padding) within the reserved
  capacity.

### 9. API and viewer

- `rusthour_api::VehicleClass` (`Car`, `Van`, `Truck`, `Bus`), serde, with
  `width_m()`.
- `VehicleSnapshot` gains a `class` column with `#[serde(default)]`, like
  `profile`: additive on the wire, so **no `PROTOCOL_VERSION` bump**.
  `length_m` now varies by class.
- `VehicleInfo` gains `class`; adding a field to a plain struct breaks Rust
  callers that build it, so the commit is `feat(api)!` as in ADR-0006 §6.
- The viewer already draws each vehicle with its snapshot `length_m`; it
  now also takes the width from the class, with the car's 1.8 m when the
  column is empty. Picking and the selection outline keep the car width
  (`shortcut:`, 0.4 m on each side at most). Colour by class is M4.4.

### 10. Measurement

bensheim-5km, seed 42, two simulated hours at 4,000 and 5,000 vehicles,
before and after, and `step_multilane/5000`. The M3.8 gate (≤ 5% removed
per arrival and ≥ 4,545 arrivals/h at 5,000) must hold. The results are in
the PR and in an addendum below.

## Alternatives considered

- **A class table looked up per step:** as in ADR-0006, per-vehicle values
  set at spawn cost nothing in the step.
- **Class overrides the profile entirely:** every truck would drive alike,
  losing the profile mix for 15% of vehicles; capping vehicle limits and
  scaling style keeps both.
- **Speed cap in `v0` only:** simpler, but not a guarantee with erratic
  noise; the governor costs one `min`.
- **A width column:** width only matters to the viewer, and the class
  already tells it.
- **Class from the Demand stream:** would shift the trips, as ADR-0006
  rejected for profiles.

## Consequences

- The golden hash changes; trips are unchanged for a seed.
- `VehicleInfo` construction is a Rust-API break (`feat(api)!`).
- Heavier vehicles take more room in queues and accelerate slower, so
  junction capacity drops a little; §10 measures it.
- M4.3 replaces the constant mix; M4.4 colours by class.

## References

- Kesting, A., Treiber, M., Helbing, D. (2009). Agents for traffic
  simulation. In *Multi-Agent Systems: Simulation and Applications*, CRC
  Press, 325–356. arXiv:0805.0300, table 2.
- Treiber, M. The Intelligent-Driver Model and its variants,
  traffic-simulation.de/info/info_IDM.html, parameter table.
- Treiber, M., Kesting, A. (2013). *Traffic Flow Dynamics.* Springer,
  ch. 11 (IDM) and ch. 14 (MOBIL).
- Karekla, X., et al. The impact of a passenger-safety-driven acceleration
  limit on the operation of a bus service. UCL Discovery 10120841 (accepted
  manuscript).
- Straßenverkehrs-Ordnung (StVO) §3(3) and §7(3c).
- Council Directive 92/6/EEC on speed limitation devices; Council
  Directive 96/53/EC on maximum dimensions and weights.

## Addendum (M4.2): as built and measured

- Built as §1–§9 describe. The lane-capacity bound of §6 is a compile-time
  check: every class's `SPEC` must be at least 4.5 m long.
- **Measured** on bensheim-5km, two simulated hours, `main` after M4.1
  before:

  | Vehicles, seed | Arrivals/h before → after | Removed per arrival before → after |
  | --- | --- | --- |
  | 4,000, 42 | 6,328 → 5,685 | 0.78% → 0.55% |
  | 5,000, 42 | 5,952 → 5,379 | 1.81% → 1.52% |
  | 5,000, 1 | 5,889 → 5,277 | 1.91% → 1.61% |
  | 5,000, 2 | 6,044 → 5,479 | 1.63% → 1.42% |

  The M3.8 gate (≤ 5% and ≥ 4,545 arrivals/h at 5,000) holds. Removals
  fall rather than rise; they stay on one-lane residential streets
  (Mühlenweg, Hermannstraße, Elisabethenstraße, Neckarstraße at seed 42),
  as before M4.2.
- **Arrivals fall by about 10%** at both counts: the mean time on the
  network (Little's law) grows from 50.4 to 55.8 minutes at 5,000. Taken
  apart at 5,000, seed 42:

  | Variant | Arrivals/h | Removed per arrival |
  | --- | --- | --- |
  | Before | 5,952 | 1.81% |
  | Trucks and buses 4.5 m, with their limits | 5,635 | 1.81% |
  | Trucks and buses 12 m, driving like cars | 5,540 | 1.32% |
  | Both (as built) | 5,379 | 1.52% |

  Length and limits each cost about 5–7%. The network runs near capacity
  at these counts, so the space a long vehicle takes in a queue and the
  time a slow one takes to clear a junction both show up directly. For
  comparison, the Highway Capacity Manual counts a heavy vehicle as two
  passenger cars at signals (`E_T = 2.0`), which for 7% heavy vehicles
  lowers saturation flow by 6.5%, before any effect on speed.
- **Step cost** on the bench grid (criterion, shared two-core machine):
  `step/5000` 782 → 780 µs (no change detected); `step_multilane/5000`
  1.36 → 1.32 ms (−4%, changed traffic). The governor's `min` is not
  measurable.

## Addendum (M4.12): trial fixes, truck gridlock and arrival rate

These points change §4 and §6, ADR-0005 §4 and the M4.7 addendum's turn
priors. Accepted in the M4 review, with the M4.14 room-check fix.

- **Why trucks were removed** (M4 trial, finding 4). Traced on
  bensheim-5km with 500 and 1,000 trucks; none of the suspects in the
  vehicle itself (a road shorter than the truck, MOBIL gaps, lane capacity)
  was the cause. Two entry rules let a waiting head starve:
  - *Higher rank in a jam.* On Fichtestraße (road 1566, a 7 km/h living
    street) the head waited for room in the exit road together with the
    queued heads of two higher-ranked roads. Whenever room opened, a major
    head standing at its line took it; patience (M3.3) only applied among
    equal ranks, so the minor head never went, and the trucks behind it on
    roads 1568 and 1570 were removed.
  - *New trips first.* On Konrad-Adenauer-Straße (road 2134) a truck waited
    for room on road 2136 while trips starting on 2136 kept entering in
    front of it: a trip needs a gap of `s0 + v·T` to the tail, a waiting
    truck the room for its 12 m.
  Trucks meet both more often than cars because room for 12 m opens rarely
  in a slow queue. The rest of the 4,000-truck gridlock is capacity: a truck
  takes about 2.5–3 cars' space and time at a junction (below).
- **Fix 1, waiving priority in a jam (`control::priority`):** a head that
  has waited longer than the patience (10 s) goes first-come before a
  higher-ranked head that crawls below 10 km/h, as StVO §11(3) asks of a
  driver with priority when the traffic situation requires it. Not before
  a head circulating on a roundabout, whose ring would lock. A moving
  major stream keeps its priority, so gap acceptance at low load is
  unchanged.
- **Fix 2, new trips yield (`Engine::spawn_trips`, StVO §10):** a trip
  doesn't enter lane 0 of its origin road while a lane head about to enter
  that lane from the node behind would be left less than its own
  `s0 + v·T` behind the new vehicle's rear. `shortcut:` only lane heads are
  checked. `Engine::spawn` keeps inserting without it, for tests and
  benches. At 5,000 vehicles about 110 trips wait at a time (about 5
  before). The free-slot list reserves room for every slot, so `despawn`
  never reallocates when more trips wait at once.
- **Why trucks arrive at about half the rate of cars** (finding 5): not
  routing and not the 80 km/h cap. With 500 vehicles, seeds 42 and 1,
  arrivals per hour by variant of the truck (`main` with fixes 1 and 2):

  | Variant | Arrivals/h |
  | --- | ---: |
  | Cars | 4,289 |
  | Trucks | 2,234 |
  | Trucks 4.5 m long | 3,060 |
  | Trucks without the 0.7 m/s² cap | 2,956 |
  | Trucks with the car's `T` and `s0` | 2,622 |
  | Trucks without the 2 m/s² `b` cap | 2,290 |
  | Trucks with all four as a car (cap and keep-right left) | 4,239 |

  So the speed cap and keep-right cost about 1%: no road above 80 km/h
  carries traffic here, and routes planned at car speed would only differ
  on such roads. 500 vehicles aren't low density for trucks: the ratio is
  0.93 at 50 vehicles and 0.81 at 200. 500 trucks have the trip time of
  about 1,400 cars, a passenger-car equivalent of about 2.8, which fits
  their length (16 m against 6.5 m per vehicle in a queue) and their slow
  start at every junction (the HCM counts 2.0).
- **Fix 3, the room check counts on a departing tail (`engine::no_room`):**
  one part of finding 5 is an artifact. A head was told to stop until the
  next road's tail had moved the head's whole length onto that road, even
  while the tail drove away, so a truck following a truck across a junction
  braked until the leader's rear was 12 m in and then took a long time to
  regain speed. The tail's rear now counts as driving on while the head
  covers its distance to the line, in proportion to the two speeds and
  never further than that distance; a standing tail still needs the room
  now. If the tail stops after a head has committed, the head enters and
  stands with its rear across the junction; the model doesn't block
  crossing traffic for that, so it costs nothing but realism. At 500 trucks
  the time held for room fell from 12.6% to 6.3% of the
  vehicle-time, for cars from 4.5% to 0.8%.
  - *Turn priors:* turns that merge into a stream were held for room most,
    so the delays routing observes fell. On the M4.7 cross at 300 veh/h a
    right turn with priority is now never held (0.6 s before) and other
    turns 0.1–1.0 s less. The turn part of the prior drops from 1 to 0.5 s
    for right and from 2 to 1.5 s for left turns; the other priors stay.
- **Measured** on bensheim-5km, two simulated hours (`main` before):

  | Run | Arrivals/h before → after | Removed per arrival before → after |
  | --- | --- | --- |
  | Default mix, 5,000, seed 42 | 5,306 → 5,700 | 1.24% → 0.44% |
  | Default mix, 5,000, seed 1 | 5,305 → 5,732 | 1.36% → 0.32% |
  | Default mix, 5,000, seed 2 | 5,284 → 5,750 | 1.53% → 0.56% |
  | Default mix, 5,000, seed 3 | 5,458 → 5,827 | 1.52% → 0.62% |
  | Default mix, 5,000, mean | 5,338 → 5,752 | 1.41% → 0.48% |
  | Trucks, 4,000, seed 42 | 2,024 → 2,402 | 44.7% → 4.6% |
  | Trucks, 2,000, seed 42 | 2,595 → 2,776 | 1.95% → 0% |
  | Trucks, 1,000, seeds 42 and 1–3 | 2,558 → 2,694 | 21 → 0 removed |
  | Trucks, 500, seeds 42 and 1–3 | 2,305 → 2,439 | 2 → 0 removed |
  | Cars, 500, seed 42 | 4,300 → 4,397 | 0 → 0 |
  | Buses, 500, seed 42 | 2,956 → 3,137 | 0 → 0 |

  The M3.8 gate (at most 5% removed, at least 4,545 arrivals/h at 5,000)
  holds. Each fix alone, from the runs before fix 3 (default mix, 5,000,
  seeds 42 and 1–3): fix 1 alone 5,168 arrivals/h and 0.77% removed, fixes
  1 and 2 5,384 and 0.71%; fix 2 alone doubled the removals at 1,000
  trucks, because more trucks then reached the starving merges. Removals at
  5,000 now lead on Starkenburgstraße. The first 30 simulated minutes at
  5,000 vehicles take about 5% longer (13.6 against 13.0 s), with more
  vehicles moving. The golden hash changes.
- **Room-check fix (M4.14, M4 architect review B1):** as first built, fix 3
  counted a standing tail as having moved the head's whole distance to the
  line when the head stood too (`vt >= v` with both 0), so a head held at
  the line got room on every other tick, competed for right of way, and
  crept towards the junction. Now the tail counts only while it drives
  (at least 0.1 m/s, the engine's standing threshold), which is what the
  text above says. Re-measured on bensheim-5km, two simulated hours
  (`main` with M4.12 before, reproducing the table above):

  | Run | Arrivals/h before → after | Removed per arrival before → after |
  | --- | --- | --- |
  | Default mix, 5,000, seed 42 | 5,700 → 5,671 | 0.44% → 0.33% |
  | Default mix, 5,000, seed 1 | 5,732 → 5,542 | 0.32% → 0.53% |
  | Default mix, 5,000, seed 2 | 5,750 → 5,788 | 0.56% → 0.47% |
  | Default mix, 5,000, seed 3 | 5,827 → 5,747 | 0.62% → 0.65% |
  | Default mix, 5,000, mean | 5,752 → 5,687 | 0.48% → 0.49% |
  | Trucks, 4,000, seed 42 | 2,402 → 2,304 | 4.6% → 4.5% |
  | Trucks, 1,000, seeds 42 and 1–3 | 2,694 → 2,707 | 0 → 0 removed |

  The M3.8 gate still holds, and trucks don't gridlock again. Arrivals at
  5,000 fall by about 1% and at 4,000 trucks by 4%: heads now wait for
  room they were wrongly given. The golden hash changes.
- **Proposed, not done:**
  - *Trucks avoid living streets and access roads.* Realistic: such streets
    are often signed against heavy goods vehicles (Zeichen 253, `hgv=no` in
    OSM, which the importer doesn't read). Cheap in A\*: a class-dependent
    penalty only raises costs, so the landmark bound stays admissible and
    routes stay deterministic. It needs a class argument to `Router::route`
    and a test of where destinations lie on such streets; a backlog item.
  - *Routing with the class's speed cap:* also cheap and exact with the
    current landmark tables (`max(travel_s, length / cap)` is never below
    the costs they were built on), but it changes nothing here (about 1%,
    above). A backlog item for maps with motorways.
