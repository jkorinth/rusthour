//! Vehicle state (structure-of-arrays) and IDM car-following kinematics (M2, M4).
//!
//! [`Vehicles`] stores one entry per slot, alive or free (ADR-0004 §1). Slots
//! are reused LIFO and carry a generation, so a [`VehicleId`] stays valid for
//! the vehicle's life and stale ids are rejected. Each lane keeps its slots
//! head first (descending position), sorted by construction: vehicles enter
//! at the tail and leave from anywhere, so nothing is ever sorted.

pub mod idm;

use crate::StateHasher;
use idm::IdmParams;
use std::collections::VecDeque;

/// `road` value of a free slot.
pub const NO_ROAD: u32 = u32::MAX;

/// Stable handle to a vehicle; rejected once the vehicle has despawned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VehicleId {
    /// Index into the columns.
    pub slot: u32,
    /// Bumped each time the slot is freed.
    pub generation: u32,
}

/// What [`Vehicles::spawn`] needs to create a vehicle.
#[derive(Clone, Copy, Debug)]
pub struct NewVehicle<'a> {
    /// Road to enter.
    pub road: u32,
    /// Lane on that road (0 = rightmost).
    pub lane: u8,
    /// Front-bumper position from the road start (m).
    pub pos_m: f64,
    /// Initial speed (m/s).
    pub speed_mps: f64,
    /// IDM parameters.
    pub params: IdmParams,
    /// Road ids, origin..=destination; `road` must be the first.
    pub route: &'a [u32],
    /// Tick the vehicle departs.
    pub depart_tick: u64,
}

/// Vehicle columns plus per-lane sorted slot lists.
///
/// The columns without invariants are public for the engine's phases; use
/// [`Vehicles::spawn`], [`Vehicles::transfer`] and [`Vehicles::despawn`] to
/// change membership, which keeps the lane lists consistent.
#[derive(Clone, Debug)]
pub struct Vehicles {
    generation: Vec<u32>,
    road: Vec<u32>,
    lane: Vec<u8>,
    /// Front bumper from the road start (m).
    pub pos_m: Vec<f64>,
    /// Speed (m/s), >= 0.
    pub speed_mps: Vec<f64>,
    /// Acceleration written by the acceleration phase (m/s²).
    pub accel_mps2: Vec<f64>,
    /// IDM parameters.
    pub params: Vec<IdmParams>,
    /// Road ids, origin..=destination; the `Vec` is reused per slot.
    pub route: Vec<Vec<u32>>,
    /// Index of the current road in `route`.
    pub route_pos: Vec<u32>,
    /// Tick the vehicle departed.
    pub depart_tick: Vec<u64>,
    free: Vec<u32>,
    /// Per `LaneId`: slots, head first. Crate-visible so the engine can walk a
    /// lane while writing the columns; change membership only via the methods.
    pub(crate) lanes: Vec<VecDeque<u32>>,
    lane_first: Vec<u32>,
}

impl Vehicles {
    /// Creates an empty store with a pre-reserved queue for every lane.
    ///
    /// # Panics
    /// If the network has more than `u32::MAX` lanes.
    #[must_use]
    pub fn new(net: &crate::network::Network) -> Self {
        let mut lanes = Vec::new();
        let mut lane_first = Vec::with_capacity(net.roads.len());
        for r in &net.roads {
            lane_first.push(u32::try_from(lanes.len()).expect("lane count fits u32"));
            // Physical maximum: bumper to bumper (gap 0, which is reachable)
            // at the 4.5 m car length, so a lane queue never reallocates.
            // Capped (about 18 km of road) so an absurd length that passed
            // validation can't overflow the allocation; longer lanes just grow.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let cap = (r.length_m / 4.5).ceil().min(4096.0) as usize;
            lanes.extend((0..r.lanes).map(|_| VecDeque::with_capacity(cap)));
        }
        Self {
            generation: Vec::new(),
            road: Vec::new(),
            lane: Vec::new(),
            pos_m: Vec::new(),
            speed_mps: Vec::new(),
            accel_mps2: Vec::new(),
            params: Vec::new(),
            route: Vec::new(),
            route_pos: Vec::new(),
            depart_tick: Vec::new(),
            free: Vec::new(),
            lanes,
            lane_first,
        }
    }

    /// Number of slots, alive or free.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.generation.len()
    }

    /// Number of lanes in the network (`LaneId` is `0..lane_count`).
    #[must_use]
    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    /// Slots in a lane, head (largest position) first.
    #[must_use]
    pub fn lane(&self, lane_id: usize) -> &VecDeque<u32> {
        &self.lanes[lane_id]
    }

    /// `LaneId` of `lane` on `road`.
    #[must_use]
    pub fn lane_id(&self, road: u32, lane: u8) -> usize {
        self.lane_first[road as usize] as usize + usize::from(lane)
    }

    /// Number of live vehicles.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.generation.len() - self.free.len()
    }

    /// The current id of `slot` (for `transfer`/`despawn` when walking slots).
    ///
    /// # Panics
    /// If `slot` is out of range.
    #[must_use]
    pub fn id(&self, slot: usize) -> VehicleId {
        VehicleId {
            slot: u32::try_from(slot).expect("slot fits u32"),
            generation: self.generation[slot],
        }
    }

    /// A lane's order together with the position column, so integration can
    /// walk the lane while writing positions without cloning the queue.
    pub fn lane_with_pos_mut(&mut self, lane_id: usize) -> (&VecDeque<u32>, &mut [f64]) {
        (&self.lanes[lane_id], &mut self.pos_m)
    }

    /// Whether the slot holds a vehicle.
    #[must_use]
    pub fn is_live(&self, slot: usize) -> bool {
        self.road[slot] != NO_ROAD
    }

    /// Whether `id` still names its vehicle.
    #[must_use]
    pub fn is_alive(&self, id: VehicleId) -> bool {
        let s = id.slot as usize;
        self.generation.get(s) == Some(&id.generation) && self.is_live(s)
    }

    /// Road of a live slot (`NO_ROAD` for a free one).
    #[must_use]
    pub fn road(&self, slot: usize) -> u32 {
        self.road[slot]
    }

    /// Lane of a live slot.
    #[must_use]
    pub fn lane_of(&self, slot: usize) -> u8 {
        self.lane[slot]
    }

    /// Adds a vehicle at the tail of its lane. The caller checks that the
    /// entry gap allows it; `pos_m` must not exceed the current tail's.
    ///
    /// # Panics
    /// If `lane` does not exist on `road`, or there are more than `u32::MAX` slots.
    pub fn spawn(&mut self, v: NewVehicle<'_>) -> VehicleId {
        let slot = if let Some(slot) = self.free.pop() {
            slot
        } else {
            let slot = u32::try_from(self.generation.len()).expect("slot count fits u32");
            self.generation.push(0);
            self.road.push(NO_ROAD);
            self.lane.push(0);
            self.pos_m.push(0.0);
            self.speed_mps.push(0.0);
            self.accel_mps2.push(0.0);
            self.params.push(IdmParams::default());
            self.route.push(Vec::new());
            self.route_pos.push(0);
            self.depart_tick.push(0);
            slot
        };
        let s = slot as usize;
        self.params[s] = v.params;
        self.speed_mps[s] = v.speed_mps;
        self.route[s].extend_from_slice(v.route);
        self.depart_tick[s] = v.depart_tick;
        self.push_tail(slot, v.road, v.lane, v.pos_m);
        VehicleId {
            slot,
            generation: self.generation[s],
        }
    }

    /// Moves a vehicle to the tail of another lane (road transfer), at
    /// `pos_m`, which must not exceed that tail's position. Returns `false`
    /// for a stale id.
    ///
    /// # Panics
    /// If `lane` does not exist on `road`.
    pub fn transfer(&mut self, id: VehicleId, road: u32, lane: u8, pos_m: f64) -> bool {
        if !self.is_alive(id) {
            return false;
        }
        self.unlink(id.slot);
        self.push_tail(id.slot, road, lane, pos_m);
        true
    }

    /// Frees the vehicle's slot. Returns `false` for a stale id.
    pub fn despawn(&mut self, id: VehicleId) -> bool {
        if !self.is_alive(id) {
            return false;
        }
        self.unlink(id.slot);
        let s = id.slot as usize;
        self.generation[s] = self.generation[s].wrapping_add(1);
        self.road[s] = NO_ROAD;
        self.lane[s] = 0;
        self.pos_m[s] = 0.0;
        self.speed_mps[s] = 0.0;
        self.accel_mps2[s] = 0.0;
        self.params[s] = IdmParams::default();
        self.route[s].clear();
        self.route_pos[s] = 0;
        self.depart_tick[s] = 0;
        self.free.push(id.slot);
        true
    }

    fn push_tail(&mut self, slot: u32, road: u32, lane: u8, pos_m: f64) {
        let s = slot as usize;
        self.road[s] = road;
        self.lane[s] = lane;
        self.pos_m[s] = pos_m;
        let q = self.lane_id(road, lane);
        let end = self
            .lane_first
            .get(road as usize + 1)
            .map_or(self.lanes.len(), |&n| n as usize);
        assert!(q < end, "lane {lane} out of range for road {road}");
        // Weaker than ADR-0004's fit rule (`pos <= x_tail - length_tail`): it
        // only guards the order; the engine enforces the physical gap.
        debug_assert!(
            self.lanes[q]
                .back()
                .is_none_or(|&t| self.pos_m[t as usize] >= pos_m),
            "entry must not pass the lane's tail"
        );
        self.lanes[q].push_back(slot);
    }

    fn unlink(&mut self, slot: u32) {
        let s = slot as usize;
        let q = self.lane_id(self.road[s], self.lane[s]);
        // Heads leave in the common case (arrivals, transfers): O(1).
        // shortcut: O(lane) scan for the rest, fine until M3 lane changes are hot.
        let i = self.lanes[q]
            .iter()
            .position(|&x| x == slot)
            .expect("live slot is in its lane");
        self.lanes[q].remove(i);
    }

    /// Feeds the whole store into `h`: columns in slot order, then the free
    /// list and every lane in `LaneId` order.
    pub fn hash_into(&self, h: &mut StateHasher) {
        h.write_u64(self.generation.len() as u64);
        for s in 0..self.generation.len() {
            h.write_u64(u64::from(self.generation[s]));
            h.write_u64(u64::from(self.road[s]));
            h.write_u64(u64::from(self.lane[s]));
            h.write_f64(self.pos_m[s]);
            h.write_f64(self.speed_mps[s]);
            h.write_f64(self.accel_mps2[s]);
            let p = &self.params[s];
            for x in [
                p.desired_speed_factor,
                p.time_headway_s,
                p.max_accel_mps2,
                p.comfort_decel_mps2,
                p.min_gap_m,
                p.length_m,
                p.sqrt_ab,
            ] {
                h.write_f64(x);
            }
            h.write_u64(self.route[s].len() as u64);
            for &r in &self.route[s] {
                h.write_u64(u64::from(r));
            }
            h.write_u64(u64::from(self.route_pos[s]));
            h.write_u64(self.depart_tick[s]);
        }
        h.write_u64(self.free.len() as u64);
        for &f in &self.free {
            h.write_u64(u64::from(f));
        }
        for q in &self.lanes {
            h.write_u64(q.len() as u64);
            for &s in q {
                h.write_u64(u64::from(s));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{Network, Road};
    use proptest::prelude::*;

    fn net() -> Network {
        let road = |lanes| Road {
            from: 0,
            to: 1,
            length_m: 130.0,
            speed_limit_mps: 14.0,
            lanes,
            start_dir: [1.0, 0.0],
            end_dir: [1.0, 0.0],
            roundabout: false,
        };
        Network {
            nodes: vec![[0.0, 0.0], [130.0, 0.0]],
            roads: vec![road(1), road(2), road(1)],
            connections: vec![],
        }
    }

    fn params() -> IdmParams {
        IdmParams {
            desired_speed_factor: 1.0,
            time_headway_s: 1.5,
            max_accel_mps2: 1.5,
            comfort_decel_mps2: 2.0,
            min_gap_m: 2.0,
            length_m: 4.5,
            sqrt_ab: 3.0_f64.sqrt(),
        }
    }

    fn new(road: u32, lane: u8, speed: f64, route: &[u32]) -> NewVehicle<'_> {
        NewVehicle {
            road,
            lane,
            pos_m: 0.0,
            speed_mps: speed,
            params: params(),
            route,
            depart_tick: 7,
        }
    }

    #[derive(Debug, Clone)]
    enum Op {
        Spawn(u32),
        /// Despawn the n-th live vehicle (modulo the live count).
        Despawn(usize),
        /// Move the n-th live vehicle to the tail of another road.
        Transfer(usize, u32),
        /// Advance every lane head to tail, never passing the leader.
        Advance(f64),
    }

    fn ops() -> impl Strategy<Value = Vec<Op>> {
        prop::collection::vec(
            prop_oneof![
                (0..3u32).prop_map(Op::Spawn),
                any::<usize>().prop_map(Op::Despawn),
                (any::<usize>(), 0..3u32).prop_map(|(i, r)| Op::Transfer(i, r)),
                (0.0..20.0f64).prop_map(Op::Advance),
            ],
            0..200,
        )
    }

    /// Runs `ops` on a fresh store, calling `check` after every op.
    fn run(ops: &[Op], check: impl Fn(&Vehicles)) {
        let net = net();
        let mut vs = Vehicles::new(&net);
        let mut live: Vec<VehicleId> = Vec::new();
        for op in ops {
            match *op {
                Op::Spawn(road) => {
                    live.push(vs.spawn(new(road, 0, 0.0, &[road])));
                }
                Op::Despawn(i) if !live.is_empty() => {
                    assert!(vs.despawn(live.swap_remove(i % live.len())));
                }
                Op::Transfer(i, road) if !live.is_empty() => {
                    let id = live[i % live.len()];
                    assert!(vs.transfer(id, road, 0, 0.0));
                }
                Op::Advance(d) => {
                    for q in 0..vs.lane_count() {
                        let mut front = f64::INFINITY;
                        let (order, pos) = vs.lane_with_pos_mut(q);
                        for &s in order {
                            let s = s as usize;
                            let x = (pos[s] + d).min(front).max(pos[s]);
                            pos[s] = x;
                            front = x - 4.5;
                        }
                    }
                }
                _ => {}
            }
            check(&vs);
        }
    }

    proptest! {
        #[test]
        fn lanes_are_sorted_by_position(ops in ops()) {
            run(&ops, |vs| {
                for lane in 0..vs.lane_count() {
                    let xs: Vec<f64> =
                        vs.lane(lane).iter().map(|&s| vs.pos_m[s as usize]).collect();
                    assert!(xs.windows(2).all(|w| w[0] >= w[1]), "{xs:?}");
                }
            });
        }

        #[test]
        fn every_live_slot_is_in_exactly_one_lane(ops in ops()) {
            run(&ops, |vs| {
                let mut count = vec![0u32; vs.slot_count()];
                for lane in 0..vs.lane_count() {
                    for &s in vs.lane(lane) {
                        count[s as usize] += 1;
                    }
                }
                for (slot, &n) in count.iter().enumerate() {
                    assert_eq!(n, u32::from(vs.is_live(slot)), "slot {slot}");
                }
            });
        }
    }

    #[test]
    fn ids_are_stable_and_generations_reject_stale_ids() {
        let net = net();
        let mut vs = Vehicles::new(&net);
        let spawn = |vs: &mut Vehicles| vs.spawn(new(0, 0, 0.0, &[0]));
        let a = spawn(&mut vs);
        let b = spawn(&mut vs);
        vs.pos_m[a.slot as usize] = 50.0;
        assert!(vs.despawn(a));
        assert!(
            !vs.is_alive(a) && !vs.despawn(a),
            "stale id must be rejected"
        );
        assert!(vs.is_alive(b), "other ids are unaffected");
        // LIFO reuse: same slot, next generation, reset state.
        let c = spawn(&mut vs);
        assert_eq!((c.slot, c.generation), (a.slot, a.generation + 1));
        assert!(vs.is_alive(c) && !vs.is_alive(a));
        assert!(vs.pos_m[c.slot as usize].abs() < 1e-12);
    }

    #[test]
    fn hash_covers_state() {
        let net = net();
        let hash = |vs: &Vehicles| {
            let mut h = StateHasher::new();
            vs.hash_into(&mut h);
            h.finish()
        };
        let mut x = Vehicles::new(&net);
        let mut y = Vehicles::new(&net);
        assert_eq!(hash(&x), hash(&y));
        let id = x.spawn(new(1, 1, 3.0, &[1, 2]));
        assert_ne!(hash(&x), hash(&y));
        y.spawn(new(1, 1, 3.0, &[1, 2]));
        assert_eq!(hash(&x), hash(&y));
        x.speed_mps[id.slot as usize] = 3.5;
        assert_ne!(hash(&x), hash(&y));
    }
}
