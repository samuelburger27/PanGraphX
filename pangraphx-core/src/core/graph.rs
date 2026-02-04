use std::collections::HashMap;
use std::fmt::Display;

/// A unique identifier for a node/segment in the graph.
pub type NodeId = usize;
pub type PathName = Vec<u8>;

/// Represents the orientation of a node traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    Forward,
    Reverse,
}

impl Display for Orientation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Orientation::Forward => write!(f, "+"),
            Orientation::Reverse => write!(f, "-"),
        }
    }
}

impl From<bool> for Orientation {
    fn from(is_reverse: bool) -> Self {
        if is_reverse {
            Orientation::Reverse
        } else {
            Orientation::Forward
        }
    }
}

/// A node (or segment) in the graph, containing a DNA sequence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Node {
    // DNA sequence of the node
    pub sequence: Vec<u8>,
    // Unique identifier for the node (also used as index into node vector)
    pub id: NodeId,
}

/// An edge (or link) connecting two nodes with specific orientations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Edge {
    pub from_node: NodeId,
    pub from_orient: Orientation,
    pub to_node: NodeId,
    pub to_orient: Orientation,
    /// Represents overlap
    pub overlap: u32,
}

/// A single step in a path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Step {
    pub node_id: NodeId,
    pub orientation: Orientation,
}

/// A named, ordered traversal through nodes in the graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Path {
    pub name: PathName,
    pub steps: Vec<Step>,
    pub overlaps: Vec<u32>,
}

/// The main graph data transfer object (DTO) containing nodes, edges, and paths
/// for serialization and deserialization.
///
/// For graph manipulation consider using CoreGraph for efficient lookup and manipulation
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreGraphDTO {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub paths: Vec<Path>,
    /// mapping from NodeId to original node names (if available).
    pub node_name_map: Option<HashMap<NodeId, Vec<u8>>>,
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
