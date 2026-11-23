//! Stable keys for network items, for edits that must survive a re-import
//! (ADR-0008); see [`Refs`].

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Network, NodeId, OsmWayPart, RoadId};

/// An edit's id, unique within a scenario, e.g. `close-3` or `slow-b47`.
pub type EditId = String;

/// A node's stable key: its OSM node id, or the edit that created it.
///
/// Reads `{ node = 15837308 }` or `{ edit = "roundabout-1", n = 0 }`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NodeRef {
    /// A node imported from OpenStreetMap ([`Node::osm_node`](crate::Node::osm_node)).
    Osm {
        /// OSM node id.
        node: i64,
    },
    /// The `n`-th node created by edit `edit`.
    Created {
        /// The creating edit.
        edit: EditId,
        /// Number within that edit, from 0.
        n: u16,
    },
}

/// A road's stable key: the OSM way part it was imported from, or the edit
/// that created it.
///
/// Reads `{ way = 4514502, from_node = 15837308, to_node = 15837309 }` or
/// `{ edit = "roundabout-1", n = 2 }`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RoadRef {
    /// A road imported from OpenStreetMap ([`Road::osm`](crate::Road::osm)).
    Osm(OsmWayPart),
    /// The `n`-th road created by edit `edit`.
    Created {
        /// The creating edit.
        edit: EditId,
        /// Number within that edit, from 0.
        n: u16,
    },
}

impl fmt::Display for NodeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Osm { node } => write!(f, "node {node}"),
            Self::Created { edit, n } => write!(f, "node {n} of edit `{edit}`"),
        }
    }
}

impl fmt::Display for RoadRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Osm(p) => write!(f, "way {} from {} to {}", p.way, p.from_node, p.to_node),
            Self::Created { edit, n } => write!(f, "road {n} of edit `{edit}`"),
        }
    }
}

/// Why a key did not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Unresolved {
    /// No item has this key. `near` lists roads of the same way and roads
    /// sharing an end node with the key, in key order: a hint for the
    /// message only, never applied.
    NotFound {
        /// Roads that may be what the key meant.
        near: Vec<RoadRef>,
    },
    /// An earlier edit removed the item.
    Deleted {
        /// The removing edit.
        by: EditId,
    },
}

/// Maps the stable keys of one network's items to their [`NodeId`]s and
/// [`RoadId`]s and back (ADR-0008 §2). Lookups are exact: a key resolves to
/// the item with exactly that key or not at all.
#[derive(Debug, Clone, Default)]
pub struct Refs {
    road_ids: HashMap<RoadRef, RoadId>,
    node_ids: HashMap<NodeRef, NodeId>,
    road_keys: Vec<Option<RoadRef>>,
    node_keys: Vec<Option<NodeRef>>,
    deleted_roads: HashMap<RoadRef, EditId>,
    deleted_nodes: HashMap<NodeRef, EditId>,
}

/// Index of the `i`-th item of a validated network.
#[allow(clippy::cast_possible_truncation)]
fn id(i: usize) -> u32 {
    i as u32
}

impl Refs {
    /// The keys of `network`'s items. Its keys must be unique, as network
    /// validation ensures; of two equal keys, the later item would win.
    #[must_use]
    pub fn new(network: &Network) -> Self {
        let node_keys: Vec<_> = (network.nodes.iter())
            .map(|n| n.osm_node.map(|node| NodeRef::Osm { node }))
            .collect();
        let road_keys: Vec<_> = (network.roads.iter())
            .map(|r| r.osm.map(RoadRef::Osm))
            .collect();
        let mut refs = Self::default();
        for (i, key) in node_keys.iter().enumerate() {
            if let Some(key) = key {
                refs.node_ids.insert(key.clone(), NodeId(id(i)));
            }
        }
        for (i, key) in road_keys.iter().enumerate() {
            if let Some(key) = key {
                refs.road_ids.insert(key.clone(), RoadId(id(i)));
            }
        }
        Self {
            road_keys,
            node_keys,
            ..refs
        }
    }

    /// The road with key `key`.
    ///
    /// # Errors
    ///
    /// [`Unresolved`] if no road has the key.
    pub fn road(&self, key: &RoadRef) -> Result<RoadId, Unresolved> {
        if let Some(&id) = self.road_ids.get(key) {
            return Ok(id);
        }
        if let Some(by) = self.deleted_roads.get(key) {
            return Err(Unresolved::Deleted { by: by.clone() });
        }
        let near = match key {
            RoadRef::Osm(k) => self.near(|p| {
                p.way == k.way
                    || [p.from_node, p.to_node]
                        .iter()
                        .any(|n| [k.from_node, k.to_node].contains(n))
            }),
            RoadRef::Created { .. } => vec![],
        };
        Err(Unresolved::NotFound { near })
    }

    /// The node with key `key`.
    ///
    /// # Errors
    ///
    /// [`Unresolved`] if no node has the key; `near` lists the roads whose
    /// key ends at that OSM node.
    pub fn node(&self, key: &NodeRef) -> Result<NodeId, Unresolved> {
        if let Some(&id) = self.node_ids.get(key) {
            return Ok(id);
        }
        if let Some(by) = self.deleted_nodes.get(key) {
            return Err(Unresolved::Deleted { by: by.clone() });
        }
        let near = match key {
            &NodeRef::Osm { node } => self.near(|p| p.from_node == node || p.to_node == node),
            NodeRef::Created { .. } => vec![],
        };
        Err(Unresolved::NotFound { near })
    }

    /// The key of `road`; `None` if it has none (or is out of range).
    #[must_use]
    pub fn road_ref(&self, road: RoadId) -> Option<&RoadRef> {
        self.road_keys.get(road.0 as usize)?.as_ref()
    }

    /// The key of `node`; `None` if it has none (or is out of range).
    #[must_use]
    pub fn node_ref(&self, node: NodeId) -> Option<&NodeRef> {
        self.node_keys.get(node.0 as usize)?.as_ref()
    }

    /// Marks the key of `node` as removed by edit `by`.
    // shortcut: no M5.1 edit removes items; M5.3's roundabout edit is the first caller.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn delete_node(&mut self, node: NodeId, by: &str) {
        if let Some(key) = self
            .node_keys
            .get_mut(node.0 as usize)
            .and_then(Option::take)
        {
            self.node_ids.remove(&key);
            self.deleted_nodes.insert(key, by.to_owned());
        }
    }

    /// OSM road keys matching `f`, in key order.
    fn near(&self, f: impl Fn(&OsmWayPart) -> bool) -> Vec<RoadRef> {
        let mut near: Vec<RoadRef> = (self.road_ids.keys())
            .filter(|k| matches!(k, RoadRef::Osm(p) if f(p)))
            .cloned()
            .collect();
        near.sort_unstable();
        near
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, Point};

    #[test]
    fn a_deleted_node_names_the_edit_that_removed_it() {
        let node = |osm| Node {
            position: Point { x: 0.0, y: 0.0 },
            osm_node: Some(osm),
        };
        let net = Network {
            nodes: vec![node(7), node(8)],
            ..Network::default()
        };
        let mut refs = Refs::new(&net);
        let key = NodeRef::Osm { node: 8 };
        assert_eq!(refs.node(&key), Ok(NodeId(1)));
        refs.delete_node(NodeId(1), "roundabout-1");
        assert_eq!(
            refs.node(&key),
            Err(Unresolved::Deleted {
                by: "roundabout-1".into()
            })
        );
        assert_eq!(refs.node_ref(NodeId(1)), None);
        assert_eq!(refs.node_ref(NodeId(0)), Some(&NodeRef::Osm { node: 7 }));
    }
}
