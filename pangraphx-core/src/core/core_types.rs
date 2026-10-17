use rayon::iter::{IntoParallelRefIterator, IntoParallelRefMutIterator, ParallelIterator};

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
            Self::Forward => write!(f, "+"),
            Self::Reverse => write!(f, "-"),
        }
    }
}

impl From<bool> for Orientation {
    fn from(is_reverse: bool) -> Self {
        if is_reverse {
            Self::Reverse
        } else {
            Self::Forward
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

/// A single step in a path: a node traversed in a given orientation.
///
/// Packed into 4 bytes as `node_id << 1 | is_reverse` (a handle, as in
/// odgi/libhandlegraph). Paths dominate the memory of pangenome graphs, so this
/// is 4× smaller than storing a `NodeId` and an `Orientation` side by side.
/// Node ids are therefore limited to [`Step::MAX_NODE_ID`].
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Step(u32);

impl Step {
    /// The largest node id a step can refer to.
    pub const MAX_NODE_ID: NodeId = (u32::MAX >> 1) as NodeId;

    /// Creates a step visiting `node_id` in the given orientation.
    ///
    /// # Errors
    /// Returns an error if `node_id` exceeds [`Step::MAX_NODE_ID`].
    pub fn new(node_id: NodeId, orientation: Orientation) -> PanResult<Self> {
        if node_id > Self::MAX_NODE_ID {
            return Err(PanGraphXError::Other(format!(
                "Node ID {node_id} exceeds the maximum supported node ID {}",
                Self::MAX_NODE_ID
            )));
        }
        #[allow(clippy::cast_possible_truncation)]
        let id = node_id as u32;
        Ok(Self(
            id << 1 | u32::from(orientation == Orientation::Reverse),
        ))
    }

    /// The node this step visits.
    #[must_use]
    pub const fn node_id(self) -> NodeId {
        (self.0 >> 1) as NodeId
    }

    /// The orientation the node is traversed in.
    #[must_use]
    pub const fn orientation(self) -> Orientation {
        if self.is_reverse() {
            Orientation::Reverse
        } else {
            Orientation::Forward
        }
    }

    /// Whether the node is traversed in reverse orientation.
    #[must_use]
    pub const fn is_reverse(self) -> bool {
        self.0 & 1 == 1
    }

    /// Returns this step moved to another node, keeping its orientation.
    ///
    /// # Errors
    /// Returns an error if `node_id` exceeds [`Step::MAX_NODE_ID`].
    pub fn with_node_id(self, node_id: NodeId) -> PanResult<Self> {
        Self::new(node_id, self.orientation())
    }
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Step")
            .field("node_id", &self.node_id())
            .field("orientation", &self.orientation())
            .finish()
    }
}

/// A uniquely named, ordered traversal through nodes in the graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Path {
    pub name: PathName,
    pub steps: Vec<Step>,
    pub overlaps: Vec<u32>,
}

/// A collection of nodes in the graph.
///
/// Wraps a vector of `Node` and enforces the invariant that node IDs correspond to their index in the vector.
/// Invariant: Node IDs are unique and correspond to their index in the vector.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Nodes(Vec<Node>);

impl Nodes {
    /// Creates a new, empty collection of nodes.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }
    /// Creates a new `Nodes` collection from a vector of sequences
    #[must_use]
    pub fn from_seq(nodes: Vec<Sequence>) -> Self {
        let nodes = nodes
            .into_iter()
            .enumerate()
            .map(|(id, sequence)| Node { sequence, id })
            .collect();
        Self(nodes)
    }

    /// Creates a `Nodes` collection from a vector of `Node`, ensuring that node IDs are unique and correspond to their index.
    ///
    /// # Errors
    /// Returns an error if any node ID does not match its index in the vector.
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
        self.0.push(Node { sequence, id });
    }

    /// Returns the number of nodes in the collection.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Checks if the collection of nodes is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get_mut(&mut self, node_id: NodeId) -> Option<&mut Node> {
        self.0.get_mut(node_id)
    }

    #[must_use]
    pub fn get(&self, node_id: NodeId) -> Option<&Node> {
        self.0.get(node_id)
    }

    /// Returns the nodes as a slice, indexed by `NodeId`.
    #[must_use]
    pub fn as_slice(&self) -> &[Node] {
        &self.0
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

    /// Returns a parallel iterator over the nodes in the collection.
    #[must_use]
    pub fn par_iter(&self) -> impl ParallelIterator<Item = &Node> {
        self.0.par_iter()
    }
    /// Returns a parallel mutable iterator over the nodes in the collection.
    pub fn par_iter_mut(&mut self) -> impl ParallelIterator<Item = &mut Node> {
        self.0.par_iter_mut()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_is_four_bytes() {
        assert_eq!(std::mem::size_of::<Step>(), 4);
    }

    #[test]
    fn step_roundtrips_id_and_orientation() {
        for id in [0, 1, 2, 12_345, Step::MAX_NODE_ID] {
            for orientation in [Orientation::Forward, Orientation::Reverse] {
                let step = Step::new(id, orientation).unwrap();
                assert_eq!(step.node_id(), id);
                assert_eq!(step.orientation(), orientation);
                assert_eq!(step.is_reverse(), orientation == Orientation::Reverse);
            }
        }
    }

    #[test]
    fn step_rejects_too_large_node_id() {
        assert!(Step::new(Step::MAX_NODE_ID + 1, Orientation::Forward).is_err());
        let step = Step::new(7, Orientation::Reverse).unwrap();
        assert!(step.with_node_id(Step::MAX_NODE_ID + 1).is_err());
        let moved = step.with_node_id(3).unwrap();
        assert_eq!(
            (moved.node_id(), moved.orientation()),
            (3, Orientation::Reverse)
        );
    }
}
