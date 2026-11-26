# ADR-0009: Roundabout entries with gap acceptance (M5.2)

- **Status:** proposed
- **Date:** 2026-11-26
- **Roadmap:** M5.2; M4.6 lands first (PR #109), and M5.3 builds
  roundabouts with this controller
- **Builds on:** ADR-0004 (`NodeControl`, determinism), ADR-0005 §4 (the
  priority rule, circulating rank, the ring exemption, impatience, the M4.7
  calibration), ADR-0006 (driver profiles), ADR-0007 addendum M4.12 (the
  jam waiver) and ADR-0008 §8 (roundabouts made by edits)

## Context

A roundabout has so far been a ring of roads flagged `roundabout`, with entries
ending in give-way. Ring roads rank highest in the priority rule (ADR-0005 §4).
An entry head yields only to ring heads at its own node that are `arriving`,
that is, within `3 s · v + 2·s0` of the line. On bensheim-5km a ring road
between an exit node and the next entry node is 8 to 25 m long. So an entry
sees a circulating car about one second before it arrives. A car one ring road
further back is invisible, and so is a car that will leave the ring before the
entry.

Field models of roundabout entries are gap-acceptance models. A driver enters
when the time to the next circulating vehicle, the lag, is at least the
critical gap `tc`. Drivers behind follow at the follow-up time `tf`. Entry
capacity then falls exponentially with the circulating flow `vc`:

| Reference | Capacity of a single-lane entry (pc/h) | Implied `tc`, `tf` (Siegloch) |
| --- | --- | --- |
| HCM 2010, exhibit 21-7 | `1130·e^(−0.001·vc)` | 5.2 s, 3.2 s |
| HCM 2016 (6th ed.), eq. 22-1 | `1380·e^(−0.00102·vc)` | 5.0 s, 2.6 s |
| HBS 2015 / Brilon and Wu, single lane | basic capacity 1,200 veh/h; Wu's formula with a minimum headway `Δ = 1.8 + 14.5/D` s | `tf` 3.0 s |

The task asks for `tc` ≈ 4–5 s and `tf` ≈ 2.5–3 s, adjusted per driver.

## Decision

### 1. The controller (`control::roundabout`)

- **Selection:** a node whose incoming roads include exactly one `roundabout`
  road and at least one other road gets `NodeControl::Roundabout`, unless it
  is signalled (signals win, as before). Other ring nodes, such as exit nodes,
  keep `Priority`.
- **Structure:** like `Signal`, `Roundabout` wraps an inner `Priority` for
  the node, so that rule still covers several things:
  - ranks (circulating traffic first);
  - entry against entry at one node;
  - stop signs;
  - commitment (`go && !can_stop`);
  - the deadlock rule;
  - don't block the box (`blocked`).

  A gate on top decides the entry heads into the ring road out of the node.
- **Constructor:** `Roundabout::new(net, ring, k, incoming, outgoing,
  patience)`, where `ring` holds the ring's road ids in driving order and
  `ring[k]` ends at this node. The ring nodes follow from the roads.
  `roundabout::ring_into(net, in_first, incoming, node)` finds the ring for
  imported maps. It walks upstream while each ring node has a single incoming
  ring road, so a ring the extract cuts open becomes an open chain, which
  `new` accepts. M5.3 passes the ring roads it creates (ADR-0008 §8) and
  flags them `roundabout`, so `ring_into` finds them as well.
- **The interface change (AGENTS.md §7):** `Controller::decide` gains
  `vs: &Vehicles`, the vehicle state at the start of phase 3. The gap needs
  vehicles beyond the lane heads. `Priority` ignores it. `Signal` passes it
  to its inner `Priority`. M4.6 (#109) had already moved `holds_at_red` and
  `holds_at_stop_sign` into the trait, and `NodeControl` dispatches through
  one `each!` macro, so the new variant is one line in the enum and one in
  the macro.

### 2. The gap model

- **The lag** is the time until the first vehicle that circulates past this
  node reaches it. The gate looks back along the ring for up to 150 m (11 s
  at 50 km/h) and counts:
  - vehicles on the upstream ring roads, every lane;
  - vehicles on entries at upstream ring nodes that can no longer stop
    (`dist < v²/2b`). An entrant that can still stop may yield, and so may
    those behind it. Without this, on a ring with entries 20 m apart, a car
    entering upstream appeared 2.8 s from the conflict point, after the
    downstream entry had accepted its gap.

  A vehicle counts only if its route continues along the ring roads to this
  node and then onto the ring road out of it.
- **Arrival time:** the earliest time to cover the distance, accelerating at
  the vehicle's `a` up to its desired speed on the ring. A standing ring car
  10 m away is about 3.6 s off, so an entry doesn't cut in front of a queue
  that is about to move. A vehicle faster than the ring's speed is timed at
  that speed, because it brakes for the ring. Using its current speed
  understated the gaps behind cars entering from 50 km/h arms by up to 30%.
- **The entry head** goes if `lag − t_line ≥ tc`. `t_line` is the head's own
  earliest arrival at the line, and standing within `2·s0` of it counts as
  being at the line. The engine stops heads `s0` short of the road end.
  Counting that last 2 m added 1.6 s to every critical gap.
- **Commitment:** once a head has had `go`, it keeps it, needing only room.
  That overrides the inner rule's yield to an arriving ring head, which the
  driver saw coming when it accepted the gap. Without this, an entrant that
  had accepted a 4.5 s lag was stopped again 1.5 s later, within the 3 s
  window of the inner rule, and the gap was lost.
- **No explicit follow-up time.** A gate that let a lane's next head go only
  `tf` after the previous entry made queues stop and restart. Zero-flow
  capacity fell from 1,328 to 831 veh/h. `tf` therefore emerges from car
  following, and per profile through `T`, `a` and `s0` (§4).

### 3. Exits

- **Not a conflict:** a vehicle that leaves the ring before this node, or at
  it, doesn't count. Its route doesn't continue onto the ring road out of the
  node.
- **Same-node entries and exits:** the inner `Priority` handles the
  geometry of an exit at the entry's own node with its chord test
  (ADR-0005 §1).
- **Test:** a car 10 m before the exit node at 8 m/s doesn't block an entry
  10 m further on if it exits there. If it stays on the ring, 2.5 s away, it
  does.

### 4. Per-profile parameters

`Driver` gains `gap: Gap { critical_s }`, set by each profile module.
`Vehicles` stores it per slot (hashed), and `NewVehicle` carries it.

| Profile | `tc` (s) | Emergent `tf` from a standing queue (s) | Zero-flow capacity (veh/h) |
| --- | ---: | ---: | ---: |
| Aggressive | 3.8 | about 2.8 (`T` = 1.0 s) | 2,045 |
| Normal | 4.5 | about 3.7 (`T` = 1.5 s) | 1,466 |
| Cautious | 5.3 | about 4.5 (`T` = 2.0 s) | 1,100 |
| Erratic | normal × U(0.8, 1.2), one draw | like normal, jittered | 1,403 |

- **Normal:** 4.5 s lies within the 4–5 s of the brief, between the
  HBS-style values and the HCM's 5.0–5.2 s.
- **Spread:** the ±0.7–0.8 s for aggressive and cautious drivers is an
  assumption, about the spread of critical gaps between drivers. It is not
  taken from a source.
- **Erratic:** the draw comes after the five IDM factors and politeness,
  from the Behavior stream, so the earlier draws are unchanged.
- **Vehicle classes:** a class doesn't change the gap (`shortcut:`). Trucks
  start more slowly, which `t_line` and car following already reflect.
- **Spawning by profile:** `Engine::spawn_profile(route, profile)` spawns a
  car with a given profile, for tests and studies of driver mixes.
  `Engine::spawn` still spawns a normal driver.

### 5. Multi-lane roundabouts

`shortcut:` single-lane first. On a multi-lane ring, every lane's vehicles
count against every entry lane, as if each conflicted. Every entry lane is
gated the same way. This is conservative: real two-lane rings let a
right-lane entry go beside inner-lane traffic. bensheim-5km has single-lane
rings only. Per-lane conflicts come when a map needs them.

### 6. Guarantees kept

- **Ring exemption (M3.8):** unchanged in `engine::blocked`. Ring heads
  continuing on the ring are never `blocked`, and the gate never applies to
  them.
- **Don't block the box:** a blocked entry head never gets `go`, even once
  committed.
- **Jam waiver (M4.12):** unchanged. It never waives a ring head's priority.
- **Liveness:** a property test drives 300–2,000 veh/h into each of four
  arms, with random profiles and routes, for five minutes. No circulating
  vehicle stands for 20 s, and vehicles keep leaving: at most 60 s pass
  without an arrival.

### 7. Determinism

- **No new randomness except the erratic factor.** The gate is a function
  of the state at the start of phase 3.
- **Hashed state:** the per-lane head and `went` flags, and each slot's `tc`.
- **Golden hash:** it changes.
- **API test:** `arrivals_and_per_road_removals_match_the_status` relied on
  seed 7 meeting a one-arm signal (5 s of red) standing. The extra draw shifts
  the Behavior stream, and seed 2 now does.

## Calibration

Measured on a synthetic single-lane roundabout (`tests/roundabout.rs`):

- **Geometry:** four arms, a 15 m ring radius and 50 km/h on the ring, as
  bensheim-5km's rings are imported. Each arm has its exit node 15° before
  its entry node, and the arms are 200 m long.
- **Traffic:** a queue of the profile's drivers always waits at arm 0.
  Normal cars from arm 3 circulate past it as a Poisson stream, at the flow
  measured in front of the entry.
- **Run length:** two simulated hours after five minutes of warm-up.

Normal drivers, veh/h:

| `vc` measured | `main` (priority rule) | M5.2 | HCM 2010 | HCM 2016 |
| ---: | ---: | ---: | ---: | ---: |
| 0 | 1,466 | 1,466 | 1,130 | 1,380 |
| 156 | 976 | 901 | 967 | 1,177 |
| 301 | 716 | 659 | 836 | 1,015 |
| 584 | 415 | 383 | 630 | 761 |
| 884 | 210 | 190 | 467 | 560 |
| 1,173 | 52 | 69 | 350 | 417 |

By profile at the same flows (veh/h):

| `vc` | Aggressive | Normal | Cautious | Erratic |
| ---: | ---: | ---: | ---: | ---: |
| 156 | 1,246 | 901 | 682 | 901 |
| 301 | 856 | 659 | 492 | 651 |
| 584 | 484 | 383 | 278 | 378 |
| 884 | 243 | 190 | 141 | 186 |

- **Within 7–21% of HCM 2010 up to about 300 veh/h, too low above:** the
  test checks the curve within ±25% at 150 and 300 veh/h. Above that the
  model falls below every reference: 61% of HCM 2010 at 584 veh/h and 41%
  at 884.
- **The cause is not the gate.** `main`'s rule, which hardly yields, is only
  8% above M5.2.
  - *Earlier runs with a 30 km/h ring* showed the same: lowering every `tc`
    by 2 s raised capacity by 2–5%, and with neither the gate nor the room
    check, the entry took 344 veh/h at 476 veh/h circulating.
- **The engine's queue discharge sets the limit.** A car that stood at the
  line crosses it, 2 m later, at only 2–4 m/s. IDM's headway at that speed,
  `T + (s0 + l)/v`, is about 3.7 s for a normal driver, against the field's
  2.6–3.2 s.
  - The ring car behind an entrant then brakes to that speed. The ring
    becomes a slow platoon (3–4 m/s at 584 veh/h in the trace), and its
    headways shrink below any critical gap.
  - Counted per circulating gap, an entry needed a gap of about 6.5 s, and
    each further entry about 3.7 s more. That is the effective `tc` and `tf`.
  - The same discharge applies at every give-way and signal from standstill.
    It is an engine property, out of M5.2's scope, and is proposed as a
    roadmap item below.
- **What the gate changes:**
  - Entries now wait for vehicles beyond the ring head.
  - They ignore vehicles that exit before them.
  - They differ by driver: aggressive drivers enter 1.74 times as often as
    cautious ones at 584 veh/h (the test requires more than 1.3 times).

## Measured on bensheim-5km

5,000 vehicles, two simulated hours, the default driver and fleet mix.
"Before" is `main` at 85235e2, which includes M5.10.

| Seed | Arrivals/h before → after | Removed per arrival before → after |
| --- | --- | --- |
| 42 | 5,670 → 5,666 | 0.33% → 0.56% |
| 1 | 5,542 → 5,586 | 0.53% → 0.40% |
| 2 | 5,788 → 5,766 | 0.47% → 0.67% |
| 3 | 5,746 → 5,684 | 0.65% → 0.43% |
| Mean | 5,687 → 5,676 | 0.49% → 0.51% |

- **Gate and cost:** the M3.8 gate (at most 5% removed, at least 4,545
  arrivals/h) holds, and the changes are within the spread between seeds.
- **Speed:** both builds take 30.5–30.9 s for one simulated hour when run
  side by side.
- **Entry delay:** the time a vehicle spends on an entry road before it
  enters the ring, minus that road's free-flow time, so queueing on the
  approach counts. Averaged over all entries of the four seeds. A scratch
  harness on the API's snapshots measured it each step. The roundabouts are
  named by their ring way and one entry road; the first four are the
  Berliner Ring roundabouts of the M2 trial.

| Ring way (an entry road) | Entries per run | Delay before → after (s) |
| --- | ---: | ---: |
| 157846715 (Berliner Ring) | 1,494 → 1,490 | 1.1 → 1.0 |
| 187276521 (Europa-Allee) | 2,652 → 2,556 | 27.4 → 30.5 |
| 246950232 (Berliner Ring) | 2,119 → 2,109 | 12.3 → 10.1 |
| 679409465 (Schwanheimer Straße) | 2,636 → 2,564 | 24.4 → 25.9 |
| 263899015 (An der Hartbrücke) | 846 → 845 | 10.3 → 12.4 |
| 8255122 (Heidelberger Straße) | 1,872 → 1,794 | 4.3 → 5.9 |

- **Busy roundabouts:** entries wait up to 3 s more on average. Up to 4% fewer
  entries pass at the Europa-Allee and Schwanheimer Straße roundabouts, whose
  rings carry the most traffic. This fits the synthetic result: the gate
  makes entries slightly more conservative than the plain priority rule.
- **Quiet roundabouts:** 157846715 hardly changes.
- **246950232:** its delay falls by 2 s. Not traced further.
- **Removals:** the CLI's `--report-stuck` lists the ten roads with the most
  removals.
  - No ring road appears in any run.
  - Before, no roundabout entry appears.
  - After, road 2330 (way 333360023, an entry of 187276521) has 3 of
    seed 2's 77 removals. Those cars stood 300 s in a queue at the busiest
    ring.
- **Follow-up (`shortcut:`), impatience at entries:** M4.7 cites drivers
  accepting shorter gaps the longer they wait (Mahmassani and Sheffi,
  1981). A `tc` that shrinks with the wait would stop such starvation.

## Alternatives considered

- **A separate controller with its own rank logic:** it would duplicate the
  priority rule's commitment, deadlock and stop-sign logic. Wrapping a
  `Priority`, as `Signal` does, reuses them.
- **Deriving `tc` from each driver's `T`:** no new column or draw, but it
  would tie the gap to car-following tuning (M4.4), and aggressive drivers'
  gaps would be a third shorter. Rejected for an explicit per-profile value.
- **Gating the follow-up time:** measured above. It caused stop-and-go and
  cut zero-flow capacity by 37%.
- **Lowering `tc` until the curve fits:** it doesn't fit at any `tc` (see
  Calibration). The values stay at the literature's.
- **Passing circulating vehicles as extra approaches** instead of `&Vehicles`:
  the engine would have to know which roads a controller watches. One
  read-only argument is simpler.

## Consequences

- Entries at the six bensheim-5km roundabouts, and at those M5.3 creates,
  accept gaps per driver. The roundabout rows in routing's priors are
  unchanged (`shortcut:` re-fit them when M5.6's metrics measure entry
  delays).
- **Proposed roadmap item:** queue discharge from standstill. A car that
  stood at the line crosses it at 2–4 m/s, which gives about 3.7 s per
  vehicle against the field's 2–3 s. That caps roundabout entry capacity
  at half the HCM's at moderate circulating flow, and slows every give-way
  and signal queue. Candidates:
  - stopping heads at the line instead of `s0` before it;
  - a start-up acceleration;
  - a lower `T` at low speed.

  The M4.7 calibration needs re-running afterwards.
