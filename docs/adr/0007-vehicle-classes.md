# ADR-0007: Vehicle classes (M4.2)

- **Status:** proposed
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
