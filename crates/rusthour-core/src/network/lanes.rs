//! Lane sets per movement and entry lanes on the next road, derived at
//! load (ADR-0005 §5).

use super::Network;
use super::movement::{self, Turn};

/// Below this length the next road leaves no room to change lanes (m).
const SHORT_ROAD_M: f64 = 60.0;

/// One movement out of a road.
#[derive(Debug, Clone, Copy)]
struct Move {
    to: u32,
    turn: Turn,
    /// Lanes of the from-road that may take it, lane 0 = bit 0.
    from_mask: u16,
}

/// The movements of every road with their lane sets.
#[derive(Debug, Clone)]
pub struct Lanes {
    /// Road `r`'s movements, ascending by target, are
    /// `moves[first[r]..first[r + 1]]`.
    first: Vec<u32>,
    moves: Vec<Move>,
}

/// The `turn_lanes` bit of a turn type.
fn arrow(t: Turn) -> u8 {
    match t {
        Turn::Left | Turn::LeftAcross => 1,
        Turn::Straight => 2,
        Turn::Right => 4,
        Turn::UTurn => 8,
    }
}

/// Lanes `lo..hi` as a mask (`hi ≤ 16`).
fn lanes_mask(lo: u8, hi: u8) -> u16 {
    u16::try_from((1u32 << hi) - (1u32 << lo)).expect("at most 16 lanes")
}

/// The default rule (ADR-0005 §5) for a movement of type `t` on a road with
/// `n` lanes, given which kinds of movement the road has.
fn default_mask(n: u8, t: Turn, [straight, right, left]: [bool; 3]) -> u16 {
    let all = lanes_mask(0, n);
    if n == 1 || t == Turn::Straight {
        return all;
    }
    let is_right = t == Turn::Right;
    let right_half = lanes_mask(0, n.div_ceil(2));
    // With no straight movement, turns only share the lanes if both kinds exist.
    match (straight, if is_right { left } else { right }) {
        (true, _) if is_right => 1,
        (true, _) => 1 << (n - 1),
        (false, true) if is_right => right_half,
        (false, true) => all & !right_half,
        (false, false) => all,
    }
}

impl Lanes {
    /// Builds the lane sets of every movement in `net.connections`: from
    /// `turn_lanes` where the road has them, else by the default rule.
    ///
    /// # Panics
    ///
    /// If the network has more than `u32::MAX` roads or a road has more
    /// than 16 lanes (the api rejects both).
    #[must_use]
    // shortcut: the importer doesn't yet warn about arrows with no movement
    // (ADR-0005 §3); the core just falls back to the default rule. It needs
    // turn types in the importer, so do it when that is shared via the api.
    pub fn new(net: &Network) -> Self {
        let mut conns = net.connections.clone();
        conns.sort_unstable();
        let mut first = vec![0u32; net.roads.len() + 1];
        for &(from, _) in &conns {
            first[from as usize + 1] += 1;
        }
        for i in 0..net.roads.len() {
            first[i + 1] += first[i];
        }
        let (in_first, incoming) = movement::incoming(net);
        let mut moves = Vec::with_capacity(conns.len());
        for (from, road) in net.roads.iter().enumerate() {
            let node = road.to as usize;
            let arms = &incoming[in_first[node] as usize..in_first[node + 1] as usize];
            let out = &conns[first[from] as usize..first[from + 1] as usize];
            let from = u32::try_from(from).expect("road count fits u32");
            let turns: Vec<Turn> = out
                .iter()
                .map(|&(_, to)| Turn::of(net, from, to, arms))
                .collect();
            let has = |f: fn(&Turn) -> bool| turns.iter().any(f);
            let kinds = [
                has(|t| *t == Turn::Straight),
                has(|t| *t == Turn::Right),
                has(|t| !matches!(t, Turn::Straight | Turn::Right)),
            ];
            let default: Vec<u16> = turns
                .iter()
                .map(|&t| default_mask(road.lanes, t, kinds))
                .collect();
            let mut masks = default.clone();
            if !road.turn_lanes.is_empty() {
                let serves = |lane: usize, t: Turn| {
                    road.turn_lanes.get(lane).is_some_and(|a| a & arrow(t) != 0)
                };
                for (m, &t) in turns.iter().enumerate() {
                    masks[m] = (0..road.lanes)
                        .filter(|&l| serves(usize::from(l), t))
                        .fold(0, |mask, l| mask | 1 << l);
                }
                // A lane whose arrows match no movement takes the default set.
                for l in
                    (0..road.lanes).filter(|&l| turns.iter().all(|&t| !serves(usize::from(l), t)))
                {
                    for (m, d) in default.iter().enumerate() {
                        masks[m] |= d & (1 << l);
                    }
                }
                // A movement no lane serves keeps its default lanes.
                for (m, mask) in masks.iter_mut().enumerate() {
                    if *mask == 0 {
                        *mask = default[m];
                    }
                }
            }
            for (i, &(_, to)) in out.iter().enumerate() {
                moves.push(Move {
                    to,
                    turn: turns[i],
                    from_mask: masks[i],
                });
            }
        }
        Self { first, moves }
    }

    fn movement(&self, from: u32, to: u32) -> Option<&Move> {
        let m =
            &self.moves[self.first[from as usize] as usize..self.first[from as usize + 1] as usize];
        m.binary_search_by_key(&to, |m| m.to).ok().map(|i| &m[i])
    }

    /// The lanes of `from` that may take the movement to `to`; all lanes if
    /// it is no connection.
    #[must_use]
    pub fn from_mask(&self, net: &Network, from: u32, to: u32) -> u16 {
        self.movement(from, to).map_or_else(
            || lanes_mask(0, net.roads[from as usize].lanes),
            |m| m.from_mask,
        )
    }

    /// The lane of `to` that a vehicle in `lane` of `from` enters. `after` is
    /// the road it takes next, if any: a short `to` leaves no room to change
    /// lanes, so the vehicle enters the nearest lane that serves that turn.
    #[must_use]
    pub fn entry(&self, net: &Network, from: u32, lane: u8, to: u32, after: Option<u32>) -> u8 {
        let target = &net.roads[to as usize];
        let last = target.lanes - 1;
        let e = match self.movement(from, to).map(|m| m.turn) {
            Some(Turn::Right) => 0,
            Some(Turn::Left | Turn::LeftAcross | Turn::UTurn) => last,
            _ => lane.min(last),
        };
        let mask = after
            .filter(|_| target.length_m < SHORT_ROAD_M)
            .map_or(u16::MAX, |c| self.from_mask(net, to, c));
        (0..=last)
            .filter(|&l| mask & (1 << l) != 0)
            .min_by_key(|&l| (l.abs_diff(e), l))
            .unwrap_or(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{EndControl, Road};

    /// Roads 0..4 enter node 0 heading N, W, S, E; roads 4..8 leave it
    /// heading N, W, S, E. From road 0: 4 is straight, 5 a left turn
    /// across traffic, 7 a right turn.
    fn node(lanes: [u8; 8], turn_lanes: &[u8], connections: Vec<(u32, u32)>) -> Network {
        let dirs = [[0.0, 1.0], [-1.0, 0.0], [0.0, -1.0], [1.0, 0.0]];
        let road = |i: usize| Road {
            from: u32::from(i < 4),
            to: u32::from(i >= 4),
            length_m: 100.0,
            speed_limit_mps: 10.0,
            lanes: lanes[i],
            start_dir: dirs[i % 4],
            end_dir: dirs[i % 4],
            roundabout: false,
            class: 0,
            priority_road: false,
            end_control: EndControl::None,
            turn_lanes: if i == 0 { turn_lanes.to_vec() } else { vec![] },
        };
        Network {
            nodes: vec![[0.0, 0.0]; 2],
            roads: (0..8).map(road).collect(),
            connections,
        }
    }

    fn masks(net: &Network) -> [u16; 3] {
        let l = Lanes::new(net);
        [4, 5, 7].map(|to| l.from_mask(net, 0, to))
    }

    #[test]
    fn default_rule_puts_right_turns_right_left_turns_left_and_straight_anywhere() {
        let all = vec![(0, 4), (0, 5), (0, 7)];
        // [straight, left, right]
        assert_eq!(
            masks(&node([2, 1, 1, 1, 2, 2, 1, 2], &[], all)),
            [0b11, 0b10, 0b01]
        );
        // Without a straight movement the turns share the lanes, right half first.
        let t = vec![(0, 5), (0, 7)];
        let m = masks(&node([3, 1, 1, 1, 2, 2, 1, 2], &[], t));
        assert_eq!([m[1], m[2]], [0b100, 0b011]);
    }

    #[test]
    fn turn_lanes_override_the_default_and_unmatched_lanes_fall_back_to_it() {
        let all = vec![(0, 4), (0, 5), (0, 7)];
        // Lane 0 is left-only, lane 1 through and right.
        let net = node([2, 1, 1, 1, 2, 2, 1, 2], &[1, 6], all);
        assert_eq!(masks(&net), [0b10, 0b01, 0b10]);
        // Lane 0's arrow is a U-turn, which doesn't exist here: lane 0 takes
        // the default set (straight and right).
        let net = node([2, 1, 1, 1, 2, 1, 1, 2], &[8, 6], vec![(0, 4), (0, 7)]);
        assert_eq!([masks(&net)[0], masks(&net)[2]], [0b11, 0b11]);
    }

    #[test]
    fn entry_lanes_follow_the_turn_and_a_lane_drop_merges() {
        let all = vec![(0, 4), (0, 5), (0, 7)];
        let net = node([2, 1, 1, 1, 2, 2, 1, 2], &[], all.clone());
        let l = Lanes::new(&net);
        let e = |lane, to| l.entry(&net, 0, lane, to, None);
        assert_eq!([e(0, 4), e(1, 4), e(1, 5), e(0, 7)], [0, 1, 1, 0]);
        // Road 4 drops to one lane: both lanes enter it.
        let net = node([2, 1, 1, 1, 1, 2, 1, 2], &[], all);
        let l = Lanes::new(&net);
        assert_eq!([0, 1].map(|lane| l.entry(&net, 0, lane, 4, None)), [0, 0]);
    }
}
