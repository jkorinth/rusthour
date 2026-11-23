//! Network edits as serializable patches (M5.1, ADR-0008); see [`apply`].

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::refs::{EditId, NodeRef, Refs, RoadRef, Unresolved};
use crate::{EndControl, MAX_LANES, Network, NetworkError, RoadId, Turns};

/// The map a patch was made on: the `.rhnet` metadata's `area` and
/// `source_date`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapRef {
    /// Area name, e.g. `bensheim-5km`.
    pub area: String,
    /// Date of the OSM extract, `YYYY-MM-DD`.
    pub source_date: String,
}

/// A list of edits, applied in order. In TOML:
///
/// ```toml
/// [map]
/// area = "bensheim-5km"
/// source_date = "2026-11-10"
///
/// [[edit]]
/// kind = "speed_limit"
/// id = "slow-b47"
/// road = { way = 198045888, from_node = 15837308, to_node = 1573379680 }
/// kmh = 30
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    /// The map the patch was made on; a different one gets a warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<MapRef>,
    /// The edits, in order.
    #[serde(rename = "edit", default)]
    pub edits: Vec<Edit>,
}

/// One change to the network. Each has an `id`, unique across all patches
/// of a run, and addresses its target by stable key (ADR-0008).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Edit {
    /// Sets the control at the end of every road into a junction: all-way
    /// stop or give-way, signals, or none (right of way by road class).
    JunctionControl {
        /// Edit id.
        id: EditId,
        /// The junction.
        junction: NodeRef,
        /// The new control.
        control: EndControl,
    },
    /// Sets the control at a road's end, for traffic on that road.
    ArmControl {
        /// Edit id.
        id: EditId,
        /// The road.
        road: RoadRef,
        /// The new control.
        control: EndControl,
    },
    /// Sets a road's lane count. Its turn lanes are kept if the count stays
    /// the same, else dropped (with a warning).
    Lanes {
        /// Edit id.
        id: EditId,
        /// The road.
        road: RoadRef,
        /// Lanes, 1 to [`MAX_LANES`].
        lanes: u8,
    },
    /// Sets a road's turn lanes and, from their number, its lane count.
    TurnLanes {
        /// Edit id.
        id: EditId,
        /// The road.
        road: RoadRef,
        /// As OSM `turn:lanes`, lanes left to right: `left|through;right`
        /// ([`Turns::parse_osm_lanes`]).
        turn_lanes: String,
    },
    /// Sets a road's speed limit.
    SpeedLimit {
        /// Edit id.
        id: EditId,
        /// The road.
        road: RoadRef,
        /// The limit in km/h, 1 to 250.
        kmh: f64,
    },
    /// Closes a road: no traffic enters, leaves or starts on it.
    Close {
        /// Edit id.
        id: EditId,
        /// The road (one direction).
        road: RoadRef,
    },
    /// Reopens a road an earlier edit closed.
    Reopen {
        /// Edit id.
        id: EditId,
        /// The road (one direction).
        road: RoadRef,
    },
}

impl Edit {
    /// The edit's id.
    #[must_use]
    pub fn id(&self) -> &EditId {
        match self {
            Self::JunctionControl { id, .. }
            | Self::ArmControl { id, .. }
            | Self::Lanes { id, .. }
            | Self::TurnLanes { id, .. }
            | Self::SpeedLimit { id, .. }
            | Self::Close { id, .. }
            | Self::Reopen { id, .. } => id,
        }
    }
}

/// What an edit targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A road.
    Road(RoadRef),
    /// A junction.
    Node(NodeRef),
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Road(key) => write!(f, "road {key}"),
            Self::Node(key) => write!(f, "junction {key}"),
        }
    }
}

/// Why patches could not be applied. [`apply`] reports all of them.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PatchError {
    /// An edit's target is not on this map.
    Unresolved {
        /// The edit.
        edit: EditId,
        /// Its target.
        target: Target,
        /// Why the key did not resolve.
        why: Unresolved,
    },
    /// Two edits have the same id.
    DuplicateEditId {
        /// The id.
        edit: EditId,
    },
    /// An edit's value is out of range, or its target can't take it.
    Invalid {
        /// The edit.
        edit: EditId,
        /// What is wrong.
        reason: String,
    },
    /// The map itself can't be patched: two items share a key.
    Map(NetworkError),
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unresolved {
                edit,
                target,
                why: Unresolved::Deleted { by },
            } => write!(
                f,
                "edit `{edit}` targets {target}, which edit `{by}` removed"
            ),
            Self::Unresolved { edit, target, why } => {
                write!(
                    f,
                    "edit `{edit}` targets {target}, which this map doesn't have"
                )?;
                let near = match why {
                    Unresolved::NotFound { near } if !near.is_empty() => near,
                    _ => return Ok(()),
                };
                let list: Vec<String> = near.iter().map(ToString::to_string).collect();
                let (which, why) = match target {
                    Target::Road(_) => (
                        "of the same way or ending at its nodes",
                        "way split or redrawn",
                    ),
                    Target::Node(_) => (
                        "whose OSM way part ends at it",
                        "junction merged or redrawn",
                    ),
                };
                write!(
                    f,
                    "; roads here {which}: {} (was the {why} since the patch was made? Nothing is \
                     applied until the key is fixed)",
                    list.join(", ")
                )
            }
            Self::DuplicateEditId { edit } => write!(
                f,
                "edit id `{edit}` is used more than once; ids must be unique across all patches"
            ),
            Self::Invalid { edit, reason } => write!(f, "edit `{edit}`: {reason}"),
            Self::Map(e) => write!(f, "the map can't be patched: {e}"),
        }
    }
}

impl std::error::Error for PatchError {}

/// Something [`apply`] did that the user may not expect.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PatchWarning {
    /// A patch was made on another map (area or extract date); targets were
    /// still matched by key.
    OtherMap {
        /// The map the patch names.
        patch: MapRef,
        /// The map it was applied to.
        map: MapRef,
    },
    /// A later edit set the same attribute of the same road; it wins.
    Overrides {
        /// The later edit.
        edit: EditId,
        /// The edit it overrides.
        earlier: EditId,
        /// The attribute, e.g. `speed limit`.
        attribute: &'static str,
        /// The road.
        road: RoadId,
        /// Its key, if it has one.
        key: Option<RoadRef>,
    },
    /// A lane count change dropped a road's turn lanes.
    TurnLanesDropped {
        /// The edit.
        edit: EditId,
        /// The road.
        road: RoadId,
        /// Its key, if it has one.
        key: Option<RoadRef>,
    },
    /// Closures cut roads off the part of the map trips use.
    CutOff {
        /// The closing edits.
        edits: Vec<EditId>,
        /// How many roads are cut off.
        count: usize,
        /// Their keys (those that have one), in key order.
        roads: Vec<RoadRef>,
    },
}

impl fmt::Display for PatchWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OtherMap { patch, map } => write!(
                f,
                "patch made on {} of {}, applied to {} of {}; targets matched by OSM ids",
                patch.area, patch.source_date, map.area, map.source_date
            ),
            Self::Overrides {
                edit,
                earlier,
                attribute,
                road,
                key,
            } => write!(
                f,
                "edit `{edit}` overrides `{earlier}` on the {attribute} of {}",
                Named(*road, key.as_ref())
            ),
            Self::TurnLanesDropped { edit, road, key } => write!(
                f,
                "edit `{edit}` changes the lane count of {}, so its turn lanes are dropped (add \
                 a turn_lanes edit to set new ones)",
                Named(*road, key.as_ref())
            ),
            Self::CutOff {
                edits,
                count,
                roads,
            } => {
                const SHOWN: usize = 5;
                let ids: Vec<String> = edits.iter().map(|e| format!("`{e}`")).collect();
                let shown: Vec<String> =
                    roads.iter().take(SHOWN).map(ToString::to_string).collect();
                write!(
                    f,
                    "closing roads ({}) cuts off {count} roads from the rest of the map, so no \
                     trips start, end or pass there: {}",
                    ids.join(", "),
                    shown.join(", ")
                )?;
                if roads.len() > SHOWN {
                    write!(f, " and {} more", roads.len() - SHOWN)?;
                }
                Ok(())
            }
        }
    }
}

/// A road by key, or by id where it has none.
struct Named<'a>(RoadId, Option<&'a RoadRef>);

impl fmt::Display for Named<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.1 {
            Some(key) => write!(f, "road {key}"),
            None => write!(f, "road {} (no OSM key)", self.0.0),
        }
    }
}

/// The attributes edits set, for conflict warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Attribute {
    Control,
    Lanes,
    SpeedLimit,
    Open,
}

impl Attribute {
    fn name(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Lanes => "lanes",
            Self::SpeedLimit => "speed limit",
            Self::Open => "closure",
        }
    }
}

/// The network being patched and what [`apply`] has seen so far.
struct Work<'a> {
    net: Network,
    refs: Refs,
    closed: Vec<Option<&'a EditId>>,
    set_by: HashMap<(Attribute, RoadId), &'a EditId>,
    warnings: Vec<PatchWarning>,
    errors: Vec<PatchError>,
}

impl<'a> Work<'a> {
    fn road(&mut self, edit: &EditId, key: &RoadRef) -> Option<RoadId> {
        self.refs
            .road(key)
            .map_err(|why| {
                self.errors.push(PatchError::Unresolved {
                    edit: edit.clone(),
                    target: Target::Road(key.clone()),
                    why,
                });
            })
            .ok()
    }

    fn invalid(&mut self, edit: &EditId, reason: String) {
        self.errors.push(PatchError::Invalid {
            edit: edit.clone(),
            reason,
        });
    }

    /// Records that `edit` sets `attribute` of `road`, warning if an earlier
    /// edit did.
    fn set(&mut self, edit: &'a EditId, attribute: Attribute, road: RoadId) {
        if let Some(earlier) = self.set_by.insert((attribute, road), edit) {
            self.warnings.push(PatchWarning::Overrides {
                edit: edit.clone(),
                earlier: earlier.clone(),
                attribute: attribute.name(),
                road,
                key: self.refs.road_ref(road).cloned(),
            });
        }
    }

    fn edit(&mut self, edit: &'a Edit) {
        let id = edit.id();
        match edit {
            Edit::JunctionControl {
                junction, control, ..
            } => {
                let node = match self.refs.node(junction) {
                    Ok(node) => node,
                    Err(why) => {
                        return self.errors.push(PatchError::Unresolved {
                            edit: id.clone(),
                            target: Target::Node(junction.clone()),
                            why,
                        });
                    }
                };
                let arms: Vec<RoadId> = (0u32..)
                    .zip(&self.net.roads)
                    .filter(|(_, r)| r.to_node == node)
                    .map(|(i, _)| RoadId(i))
                    .collect();
                if arms.is_empty() {
                    return self.invalid(id, format!("no road ends at junction {junction}"));
                }
                for road in arms {
                    self.set(id, Attribute::Control, road);
                    self.net.roads[road.0 as usize].end_control = *control;
                }
            }
            Edit::ArmControl { road, control, .. } => {
                if let Some(road) = self.road(id, road) {
                    self.set(id, Attribute::Control, road);
                    self.net.roads[road.0 as usize].end_control = *control;
                }
            }
            Edit::Lanes { road, lanes, .. } => {
                let road = self.road(id, road);
                if !(1..=MAX_LANES).contains(lanes) {
                    return self
                        .invalid(id, format!("lanes must be 1 to {MAX_LANES}, got {lanes}"));
                }
                if let Some(road) = road {
                    self.set(id, Attribute::Lanes, road);
                    let r = &mut self.net.roads[road.0 as usize];
                    if r.lanes != *lanes && !r.turn_lanes.is_empty() {
                        r.turn_lanes.clear();
                        self.warnings.push(PatchWarning::TurnLanesDropped {
                            edit: id.clone(),
                            road,
                            key: self.refs.road_ref(road).cloned(),
                        });
                    }
                    self.net.roads[road.0 as usize].lanes = *lanes;
                }
            }
            Edit::TurnLanes {
                road, turn_lanes, ..
            } => {
                let road = self.road(id, road);
                let turns = Turns::parse_osm_lanes(turn_lanes)
                    .filter(|t| t.len() <= usize::from(MAX_LANES));
                let Some(turns) = turns else {
                    return self.invalid(
                        id,
                        format!(
                            "turn_lanes `{turn_lanes}` must list 1 to {MAX_LANES} lanes left to \
                             right, separated by `|`, each made of left, through, right or \
                             reverse joined by `;` (as OSM turn:lanes)"
                        ),
                    );
                };
                if let Some(road) = road {
                    self.set(id, Attribute::Lanes, road);
                    let r = &mut self.net.roads[road.0 as usize];
                    r.lanes = u8::try_from(turns.len()).expect("at most MAX_LANES");
                    r.turn_lanes = turns;
                }
            }
            Edit::SpeedLimit { road, kmh, .. } => {
                let road = self.road(id, road);
                if !(1.0..=250.0).contains(kmh) {
                    return self.invalid(id, format!("kmh must be 1 to 250, got {kmh}"));
                }
                if let Some(road) = road {
                    self.set(id, Attribute::SpeedLimit, road);
                    self.net.roads[road.0 as usize].speed_limit_mps = kmh / 3.6;
                }
            }
            Edit::Close { road, .. } | Edit::Reopen { road, .. } => {
                if let Some(road) = self.road(id, road) {
                    self.set(id, Attribute::Open, road);
                    self.closed[road.0 as usize] = matches!(edit, Edit::Close { .. }).then_some(id);
                }
            }
        }
    }

    /// Takes closed roads out of traffic: no movements into or out of them,
    /// and no signal phase for them. Warns about roads that leaves without
    /// routes.
    fn close_roads(&mut self, base: &Network) {
        let is_closed = |r: RoadId| self.closed[r.0 as usize].is_some();
        if self.closed.iter().all(Option::is_none) {
            return;
        }
        let closed = &self.closed;
        self.net.connections.retain(|c| {
            closed[c.from_road.0 as usize].is_none() && closed[c.to_road.0 as usize].is_none()
        });
        let mut edits: Vec<EditId> = Vec::new();
        for (r, by) in self.net.roads.iter_mut().zip(closed) {
            if let Some(by) = by {
                r.end_control = EndControl::None;
                if !edits.contains(by) {
                    edits.push((*by).clone());
                }
            }
        }
        let (Ok(before), Ok(after)) = (routable(base), routable(&self.net)) else {
            return; // invalid edits; building the simulation reports them
        };
        let cut: Vec<RoadId> = (before.difference(&after).copied())
            .filter(|&r| !is_closed(r))
            .collect();
        if cut.is_empty() {
            return;
        }
        let mut roads: Vec<RoadRef> = cut
            .iter()
            .filter_map(|&r| self.refs.road_ref(r).cloned())
            .collect();
        roads.sort_unstable();
        self.warnings.push(PatchWarning::CutOff {
            edits,
            count: cut.len(),
            roads,
        });
    }
}

/// The roads trips can use: the router's largest strongly connected part.
fn routable(net: &Network) -> Result<HashSet<RoadId>, NetworkError> {
    let router = rusthour_core::routing::Router::new(&net.to_core()?);
    Ok(router.routable().iter().map(|&r| RoadId(r)).collect())
}

/// Applies `patches` in order, each one's edits in file order, to `base`,
/// the network of map `map` (ADR-0008 §4).
///
/// Every edit resolves its targets on the network as the edits before it
/// left it. Nothing is applied unless everything resolves and is valid: all
/// failures are reported together, in edit order. Items keep their ids, so
/// the returned [`Refs`] also serves the patched network.
///
/// # Errors
///
/// Every [`PatchError`] found, in edit order.
pub fn apply(
    base: &Network,
    map: &MapRef,
    patches: &[Patch],
) -> Result<(Network, Refs, Vec<PatchWarning>), Vec<PatchError>> {
    base.check_keys().map_err(|e| vec![PatchError::Map(e)])?;
    let mut work = Work {
        net: base.clone(),
        refs: Refs::new(base),
        closed: vec![None; base.roads.len()],
        set_by: HashMap::new(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let mut ids = HashSet::new();
    let mut other_maps = Vec::new();
    for patch in patches {
        if let Some(made_on) = &patch.map
            && made_on != map
            && !other_maps.contains(made_on)
        {
            other_maps.push(made_on.clone());
            work.warnings.push(PatchWarning::OtherMap {
                patch: made_on.clone(),
                map: map.clone(),
            });
        }
        for edit in &patch.edits {
            if !ids.insert(edit.id()) {
                work.errors.push(PatchError::DuplicateEditId {
                    edit: edit.id().clone(),
                });
            }
            work.edit(edit);
        }
    }
    if !work.errors.is_empty() {
        return Err(work.errors);
    }
    work.close_roads(base);
    Ok((work.net, work.refs, work.warnings))
}
