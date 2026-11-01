use super::core_types::{Edge, Node, NodeId, NodeName, Nodes, Path};
use crate::error::{PanGraphXError, PanResult};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

/// The parts of a graph, before validation. [`CoreGraphDTO::new`] checks them
/// and [`CoreGraphDTO::into_parts`] gives them back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphParts {
    pub nodes: Nodes,
    pub edges: Vec<Edge>,
    pub paths: Vec<Path>,
    /// Mapping from `NodeId` to original node names (if available).
    pub node_name_map: Option<HashMap<NodeId, NodeName>>,
}

/// The main graph data transfer object (DTO) containing nodes, edges, and paths
/// for serialization and deserialization.
///
/// A `CoreGraphDTO` is always valid: every edge and path step refers to an
/// existing node, every `node_name_map` key is a node, and path names are
/// unique. Code holding one can therefore index nodes directly.
///
/// For graph manipulation consider using `CoreGraph` for efficient lookup and manipulation
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoreGraphDTO {
    nodes: Nodes,
    edges: Vec<Edge>,
    paths: Vec<Path>,
    node_name_map: Option<HashMap<NodeId, NodeName>>,
}

impl CoreGraphDTO {
    /// Validates `parts` and builds a graph from them.
    ///
    /// # Errors
    /// Returns [`PanGraphXError::InvalidGraph`] if an edge or path step refers
    /// to a missing node, a `node_name_map` key is not a node, or two paths
    /// share a name.
    pub fn new(parts: GraphParts) -> PanResult<Self> {
        if let Some(message) = invalid_reference(&parts) {
            return Err(PanGraphXError::InvalidGraph(message));
        }
        Ok(Self::from_valid_parts(parts))
    }

    /// Builds a graph from parts that are already known to be valid, such as
    /// those of a `CoreGraph`, which keeps the same invariants.
    pub(crate) fn from_valid_parts(parts: GraphParts) -> Self {
        let GraphParts {
            nodes,
            edges,
            paths,
            node_name_map,
        } = parts;
        Self {
            nodes,
            edges,
            paths,
            node_name_map,
        }
    }

    /// Takes the graph apart, e.g. to edit it and validate it again with
    /// [`CoreGraphDTO::new`].
    #[must_use]
    pub fn into_parts(self) -> GraphParts {
        GraphParts {
            nodes: self.nodes,
            edges: self.edges,
            paths: self.paths,
            node_name_map: self.node_name_map,
        }
    }

    /// The nodes, indexed by `NodeId`.
    #[must_use]
    pub const fn nodes(&self) -> &Nodes {
        &self.nodes
    }

    /// The edges, in input order.
    #[must_use]
    pub const fn edges(&self) -> &[Edge] {
        self.edges.as_slice()
    }

    /// The paths, in input order.
    #[must_use]
    pub const fn paths(&self) -> &[Path] {
        self.paths.as_slice()
    }

    /// Mapping from `NodeId` to original node names (if available). Nodes
    /// without an entry are named by their id.
    #[must_use]
    pub const fn node_name_map(&self) -> Option<&HashMap<NodeId, NodeName>> {
        self.node_name_map.as_ref()
    }

    #[must_use]
    pub fn get_node_name(&self, node: &Node) -> String {
        self.get_name_from_id(node.id)
    }

    #[must_use]
    pub fn get_name_from_id(&self, node_id: NodeId) -> String {
        if let Some(map) = &self.node_name_map
            && let Some(name) = map.get(&node_id)
        {
            return String::from_utf8_lossy(name).to_string();
        }
        node_id.to_string()
    }
}

/// Describes the first reason `parts` do not form a valid graph, if any.
fn invalid_reference(parts: &GraphParts) -> Option<String> {
    let n = parts.nodes.len();
    if let Some(edge) = parts
        .edges
        .par_iter()
        .find_first(|e| e.from_node >= n || e.to_node >= n)
    {
        return Some(format!(
            "edge {} -> {} refers to a missing node",
            edge.from_node, edge.to_node
        ));
    }
    if let Some(message) = parts.paths.par_iter().find_map_first(|path| {
        path.steps.iter().find(|s| s.node_id() >= n).map(|step| {
            format!(
                "path {} refers to missing node {}",
                String::from_utf8_lossy(&path.name),
                step.node_id()
            )
        })
    }) {
        return Some(message);
    }
    if let Some(id) = parts
        .node_name_map
        .as_ref()
        .and_then(|map| map.keys().filter(|&&id| id >= n).min())
    {
        return Some(format!("node name given for missing node {id}"));
    }
    let mut names = HashSet::with_capacity(parts.paths.len());
    parts
        .paths
        .iter()
        .find(|path| !names.insert(path.name.as_slice()))
        .map(|path| {
            format!(
                "duplicate path name {}",
                String::from_utf8_lossy(&path.name)
            )
        })
}

#[cfg(test)]
impl CoreGraphDTO {
    /// Checks whether two graphs are equal up to node, edge and path order,
    /// matching nodes by name (from `node_name_map` or stringified IDs).
    ///
    /// Duplicate sequences are handled since matching is by name. Edges are
    /// compared as a set in their stored direction (`a+ -> b+` differs from
    /// its mirror `b- -> a-`, and repeated edges are not counted), and path
    /// overlaps are ignored. The integration tests compare graphs with
    /// `tests/common::canonical` instead.
    pub(crate) fn eq_by_node_names(&self, other: &Self) -> bool {
        // Quick cardinality checks
        if self.nodes.len() != other.nodes.len()
            || self.edges.len() != other.edges.len()
            || self.paths.len() != other.paths.len()
        {
            return false;
        }

        // Build name → NodeId maps for both graphs
        let self_name_to_id: HashMap<String, NodeId> = (0..self.nodes.len())
            .map(|id| (self.get_name_from_id(id), id))
            .collect();
        let other_name_to_id: HashMap<String, NodeId> = (0..other.nodes.len())
            .map(|id| (other.get_name_from_id(id), id))
            .collect();

        if self_name_to_id.len() != self.nodes.len() || other_name_to_id.len() != other.nodes.len()
        {
            // ensure all nodes have unique names
            return false;
        }
        // Verify same set of node names
        if self_name_to_id.len() != other_name_to_id.len() {
            return false;
        }

        // Build self NodeId → other NodeId mapping via shared names
        // and verify sequences match
        let mut id_mapping: HashMap<NodeId, NodeId> = HashMap::with_capacity(self.nodes.len());
        for (name, &self_id) in &self_name_to_id {
            let Some(&other_id) = other_name_to_id.get(name) else {
                return false; // name exists in self but not in other
            };
            if self.nodes[self_id].sequence != other.nodes[other_id].sequence {
                return false; // same name, different sequence
            }
            id_mapping.insert(self_id, other_id);
        }

        // Check edges: remap each self-edge via the bijection and look it up in other
        let other_edge_set: HashSet<&Edge> = other.edges.iter().collect();
        let all_edges_match = self.edges.par_iter().all(|edge| {
            let Some(&mapped_from) = id_mapping.get(&edge.from_node) else {
                return false;
            };
            let Some(&mapped_to) = id_mapping.get(&edge.to_node) else {
                return false;
            };
            let remapped = Edge {
                from_node: mapped_from,
                from_orient: edge.from_orient,
                to_node: mapped_to,
                to_orient: edge.to_orient,
                overlap: edge.overlap,
            };
            other_edge_set.contains(&remapped)
        });
        if !all_edges_match {
            return false;
        }

        // Check paths: match by name, then verify steps match after remapping
        let other_paths_by_name: HashMap<&[u8], &Path> =
            other.paths.iter().map(|p| (p.name.as_slice(), p)).collect();

        self.paths.par_iter().all(|self_path| {
            let Some(other_path) = other_paths_by_name.get(self_path.name.as_slice()) else {
                return false; // path name not found
            };
            if self_path.steps.len() != other_path.steps.len() {
                return false;
            }
            self_path
                .steps
                .iter()
                .zip(other_path.steps.iter())
                .all(|(s, o)| {
                    let Some(&mapped_id) = id_mapping.get(&s.node_id()) else {
                        return false;
                    };
                    mapped_id == o.node_id() && s.orientation() == o.orientation()
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::core_types::{Orientation, Step};

    fn parts(edge_to: NodeId, step_node: NodeId) -> GraphParts {
        GraphParts {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec()]),
            edges: vec![Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: edge_to,
                to_orient: Orientation::Forward,
                overlap: 0,
            }],
            paths: vec![Path {
                name: b"p".to_vec(),
                steps: vec![Step::new(step_node, Orientation::Forward).unwrap()],
                overlaps: Vec::new(),
            }],
            node_name_map: None,
        }
    }

    fn rejection(parts: GraphParts) -> String {
        match CoreGraphDTO::new(parts) {
            Err(PanGraphXError::InvalidGraph(message)) => message,
            other => panic!("expected InvalidGraph, got {other:?}"),
        }
    }

    #[test]
    fn valid_parts_round_trip() {
        let graph = CoreGraphDTO::new(parts(1, 1)).unwrap();
        assert_eq!(graph.into_parts(), parts(1, 1));
        assert!(CoreGraphDTO::new(GraphParts::default()).is_ok());
    }

    #[test]
    fn dangling_references_are_rejected() {
        assert!(rejection(parts(2, 1)).contains("edge 0 -> 2"));
        assert!(rejection(parts(1, 5)).contains("node 5"));
    }

    #[test]
    fn names_of_missing_nodes_are_rejected() {
        let mut bad = parts(1, 1);
        bad.node_name_map = Some(HashMap::from([(0, b"a".to_vec()), (7, b"b".to_vec())]));
        assert!(rejection(bad).contains("missing node 7"));
    }

    #[test]
    fn duplicate_path_names_are_rejected() {
        let mut bad = parts(1, 1);
        bad.paths.push(bad.paths[0].clone());
        assert!(rejection(bad).contains("duplicate path name p"));
    }
}
