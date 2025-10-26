use gfa::gfa::orientation::Orientation as GFAOrientation;
use gfa::gfa::{GFA, SegmentId};
use gfa::optfields::OptFields;
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
    pub name: String,
    pub steps: Vec<Step>,
    pub overlap: Vec<Vec<u8>>,
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

impl CoreGraph {
    pub fn from_gfa<T: OptFields>(gfa: GFA<usize, T>) -> Self {
        let mut graph = CoreGraph::default();
        let nodes: HashMap<NodeId, Node> = gfa
            .segments
            .into_iter()
            .map(|seq| {
                let node_id = seq.name as NodeId;
                let sequence = seq.sequence;
                (
                    node_id,
                    Node {
                        id: node_id,
                        sequence,
                    },
                )
            })
            .collect();
        let edges: Vec<Edge> = gfa
            .links
            .into_iter()
            .map(|link| Edge {
                from_node: link.from_segment as NodeId,
                from_orient: Orientation::from_gfa_lib(link.from_orient),
                to_node: link.to_segment as NodeId,
                to_orient: Orientation::from_gfa_lib(link.to_orient),
                overlap: link.overlap,
            })
            .collect();

        let paths = gfa
            .paths
            .into_iter()
            .map(|p| {
                // TODO finish
            })
            .collect();
        graph
    }
}
