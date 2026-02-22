use super::core_types::{Edge, Node, NodeId, NodeName, Path, Nodes};
use std::collections::HashMap;

/// The main graph data transfer object (DTO) containing nodes, edges, and paths
/// for serialization and deserialization.
///
/// For graph manipulation consider using CoreGraph for efficient lookup and manipulation
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoreGraphDTO {
    pub(crate) nodes: Nodes,
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
