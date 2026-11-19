# ADR-0006: Driver profiles (M4.1)

- **Status:** proposed
- **Date:** 2026-11-19
- **Roadmap:** M4.1; M4.3 makes the mix configurable, M4.4 colours by it
- **Builds on:** ADR-0004 §1 (per-vehicle `IdmParams`, RNG streams) and
  ADR-0005 §6 and §7 (MOBIL, `MobilParams`)

## Context

Every vehicle drives the same M2 car: IDM `T = 1.5 s`, `a = 1.5 m/s²`,
`b = 2 m/s²`, `s0 = 2 m`, `v0 =` the speed limit, and MOBIL's defaults.
ADR-0004 already keeps the IDM parameters per vehicle and ADR-0005 the MOBIL
ones, both set at spawn, so a profile only has to choose those values; the
step loop needs no table lookup. The Behavior RNG stream (2) was reserved for
driver noise.

## Decision

### 1. The extension point (`behavior::profile`)

```rust
pub enum Profile { Aggressive, Normal, Cautious, Erratic } // #[default] Normal
pub struct Driver { pub idm: IdmParams, pub mobil: MobilParams }
impl Profile {
    pub fn draw(rng) -> Profile;            // the default mix (§3)
    pub fn base(self) -> Driver;            // without jitter
    pub fn driver(self, rng) -> Driver;     // erratic draws its jitter
    pub fn noise_mps2(self, rng) -> f64;    // 0 unless erratic; draws only then
}
```

- Each profile is a submodule of `behavior/profile/` with its constants
  (and, for erratic, its draws). The `match` arms in `profile.rs` are the
  only shared lines (AGENTS.md §6, ADR-0004 "Extension points").
- The vehicle store gains two columns: `profile: Vec<Profile>` (one byte)
  and `noise_mps2: Vec<f64>`. `NewVehicle` gains `profile` and `mobil`.
  `Engine::spawn(route, params)` keeps its signature and spawns a `Normal`
  driver with those IDM parameters, for tests and hand-placed vehicles.
- The vehicle length stays 4.5 m in every profile; it belongs to vehicle
  classes (M4.2), which will set `IdmParams::length_m` after the profile.

### 2. Parameters

| Profile | Share | v0 factor | T (s) | a (m/s²) | b (m/s²) | s0 (m) | p | Δa_th | Δa_bias | b_safe |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Aggressive | 15% | 1.10 | 1.0 | 2.0 | 3.0 | 1.5 | 0.1 | 0.05 | 0.2 | 5.0 |
| Normal | 60% | 1.00 | 1.5 | 1.5 | 2.0 | 2.0 | 0.2 | 0.1 | 0.3 | 4.0 |
| Cautious | 20% | 0.90 | 2.0 | 1.0 | 1.5 | 3.0 | 0.5 | 0.2 | 0.4 | 3.0 |
| Erratic | 5% | Normal × U(0.8, 1.2), each drawn | ″ | ″ | ″ | ″ | U(0, 0.4) | 0.1 | 0.3 | 4.0 |

- **Normal is today's car**, so the profile mix is the only change in
  behaviour for most vehicles.
- **Literature:**
  - IDM (Treiber, Hennecke and Helbing 2000). Kesting, Treiber and
    Helbing (2010) use `T = 1.5 s`, `s0 = 2 m`, `a = 1.4 m/s²`,
    `b = 2 m/s²` for a typical car, the basis of Normal. Treiber and
    Kesting, *Traffic Flow Dynamics* (2013), ch. 11, give `T = 1 s`,
    `a = 1 m/s²`, `b = 1.5 m/s²` for city traffic and note that `T`
    spans about 0.8–2 s across drivers; Aggressive and Cautious sit at the
    ends of that span.
  - Varying driving style by scaling `T`, `a` and `b` follows Kesting,
    Treiber, Schönhof and Helbing (2008), whose driving strategies are
    factors on those three parameters.
  - MOBIL (Kesting, Treiber and Helbing 2007; the book, ch. 14):
    `Δa_th = 0.1`, `Δa_bias = 0.3`, `b_safe = 4 m/s²` as in ADR-0005;
    politeness `p = 0` is egoistic and the book uses values up to about
    0.5 for realistic drivers. Aggressive drivers are near egoistic and
    accept harder braking behind them; cautious ones are polite and want a
    larger gap.
- **The mean is about today's car:** mean `T` = 1.52 s and mean
  `v0` factor = 0.995, so capacity should move little; §6 measures it.
- **δ stays 4 for everyone.** ADR-0004 computes it as squares, with no
  `powf`, for determinism across targets; a per-profile exponent would
  need `libm` and buys nothing the other parameters don't.
- **Right-of-way patience** (`PATIENCE_MS`, ADR-0005 M3.3 addendum) stays
  per node, not per profile. It is read in `control`, so a per-driver value
  needs a new `Approach` field, a change of `control`'s interface
  (AGENTS.md §7). M4.7 calibrates patience and can add it there.

### 3. Assignment at spawn

- A trip draws its profile from the **Behavior stream** when it joins its
  origin's waiting FIFO: one `next_u32`, mapped to 0–99 by
  `(u · 100) >> 32` and compared with the cumulative shares 15, 75, 95.
  The FIFO holds `(Trip, Profile)`, hashed.
- The entry-gap check uses `base()`, so a cautious car waits for its
  longer gap. When the trip enters, `driver(rng)` gives the final values;
  only erratic draws there (6 draws: five IDM factors and `p`).
- The Demand stream is untouched, so the same seed gives the same trips as
  before M4.1 (common random numbers for before/after comparisons).
- The mix is a constant in `profile.rs` until M4.3 makes it a run setting.

### 4. Erratic noise

- An acceleration offset `noise_mps2`, uniform in ±0.5 m/s², redrawn once
  per simulated second per erratic vehicle, on the same staggered slots as
  MOBIL (`(tick + slot) % (1000 / dt_ms) == 0`). A new phase **3b**
  (driver noise) visits those slots in slot order; non-erratic slots, free
  ones included, draw nothing and keep 0.
- Phase 4 adds it to the IDM acceleration before the no-right-pass and
  merge caps and the −9 m/s² clamp. Integration still clamps every vehicle
  behind its leader and before a stop line, so the noise can't cause an
  overlap.
- **Bounded by construction**, and smaller than the lowest erratic `a`
  (1.2 m/s²), so it never cancels the start-up acceleration on a free road.
- *Literature:* Treiber, Kesting and Helbing (2006) and Treiber and Kesting
  (2018) add a Wiener-process acceleration noise with a correlation time of
  some seconds to IDM. A piecewise-constant uniform offset is the cheapest
  stand-in: one draw per erratic vehicle-second (about 25 per step at
  5,000 vehicles), no state beyond one column. `shortcut:` upgrade to an
  Ornstein–Uhlenbeck process if M4.4's panel or the trial shows the steps.
- Erratic drivers are also inconsistent across vehicles: the parameter
  jitter of §3.

### 5. Determinism and allocation

- All draws come from the Behavior stream in a fixed order: profiles in
  trip order (phase 1), jitter in insertion order (phase 1), noise in slot
  order (phase 3b). Floats come from bits, `(u >> 11) · 2⁻⁵³`, with only
  `+ − × ÷` and `sqrt` (for `√(a·b)`).
- Hashed: the `profile` and `noise_mps2` columns, the waiting trips'
  profiles; the stream's word position already is. The golden hash changes.
- No allocation: the columns grow with the slots like the others; the
  FIFO entries grow from 8 to 12 bytes within the reserved capacity.

### 6. API

- `rusthour_api::DriverProfile` (`Aggressive`, `Normal`, `Cautious`,
  `Erratic`), serde.
- `VehicleSnapshot` gains a `profile` column with `#[serde(default)]`, like
  `lane_shift`: additive on the wire, so **no `PROTOCOL_VERSION` bump**.
- `DriverInfo` gains `profile`. Adding a field to a plain struct breaks Rust
  callers that build it, so the commit is `feat(api)!` as in ADR-0005's
  M3.11 addendum; the viewer's one test literal is updated.

### 7. Measurement

bensheim-5km, seed 42, two simulated hours at 4,000 and 5,000 vehicles,
before and after, and `step/5000` and `step_multilane/5000`. The M3.8 gate
(≤ 5% removed per arrival and ≥ 4,545 arrivals/h at 5,000) must hold. The
results are in the PR and in an addendum below.

## Alternatives considered

- **A profile table looked up per step:** one indirection per vehicle-step
  for nothing; ADR-0004 already chose per-vehicle values.
- **Profile from the Demand stream:** simpler, but every draw would shift
  the trips that follow it, so before/after runs would compare different
  traffic.
- **Drawing the profile when the trip enters:** the entry check needs the
  profile's gap, and a trip that retries for several steps would need the
  draw kept somewhere anyway; the FIFO is that place.
- **White noise every step:** averages out within a second at
  `dt = 0.1 s`, so it would barely show, and costs ten times the draws.
- **Noise for every profile:** turns a 5% effect into a global change of
  the flow; left to calibration if ever wanted.

## Consequences

- The golden hash changes; trips are unchanged for a seed.
- `DriverInfo` construction is a Rust-API break (`feat(api)!`).
- M4.3 replaces the constant mix; M4.2 sets the length after the profile;
  M4.7 may add per-profile patience.

## References

- Treiber, M., Hennecke, A., Helbing, D. (2000). Congested traffic states
  in empirical observations and microscopic simulations. *Phys. Rev. E* 62,
  1805–1824.
- Treiber, M., Kesting, A., Helbing, D. (2006). Delays, inaccuracies and
  anticipation in microscopic traffic models. *Physica A* 360, 71–88.
- Kesting, A., Treiber, M., Helbing, D. (2007). General lane-changing model
  MOBIL for car-following models. *Transp. Res. Rec.* 1999, 86–94.
- Kesting, A., Treiber, M., Schönhof, M., Helbing, D. (2008). Adaptive
  cruise control design for active congestion avoidance. *Transp. Res. C*
  16, 668–683.
- Kesting, A., Treiber, M., Helbing, D. (2010). Enhanced intelligent driver
  model to access the impact of driving strategies on traffic capacity.
  *Phil. Trans. R. Soc. A* 368, 4585–4605.
- Treiber, M., Kesting, A. (2013). *Traffic Flow Dynamics.* Springer,
  ch. 11 (IDM) and ch. 14 (MOBIL).
- Treiber, M., Kesting, A. (2018). The Intelligent Driver Model with
  stochasticity: new insights into traffic flow oscillations. *Transp. Res.
  B* 117, 613–623.

## Addendum (M4.1): as built and measured

- Built as §1–§6 describe.
- **Measured** on bensheim-5km, two simulated hours, `main` (792a5af)
  before:

  | Vehicles, seed | Arrivals/h before → after | Removed per arrival before → after |
  | --- | --- | --- |
  | 4,000, 42 | 6,384 → 6,328 | 0.80% → 0.78% |
  | 5,000, 42 | 6,062 → 5,952 | 1.36% → 1.81% |
  | 5,000, 1 | 5,936 → 5,889 | 1.58% → 1.91% |
  | 5,000, 2 | 6,057 → 6,044 | 1.60% → 1.63% |
  | 5,000, mean of the three | 6,018 → 5,962 | 1.51% → 1.78% |

  The M3.8 gate (≤ 5% and ≥ 4,545 arrivals/h at 5,000) holds. Spread
  headways cost about 1% of arrivals and a quarter of a point of removals
  at 5,000; the removals stay on one-lane residential streets
  (Konrad-Adenauer-Straße, Grieselstraße, Neckarstraße at seed 42).
- **Step cost** on the bench grid (criterion, on a shared two-core machine,
  so ±5% noise): `step/5000` 794 → 777 µs (no change detected);
  `step_multilane/5000` 1.36 → 1.41–1.45 ms over two runs (+3% to +8%).
  Phase 3b visits a tenth of the slots per step; most of the difference is
  the changed traffic, which the bench can't separate from the overhead.

## Addendum (M4.3): the mix as a run setting

- `Config` gains `profile_mix` and, for ADR-0007's classes, `class_mix`:
  relative `u32` weights in `Profile::ALL` and `Class::ALL` order. The
  draw maps one `next_u32` to `0..total` by `(u · total) >> 32` (in
  `u128`), so only the ratios matter: `[15, 60, 20, 5]` and parts per
  million of the same shares draw alike, and the default reproduces the
  golden hash. The weights are not hashed; the profiles and classes drawn
  are.
- `rusthour-api` validates at the boundary: `DriverMix` and `FleetMix`
  hold one `f64` share per name (finite, ≥ 0, sum > 0), normalise them,
  parse `name=share,…`, and give the weights as parts per million;
  `InProcess` rejects an all-zero weight array (`NetworkError::EmptyMix`).
  One field per name, so M5.5's scenario file maps `[drivers]` and
  `[fleet]` tables onto them 1:1.
- The CLI takes `--drivers` and `--fleet` and echoes both mixes; the viewer
  edits them in a folded section and applies them on "Reset".
