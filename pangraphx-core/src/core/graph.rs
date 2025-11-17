use std::fmt::Display;

use gfa::cigar::CIGAR;

/// A unique identifier for a node/segment in the graph.
pub type NodeId = Vec<u8>;
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
    pub id: NodeId,
    // TODO: use a more efficient representation for sequences
    pub sequence: Vec<u8>,
}

/// An edge (or link) connecting two nodes with specific orientations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Edge {
    pub from_node: NodeId,
    pub from_orient: Orientation,
    pub to_node: NodeId,
    pub to_orient: Orientation,
    /// Represents overlap
    pub overlap: Vec<u8>,
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
    pub overlaps: Vec<Option<CIGAR>>,
}

/// The central, in-memory representation of a genome graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CoreGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub paths: Vec<Path>,
}
