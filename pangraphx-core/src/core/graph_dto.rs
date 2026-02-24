use crate::PathName;

use super::core_types::{Edge, Node, NodeId, NodeName, Nodes, Path, Sequence};
use std::collections::{HashMap, HashSet};

/// The main graph data transfer object (DTO) containing nodes, edges, and paths
/// for serialization and deserialization.
///
/// For graph manipulation consider using CoreGraph for efficient lookup and manipulation
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoreGraphDTO {
    pub nodes: Nodes,
    pub edges: Vec<Edge>,
    pub paths: Vec<Path>,
    /// mapping from NodeId to original node names (if available).
    pub node_name_map: Option<HashMap<NodeId, NodeName>>,
}

impl CoreGraphDTO {
    pub fn get_node_name(&self, node: &Node) -> String {
        self.get_name_from_id(node.id)
    }

    pub fn get_name_from_id(&self, node_id: NodeId) -> String {
        if let Some(map) = &self.node_name_map {
            if let Some(name) = map.get(&node_id) {
                return String::from_utf8_lossy(name).to_string();
            }
        }
        node_id.to_string()
    }
}

impl CoreGraphDTO {
    /// Checks if two CoreGraphDTO instances are isomorphic, meaning they represent the same graph structure and contain same sequences, regardless of the order of nodes, edges, and paths.
    pub fn isomorphic(&self, other: &CoreGraphDTO) -> bool {
        // Check if the number of nodes, edges, and paths are the same
        if self.nodes.len() != other.nodes.len()
            || self.edges.len() != other.edges.len()
            || self.paths.len() != other.paths.len()
        {
            return false;
        }

        let self_sequences: HashSet<&[u8]> = self
            .nodes
            .iter()
            .map(|node| node.sequence.as_slice())
            .collect();

        let other_sequences: HashSet<&[u8]> = other
            .nodes
            .iter()
            .map(|node| node.sequence.as_slice())
            .collect();

        if self_sequences != other_sequences {
            return false;
        }

        let self_node_map: HashMap<&[u8], NodeId> = self
            .nodes
            .iter()
            .map(|node| (node.sequence.as_slice(), node.id))
            .collect();

        let other_node_map: HashMap<&[u8], NodeId> = other
            .nodes
            .iter()
            .map(|node| (node.sequence.as_slice(), node.id))
            .collect();

        let other_edge_set: HashSet<&Edge> = other.edges.iter().collect();

        // Check if all edges in self have a corresponding edge in other
        for edge in &self.edges {
            let from_seq = self.nodes[edge.from_node].sequence.as_slice();
            let to_seq = self.nodes[edge.to_node].sequence.as_slice();
            let other_from = other_node_map.get(from_seq).unwrap();
            let other_to = other_node_map.get(to_seq).unwrap();

            let other_expected_edge = Edge {
                from_node: *other_from,
                from_orient: edge.from_orient,
                to_node: *other_to,
                to_orient: edge.to_orient,
                overlap: edge.overlap,
            };
            if !other_edge_set.contains(&other_expected_edge) {
                return false;
            }
        }

        // Check if all paths in self have a corresponding path in other
        let other_path_set: HashSet<PathName> = other.paths.iter().map(|path| path.name.clone()).collect();
        let self_path_set: HashSet<PathName> = self.paths.iter().map(|path| path.name.clone()).collect();

        // TODO for now only check if path names are the same, but ideally should also check if the steps and overlaps are the same (ignoring node IDs)
        self_path_set == other_path_set
    }
}
