use super::core_types::{Edge, Node, NodeId, NodeName, Nodes, Path};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

/// The main graph data transfer object (DTO) containing nodes, edges, and paths
/// for serialization and deserialization.
///
/// For graph manipulation consider using `CoreGraph` for efficient lookup and manipulation
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoreGraphDTO {
    pub nodes: Nodes,
    pub edges: Vec<Edge>,
    pub paths: Vec<Path>,
    /// mapping from `NodeId` to original node names (if available).
    pub node_name_map: Option<HashMap<NodeId, NodeName>>,
}

impl CoreGraphDTO {
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

impl CoreGraphDTO {
    /// Checks if two `CoreGraphDTO` instances are isomorphic, meaning they represent
    /// the same graph structure and contain the same sequences, regardless of the
    /// internal ordering of nodes, edges, and paths.
    ///
    /// Uses node names (from `node_name_map` or stringified IDs) as the canonical
    /// identity for building a bijection between the two graphs. This correctly
    /// handles graphs with duplicate sequences.
    #[must_use]
    pub fn isomorphic(&self, other: &Self) -> bool {
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
                    let Some(&mapped_id) = id_mapping.get(&s.node_id) else {
                        return false;
                    };
                    mapped_id == o.node_id && s.orientation == o.orientation
                })
        })
    }
}
