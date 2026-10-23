# Ideas and future work

Ideas worth keeping that are not scheduled yet. When one is picked up, it
becomes a roadmap item, with an ADR if it changes the design. The short
backlog in [ROADMAP.md](../ROADMAP.md) §7 lists the rest.

## Detailed maneuvering submodel

Vehicles today move along a road in one lane: position is
`(road, lane, offset_m)` (ADR-0004). Lane changes come with MOBIL (M3) and
route updates with M5.5. A maneuvering submodel would add the moves real
drivers make when the plain lane model doesn't fit, as a submodule of
`behavior` implementing its trait, with one variant in the dispatch enum
(AGENTS.md §6), so it can be switched on per scenario or driver
profile.

### Turning around in congestion

A driver stuck in a jam pulls out, turns into the opposite direction and
tries another route.

- **Opposite road.** Each direction of a street is its own road, but
  nothing stores which road is the other direction: only `Placement` finds
  twins, by comparing reversed geometry, to draw them side by side. A stored
  twin index (set at import or derived at load time) is a prerequisite.
- **Transfer.** A vehicle whose front bumper is at offset `s` on road A→B
  moves to offset `length − s + L` on road B→A (`L` = vehicle length, since
  the front and rear swap). It is the same kind of move as a lane change,
  including inserting into the middle of a lane list, which M3 needs anyway.
- **Gap check.** Enough space on the opposite lane in both directions, as
  for MOBIL's safety criterion.
- **Reroute.** A new route from the new position, through the route-update
  phase (M5.5).
- **Where it's allowed.** OSM says little about road markings, so a
  plausible rule is needed, e.g. not on motorways or dual carriageways, only
  below a speed and above a waiting time, and driver-profile dependent
  (aggressive drivers do it sooner).

### Lateral position

Pulling out, squeezing past, or turning in several moves needs a lateral
offset within the lane: an optional field in the vehicle columns, in the
style of SUMO's sublane model. It affects animation and, if the submodel
uses it, gap checks. The road-relative position model stays.

### Other maneuvers for the same submodel

- Parking in and out (with the parking search in the backlog).
- Waiting in a turning lane or blocking a lane while turning left.
- Emergency vehicles: others pull to the side.
