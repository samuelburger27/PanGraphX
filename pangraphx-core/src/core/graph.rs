use gfa::cigar::CIGAR;
use gfa::gfa::orientation::Orientation as GFAOrientation;
use std::collections::HashMap;
/// A unique identifier for a node/segment in the graph.
pub type NodeId = u64;

/// Represents the orientation of a node traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    Forward,
    Reverse,
}

impl Orientation {
    /// Create an Orientation from internal GFA representation
    pub fn from_gfa_lib(orientation: GFAOrientation) -> Self {
        match orientation {
            GFAOrientation::Forward => Orientation::Forward,
            GFAOrientation::Backward => Orientation::Reverse,
        }
    }
}

/// A node (or segment) in the graph, containing a DNA sequence.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub sequence: Vec<u8>, // Using Vec<u8> is efficient for ASCII/DNA
}

/// An edge (or link) connecting two nodes with specific orientations.
#[derive(Debug, Clone)]
pub struct Edge {
    pub from_node: NodeId,
    pub from_orient: Orientation,
    pub to_node: NodeId,
    pub to_orient: Orientation,
    /// Represents overlap
    pub overlap: Vec<u8>,
}

/// A single step in a path.
#[derive(Debug, Clone, Copy)]
pub struct Step {
    pub node_id: NodeId,
    pub orientation: Orientation,
}

/// A named, ordered traversal through nodes in the graph.
#[derive(Debug, Clone)]
pub struct Path {
    pub name: Vec<u8>,
    pub steps: Vec<Step>,
    pub overlaps: Vec<Option<CIGAR>>,
}

/// The central, in-memory representation of a genome graph.
#[derive(Debug, Default)]
pub struct CoreGraph {
    // TODO maybe use Vec instead of HashMap, further benchmarks needed
    // TODO nodes uses NodeId which is also in the Node struct, redundant?
    /// Stores all nodes, keyed by their ID for fast lookup.
    pub nodes: HashMap<NodeId, Node>,
    /// Stores all edges.
    pub edges: Vec<Edge>,
    /// Stores all paths, keyed by their name.
    pub paths: HashMap<Vec<u8>, Path>,
}
