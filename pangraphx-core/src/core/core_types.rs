use crate::{PanResult, error::PanGraphXError};
use std::fmt::Display;
use std::ops::{Index, IndexMut};

/// A unique identifier for a node/segment in the graph.
pub type NodeId = usize;

pub type PathName = Vec<u8>;
pub type NodeName = Vec<u8>;
pub type Sequence = Vec<u8>;

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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Node {
    // DNA sequence of the node
    pub sequence: Sequence,
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

/// A collection of nodes in the graph.
/// Wraps a vector of `Node` and enforces the invariant that node IDs correspond to their index in the vector.
/// Invariant: Node IDs are unique and correspond to their index in the vector.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Nodes(Vec<Node>);

impl Nodes {
    /// Creates a new, empty collection of nodes.
    pub fn new() -> Self {
        Self(Vec::new())
    }
    /// Creates a new `Nodes` collection from a vector of sequences
    pub fn from_seq(nodes: Vec<Sequence>) -> Self {
        let nodes = nodes
            .into_iter()
            .enumerate()
            .map(|(id, sequence)| Node { id, sequence })
            .collect();
        Self(nodes)
    }

    /// Creates a `Nodes` collection from a vector of `Node`, ensuring that node IDs are unique and correspond to their index.
    pub fn from_node_vec(nodes: Vec<Node>) -> PanResult<Self> {
        // Ensure node IDs are unique and correspond to their index
        for (index, node) in nodes.iter().enumerate() {
            if node.id != index {
                return Err(PanGraphXError::Other(format!(
                    "Node ID {} does not match its index {}",
                    node.id, index
                )));
            }
        }
        Ok(Self(nodes))
    }

    /// Adds a new node with the given sequence, automatically assigning it the next available ID.
    pub fn push(&mut self, sequence: Sequence) {
        let id = self.0.len();
        self.0.push(Node { id, sequence });
    }

    /// Returns the number of nodes in the collection.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Checks if the collection of nodes is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get_mut(&mut self, node_id: NodeId) -> Option<&mut Node> {
        self.0.get_mut(node_id)
    }

    pub fn get(&self, node_id: NodeId) -> Option<&Node> {
        self.0.get(node_id)
    }

    /// Returns an iterator over the nodes in the collection.
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.0.iter()
    }

    /// Returns a mutable iterator over the nodes in the collection.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Node> {
        self.0.iter_mut()
    }

    /// Removes a node by its ID and returns it if it exists.
    /// Note: This will shift all subsequent nodes down by one and update their IDs accordingly.
    pub fn remove(&mut self, node_id: NodeId) -> Option<Node> {
        if node_id < self.0.len() {
            for i in (node_id + 1)..self.0.len() {
                self.0[i].id = i - 1;
            }
            Some(self.0.remove(node_id))
        } else {
            None
        }
    }
}

impl Index<usize> for Nodes {
    type Output = Node;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<usize> for Nodes {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

impl IntoIterator for Nodes {
    type Item = Node;
    type IntoIter = std::vec::IntoIter<Node>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Nodes {
    type Item = &'a Node;
    type IntoIter = std::slice::Iter<'a, Node>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a> IntoIterator for &'a mut Nodes {
    type Item = &'a mut Node;
    type IntoIter = std::slice::IterMut<'a, Node>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}
