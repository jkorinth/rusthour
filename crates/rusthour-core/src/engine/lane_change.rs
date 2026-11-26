//! Phase 2b: lane changes by MOBIL, European asymmetric (ADR-0005 §6,
//! M3.6), with a route incentive towards the lanes of the next movement
//! (§7, M3.7).

use std::collections::VecDeque;

use super::{Engine, MAX_DECEL_MPS2, OUT_OF_TOWN_MPS, leader_ahead, no_right_pass};
use crate::behavior::mobil::{self, Accels};
use crate::network::Network;
use crate::network::lanes::Lanes;
use crate::vehicle::Vehicles;
use crate::vehicle::idm::{self, Leader};

/// Each vehicle is evaluated once per this much simulated time.
const EVAL_PERIOD_MS: u64 = 1000;
/// No new change this soon after the last one.
const COOLDOWN_MS: u64 = 3000;
/// No changes in a road's first metres: the followers on the previous road
/// aren't seen (`shortcut:` ADR-0005 §6; look back across the node if the
/// M3 trial shows cut-ins right after junctions).
const ENTRY_ZONE_M: f64 = 10.0;
/// Roads shorter than this leave no room to change lanes.
const MIN_ROAD_M: f64 = 20.0;
/// The lane shift fades out over this much simulated time (viewer).
const FADE_MS: u64 = 2000;
/// The route incentive starts at least this far before the line (m).
const ROUTE_ZONE_M: f64 = 100.0;

impl Engine {
    /// Phase 2b: each due vehicle on a multi-lane road weighs a change to
    /// either side by MOBIL, from start-of-step state; then the changes are
    /// made in `(LaneId, head first)` order, each re-checked for safety
    /// against the lanes as already changed this step.
    pub(super) fn change_lanes(&mut self) {
        let dt = u64::from(self.config.dt_ms);
        let period = (EVAL_PERIOD_MS / dt).max(1);
        let cooldown = COOLDOWN_MS / dt;
        let (net, lanes, vs) = (&self.network, &self.lanes, &mut self.vehicles);
        for t in &mut vs.lc_ticks {
            *t = t.saturating_add(1);
        }
        let todo = &mut self.lane_change_candidates;
        todo.clear();
        // The due slots, `(tick + slot) % period == 0`, as in phase 2.
        let first = (period - self.tick % period) % period;
        let step = usize::try_from(period).unwrap_or(usize::MAX);
        for u in (usize::try_from(first).unwrap_or(usize::MAX)..vs.slot_count()).step_by(step) {
            if !vs.is_live(u) {
                continue;
            }
            let (r, x) = (vs.road(u), vs.pos_m[u]);
            let road = &net.roads[r as usize];
            if road.lanes < 2
                || road.length_m < MIN_ROAD_M
                || x < ENTRY_ZONE_M
                || u64::from(vs.lc_ticks[u]) < cooldown
            {
                continue;
            }
            let q = vs.lane_id(r, vs.lane_of(u));
            let lane = vs.lane(q);
            let mut i = lane.partition_point(|&o| vs.pos_m[o as usize] > x);
            while lane[i] as usize != u {
                i += 1; // past others at exactly its position
            }
            if let Some(to) = best_change(net, lanes, vs, q, i) {
                todo.push((q, i, u, to));
            }
        }
        // Made in `(LaneId, head first)` order.
        todo.sort_unstable_by_key(|&(q, i, _, _)| (q, i));
        for &(_, _, u, to) in todo.iter() {
            if let Some((_, nf)) = gap_check(vs, u, vs.lane_id(vs.road(u), to)) {
                let ok = nf.is_none_or(|f| {
                    new_follower_after(net, vs, u, f) >= -vs.mobil[u].safe_decel_mps2
                });
                if ok {
                    vs.change_lane(vs.id(u), to);
                    self.lane_changes += 1;
                }
            }
        }
    }

    /// Lane changes so far.
    #[must_use]
    pub fn lane_changes(&self) -> u64 {
        self.lane_changes
    }

    /// Fallback re-plans of heads stuck in the wrong lane so far (M3.7).
    #[must_use]
    pub fn fallback_replans(&self) -> u64 {
        self.fallback_replans
    }

    /// How far `slot` is still drawn towards the lane it left, in lanes
    /// (positive: towards the left); fades from the full change to 0 over
    /// 2 s. The core changes lanes instantly; this is for the viewer.
    #[must_use]
    pub fn lane_shift(&self, slot: usize) -> f64 {
        let vs = &self.vehicles;
        let ms = u64::from(vs.lc_ticks[slot]) * u64::from(self.config.dt_ms);
        if ms >= FADE_MS {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)] // below 2,000
        let left = 1.0 - ms as f64 / FADE_MS as f64;
        (f64::from(vs.lc_from[slot]) - f64::from(vs.lane_of(slot))) * left
    }
}

/// Where the lane choice zone before the line starts for `slot` (m to the end).
pub(super) fn route_zone_m(vs: &Vehicles, slot: usize) -> f64 {
    ROUTE_ZONE_M.max(3.0 * vs.speed_mps[slot] * vs.params[slot].time_headway_s)
}

/// The lane `slot` must change to: one step towards the lanes of its next
/// movement, if it is in the zone and its own lane doesn't serve it.
fn merge_to(net: &Network, lanes: &Lanes, vs: &Vehicles, slot: usize) -> Option<u8> {
    let (r, lane) = (vs.road(slot), vs.lane_of(slot));
    let road = &net.roads[r as usize];
    if road.length_m - vs.pos_m[slot] >= route_zone_m(vs, slot) {
        return None;
    }
    let &to = vs.route[slot].get(vs.route_pos[slot] as usize + 1)?;
    let serves = lanes.from_mask(net, r, to);
    if serves >> lane & 1 == 1 {
        return None;
    }
    // The nearest lane that serves it, the right one on a tie.
    let k = (0..road.lanes)
        .filter(|&k| serves >> k & 1 == 1)
        .min_by_key(|&k| k.abs_diff(lane))?;
    Some(if k > lane { lane + 1 } else { lane - 1 })
}

/// Cap on the acceleration of `slot` so that the nearest vehicle in the
/// lane `side` beside it with its rear ahead of `slot` can merge in front of
/// it, if it must ([`merge_to`]); like a leader. `state` carries, down the
/// lane of `slot`, how many of `side`'s vehicles have their rear ahead and
/// that nearest one if it must merge.
pub(super) fn merge_cap(
    net: &Network,
    lanes: &Lanes,
    vs: &Vehicles,
    side: &VecDeque<u32>,
    state: &mut (usize, Option<usize>),
    slot: usize,
    v0: f64,
) -> f64 {
    let (x, (ahead, merger)) = (vs.pos_m[slot], state);
    let rear = |o: usize| vs.pos_m[o] - vs.params[o].length_m;
    let before = *ahead;
    while side.get(*ahead).is_some_and(|&o| rear(o as usize) > x) {
        *ahead += 1;
    }
    if *ahead != before {
        let m = side[*ahead - 1] as usize;
        *merger = (merge_to(net, lanes, vs, m) == Some(vs.lane_of(slot))).then_some(m);
    }
    merger.map_or(f64::INFINITY, |m| {
        let leader = Leader {
            gap_m: rear(m) - x,
            speed_mps: vs.speed_mps[m],
        };
        idm::acceleration(&vs.params[slot], v0, vs.speed_mps[slot], Some(leader))
    })
}

/// IDM acceleration of `slot` behind `leader`, as phase 4 clamps it.
fn accel(net: &Network, vs: &Vehicles, slot: usize, leader: Option<Leader>) -> f64 {
    let p = &vs.params[slot];
    let v0 = p.v0(net.roads[vs.road(slot) as usize].speed_limit_mps);
    idm::acceleration(p, v0, vs.speed_mps[slot], leader).max(-MAX_DECEL_MPS2)
}

/// `f` following `l` in the same lane.
fn behind(vs: &Vehicles, f: usize, l: usize) -> Leader {
    Leader {
        gap_m: vs.pos_m[l] - vs.params[l].length_m - vs.pos_m[f],
        speed_mps: vs.speed_mps[l],
    }
}

/// The leader of `slot` if it drove in lane `lane` (of its road) with
/// `ahead` the nearest vehicle there: that one, or the tail across the road
/// end as in phase 4 (stop lines are left out on both sides of the choice).
fn leader_of(
    net: &Network,
    lanes: &Lanes,
    vs: &Vehicles,
    slot: usize,
    lane: u8,
    ahead: Option<usize>,
) -> Option<Leader> {
    if let Some(l) = ahead {
        return Some(behind(vs, slot, l));
    }
    let dist_m = net.roads[vs.road(slot) as usize].length_m - vs.pos_m[slot];
    leader_ahead(net, lanes, vs, slot, (lane, None), dist_m).map(|(l, _)| l)
}

/// The new leader and follower of `slot` in `LaneId` `q`, if the gaps to
/// both are at least its `s0`.
#[allow(clippy::type_complexity)]
fn gap_check(vs: &Vehicles, slot: usize, q: usize) -> Option<(Option<usize>, Option<usize>)> {
    let target = vs.lane(q);
    let x = vs.pos_m[slot];
    let j = target.partition_point(|&o| vs.pos_m[o as usize] > x);
    let nl = j.checked_sub(1).map(|k| target[k] as usize);
    let nf = target.get(j).map(|&f| f as usize);
    let s0 = vs.params[slot].min_gap_m;
    let ok = nl.is_none_or(|l| behind(vs, slot, l).gap_m >= s0)
        && nf.is_none_or(|f| behind(vs, f, slot).gap_m >= s0);
    ok.then_some((nl, nf))
}

/// The new follower's acceleration once `slot` is in front of it.
fn new_follower_after(net: &Network, vs: &Vehicles, slot: usize, f: usize) -> f64 {
    accel(net, vs, f, Some(behind(vs, f, slot)))
}

/// The target lane of the vehicle at index `i` of `LaneId` `q`, if MOBIL
/// changes it; the side with the larger margin wins, the right on a tie.
#[allow(clippy::many_single_char_names)] // `c`, `n`, `o` as in the book
fn best_change(net: &Network, lanes: &Lanes, vs: &Vehicles, q: usize, i: usize) -> Option<u8> {
    let lane = vs.lane(q);
    let c = lane[i] as usize;
    let (r, from) = (vs.road(c), vs.lane_of(c));
    let n = net.roads[r as usize].lanes;
    let old_leader = i.checked_sub(1).map(|k| lane[k] as usize);
    let old_follower = lane.get(i + 1).map(|&f| f as usize);
    let out_of_town = net.roads[r as usize].speed_limit_mps > OUT_OF_TOWN_MPS;
    // Whether the ban on passing on the right (phase 4) would hold back a
    // vehicle at `v` behind `l` once `l` is in the lane to its left.
    let banned = |l: usize, v: f64| out_of_town && no_right_pass(vs.speed_mps[l], v);
    // Route incentive (ADR-0005 §7): within the zone before the line, each
    // lane between a vehicle and the lanes of its next movement costs
    // `penalty`, growing linearly to `2·b` at the line.
    let serves = vs.route[c]
        .get(vs.route_pos[c] as usize + 1)
        .map_or(u16::MAX, |&next| lanes.from_mask(net, r, next));
    let off = |l: u8| {
        (0..n)
            .filter(|&k| serves >> k & 1 == 1)
            .map(|k| k.abs_diff(l))
            .min()
            .unwrap_or(0)
    };
    let p = &vs.params[c];
    let zone = route_zone_m(vs, c);
    let dist_m = net.roads[r as usize].length_m - vs.pos_m[c];
    // In the zone, never away from the lanes of the next movement; and only
    // into a gap of `s0` both ways. In dense traffic there is often no such
    // side, and then nothing else needs computing.
    let sides = [(from.wrapping_sub(1), false), (from + 1, true)].map(|(to, left)| {
        if to >= n || (dist_m < zone && off(to) > off(from)) {
            return None;
        }
        gap_check(vs, c, vs.lane_id(r, to)).map(|gap| (to, left, gap))
    });
    if sides.iter().all(Option::is_none) {
        return None;
    }
    let penalty = 2.0 * p.comfort_decel_mps2 * (1.0 - dist_m / zone).max(0.0);
    let a_c = accel(net, vs, c, leader_of(net, lanes, vs, c, from, old_leader));
    let a_o = old_follower.map(|o| {
        let now = accel(net, vs, o, Some(behind(vs, o, c)));
        [
            now,
            accel(net, vs, o, leader_of(net, lanes, vs, o, from, old_leader)),
        ]
    });
    // Moving left frees `o` only if it may then pass `c` on the right.
    let a_o_left = a_o.map(|[now, after]| {
        let held = old_follower.is_some_and(|o| banned(c, vs.speed_mps[o]));
        [now, if held { after.min(now) } else { after }]
    });
    // Moving right gains nothing if `c` may not then pass its old leader.
    let held_right = old_leader.is_some_and(|l| banned(l, vs.speed_mps[c]));
    let mut best: Option<(f64, u8)> = None;
    for (to, left, (nl, nf)) in sides.into_iter().flatten() {
        // In the zone, a change towards them is mandatory: politeness is waived.
        let mandatory = dist_m < zone && off(to) < off(from);
        let m = &vs.mobil[c];
        let a_n = nf.map(|f| {
            let now = accel(net, vs, f, leader_of(net, lanes, vs, f, to, nl));
            [now, new_follower_after(net, vs, c, f)]
        });
        if a_n.is_some_and(|a| a[1] < -m.safe_decel_mps2) {
            continue;
        }
        let mut after = accel(net, vs, c, leader_of(net, lanes, vs, c, to, nl));
        if !left && held_right {
            after = after.min(a_c);
        }
        let a = if mandatory {
            Accels {
                c: [a_c, after],
                ..Accels::default()
            }
        } else {
            Accels {
                c: [a_c, after],
                n: a_n.unwrap_or_default(),
                o: (if left { a_o_left } else { a_o }).unwrap_or_default(),
            }
        };
        let gain = penalty * (f64::from(off(from)) - f64::from(off(to)));
        let margin = mobil::margin(m, left, &a) + gain;
        if margin > 0.0 && best.is_none_or(|(b, _)| margin > b) {
            best = Some((margin, to));
        }
    }
    best.map(|(_, to)| to)
}

#[cfg(test)]
mod tests {
    use super::super::car;
    use crate::network::{EndControl, Network, Road};
    use crate::vehicle::NewVehicle;
    use crate::vehicle::class::Class;
    use crate::vehicle::idm::IdmParams;
    use crate::{Config, Engine};

    /// One straight road with two lanes and the given speed limit.
    fn straight(length_m: f64, speed_limit_mps: f64) -> Network {
        Network {
            nodes: vec![[0.0, 0.0], [length_m, 0.0]],
            roads: vec![Road {
                from: 0,
                to: 1,
                length_m,
                speed_limit_mps,
                lanes: 2,
                start_dir: [1.0, 0.0],
                end_dir: [1.0, 0.0],
                roundabout: false,
                class: 0,
                priority_road: false,
                end_control: EndControl::None,
                turn_lanes: vec![],
            }],
            connections: vec![],
        }
    }

    /// Puts a vehicle at the tail of `lane` of road 0 at `pos_m`, driving
    /// `speed_mps`; returns its slot.
    fn put(e: &mut Engine, lane: u8, pos_m: f64, speed_mps: f64, params: IdmParams) -> usize {
        let id = e.vehicles.spawn(NewVehicle {
            road: 0,
            lane,
            pos_m,
            speed_mps,
            params,
            mobil: crate::behavior::mobil::MobilParams::default(),
            gap: crate::behavior::profile::Gap::default(),
            profile: crate::behavior::profile::Profile::Normal,
            class: crate::vehicle::class::Class::Car,
            route: &[0],
            depart_tick: 0,
        });
        id.slot as usize
    }

    fn factor(f: f64) -> IdmParams {
        IdmParams {
            desired_speed_factor: f,
            ..car()
        }
    }

    /// Turns MOBIL off for `slot`, so only the vehicle under test decides.
    fn never_changes(e: &mut Engine, slot: usize) {
        e.vehicles.mobil[slot].threshold_mps2 = f64::INFINITY;
    }

    fn assert_lanes_sorted(e: &Engine) {
        let vs = &e.vehicles;
        for q in 0..vs.lane_count() {
            let x: Vec<f64> = vs.lane(q).iter().map(|&s| vs.pos_m[s as usize]).collect();
            assert!(x.windows(2).all(|w| w[0] >= w[1]), "lane {q}: {x:?}");
        }
    }

    #[test]
    fn a_fast_car_overtakes_on_the_left_and_keeps_right_again() {
        let mut e = Engine::with_network(Config::default(), 1, straight(3000.0, 27.8));
        // 72 km/h on a 100 km/h road: the fast car may not pass it on the
        // right, so the slow car has no reason to make room on the left.
        let slow = put(&mut e, 0, 300.0, 20.0, factor(0.72));
        let fast = put(&mut e, 0, 200.0, 27.0, factor(1.0));
        let mut overtook = false;
        for _ in 0..1200 {
            let was = e.vehicles.lane_of(fast);
            e.step(1);
            assert_lanes_sorted(&e);
            let vs = &e.vehicles;
            if vs.lane_of(fast) != was {
                // The viewer slides it over from the old lane.
                let full = f64::from(was) - f64::from(vs.lane_of(fast));
                assert!((e.lane_shift(fast) - full).abs() < 1e-9);
            }
            assert_eq!(vs.lane_of(slow), 0, "the slow car keeps right");
            if vs.pos_m[fast] < vs.pos_m[slow] + 4.5 && vs.pos_m[fast] > vs.pos_m[slow] - 4.5 {
                assert_eq!(vs.lane_of(fast), 1, "passes on the left only");
                overtook = true;
            }
            if vs.pos_m[fast] > vs.pos_m[slow] + 150.0 {
                break;
            }
        }
        assert!(overtook, "never drew level");
        let vs = &e.vehicles;
        assert!(vs.pos_m[fast] > vs.pos_m[slow] + 150.0, "never got past");
        assert_eq!(vs.lane_of(fast), 0, "back in the right lane");
        assert!(e.lane_changes() >= 2);
    }

    /// Vehicle 0 (due at tick 0) drives 10 m/s, 15 m behind a car standing
    /// in lane 0; `follower` is put in lane 1 behind it. Returns its lane
    /// after one step.
    fn lane_after_one_step(follower: impl FnOnce(&mut Engine) -> usize) -> u8 {
        lane_after_one_step_of(car(), follower)
    }

    /// [`lane_after_one_step`] with vehicle 0 driving with `params`.
    fn lane_after_one_step_of(
        params: IdmParams,
        follower: impl FnOnce(&mut Engine) -> usize,
    ) -> u8 {
        let mut e = Engine::with_network(Config::default(), 1, straight(500.0, 13.9));
        // Spawned first (and moved behind the blocker) to be due at tick 0.
        let c = put(&mut e, 1, 100.0, 10.0, params);
        assert_eq!(c, 0);
        let blocker = put(&mut e, 0, 119.5, 0.0, car());
        never_changes(&mut e, blocker);
        e.vehicles.change_lane(e.vehicles.id(c), 0);
        e.vehicles.lc_ticks[c] = u16::MAX;
        let f = follower(&mut e);
        never_changes(&mut e, f);
        e.step(1);
        e.vehicles.lane_of(c)
    }

    #[test]
    fn no_change_into_a_gap_below_s0_or_that_brakes_the_follower_too_hard() {
        // A follower that would barely mind (short s0, standing): only the
        // changing car's own s0 = 2 m decides.
        let calm = |gap: f64| {
            move |e: &mut Engine| {
                let p = IdmParams {
                    min_gap_m: 0.5,
                    time_headway_s: 0.5,
                    ..car()
                };
                put(e, 1, 100.0 - 4.5 - gap, 0.0, p)
            }
        };
        assert_eq!(lane_after_one_step(calm(1.5)), 0, "gap below s0");
        assert_eq!(lane_after_one_step(calm(3.0)), 1, "gap of s0 or more");
        // A fast follower 20 m back would brake harder than b_safe = 4 m/s².
        let fast = |e: &mut Engine| put(e, 1, 100.0 - 4.5 - 20.0, 25.0, factor(2.0));
        assert_eq!(lane_after_one_step(fast), 0, "unsafe for the follower");
        let far = |e: &mut Engine| put(e, 1, 20.0, 10.0, car());
        assert_eq!(lane_after_one_step(far), 1, "safe for the follower");
    }

    /// ADR-0007 §5: the gap to the new follower is measured from the
    /// changing vehicle's own rear, so a gap a car takes is too short for a
    /// vehicle as long as a truck.
    #[test]
    fn a_longer_vehicle_needs_a_longer_gap_to_change_lanes() {
        // A standing follower 6 m behind a car's rear.
        let calm = |e: &mut Engine| {
            let p = IdmParams {
                min_gap_m: 0.5,
                time_headway_s: 0.5,
                ..car()
            };
            put(e, 1, 100.0 - 4.5 - 6.0, 0.0, p)
        };
        assert_eq!(lane_after_one_step_of(car(), calm), 1, "a car fits");
        let long = IdmParams {
            length_m: Class::Truck.spec().length_m,
            ..car()
        };
        assert_eq!(lane_after_one_step_of(long, calm), 0, "a truck doesn't");
    }

    /// Whether a car in lane 0 at 27 m/s gets past one in lane 1 at 20 m/s
    /// (72 km/h) within a minute, with MOBIL off, on a road with `limit`.
    fn passes_on_the_right(limit: f64) -> bool {
        let mut e = Engine::with_network(Config::default(), 1, straight(3000.0, limit));
        let fast = put(&mut e, 0, 250.0, 27.0, factor(27.0 / limit));
        let slow = put(&mut e, 1, 300.0, 20.0, factor(20.0 / limit));
        never_changes(&mut e, fast);
        never_changes(&mut e, slow);
        e.step(600);
        e.vehicles.pos_m[fast] > e.vehicles.pos_m[slow]
    }

    #[test]
    fn no_overtaking_on_the_right_outside_towns_but_inside_them() {
        // Same speeds, only the limit differs: 100 km/h, then 50 km/h.
        assert!(!passes_on_the_right(27.8), "outside towns");
        assert!(passes_on_the_right(13.9), "inside towns");
    }
}
