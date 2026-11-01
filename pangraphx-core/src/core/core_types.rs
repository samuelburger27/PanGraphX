use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::{PanResult, error::PanGraphXError};
use std::fmt::{Display, Write};
use std::ops::Index;

/// A unique identifier for a node/segment in the graph.
pub type NodeId = usize;

pub type PathName = Vec<u8>;
pub type NodeName = Vec<u8>;
pub type Sequence = Vec<u8>;

/// A node's name as [`CoreGraphDTO::node_label`](crate::CoreGraphDTO::node_label)
/// gives it: the original name, or the node id when it has none.
///
/// Labels compare and hash by their bytes, an id label's bytes being its
/// decimal form, so `Name(b"7")` equals `Id(7)`. `Display` writes the name as
/// lossy UTF-8. Nothing here allocates.
#[derive(Clone, Copy)]
pub enum NodeLabel<'a> {
    Name(&'a [u8]),
    Id(NodeId),
}

impl NodeLabel<'_> {
    /// Calls `f` with the label's bytes.
    fn with_bytes<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        match *self {
            Self::Name(name) => f(name),
            Self::Id(id) => f(itoa::Buffer::new().format(id).as_bytes()),
        }
    }
}

impl Display for NodeLabel<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Name(name) => {
                for chunk in name.utf8_chunks() {
                    f.write_str(chunk.valid())?;
                    if !chunk.invalid().is_empty() {
                        f.write_char(char::REPLACEMENT_CHARACTER)?;
                    }
                }
                Ok(())
            }
            Self::Id(id) => write!(f, "{id}"),
        }
    }
}

impl std::fmt::Debug for NodeLabel<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeLabel(\"{self}\")")
    }
}

impl PartialEq for NodeLabel<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.with_bytes(|a| other.with_bytes(|b| a == b))
    }
}

impl Eq for NodeLabel<'_> {}

impl std::hash::Hash for NodeLabel<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.with_bytes(|bytes| bytes.hash(state));
    }
}

impl PartialEq<str> for NodeLabel<'_> {
    fn eq(&self, other: &str) -> bool {
        self.with_bytes(|bytes| bytes == other.as_bytes())
    }
}

impl PartialEq<&str> for NodeLabel<'_> {
    fn eq(&self, other: &&str) -> bool {
        *self == **other
    }
}

/// Represents the orientation of a node traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    Forward,
    Reverse,
}

impl Orientation {
    /// The opposite orientation.
    #[must_use]
    pub const fn flip(self) -> Self {
        match self {
            Self::Forward => Self::Reverse,
            Self::Reverse => Self::Forward,
        }
    }
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
///
/// A node's id is its index in [`Nodes`]; it is not stored, so iterate with
/// `enumerate` when the id is needed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Node {
    /// DNA sequence of the node.
    pub sequence: Sequence,
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

impl Edge {
    /// Whether this edge is the link from `from_node` (in `from_orient`) to
    /// `to_node` (in `to_orient`), stored in either direction: `a+ -> b+` is
    /// the same link as `b- -> a-`. The overlap is not compared.
    #[must_use]
    pub fn is_link(
        &self,
        from_node: NodeId,
        from_orient: Orientation,
        to_node: NodeId,
        to_orient: Orientation,
    ) -> bool {
        let stored = (
            self.from_node,
            self.from_orient,
            self.to_node,
            self.to_orient,
        );
        stored == (from_node, from_orient, to_node, to_orient)
            || stored == (to_node, to_orient.flip(), from_node, from_orient.flip())
    }
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
            return Err(PanGraphXError::NodeIdTooLarge {
                node_id,
                max: Self::MAX_NODE_ID,
            });
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

/// A collection of nodes in the graph, indexed by [`NodeId`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Nodes(Vec<Node>);

impl Nodes {
    /// Creates a new, empty collection of nodes.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }
    /// Creates a new `Nodes` collection from a vector of sequences; the
    /// sequence at index `i` becomes node `i`.
    #[must_use]
    pub fn from_seq(nodes: Vec<Sequence>) -> Self {
        Self(
            nodes
                .into_iter()
                .map(|sequence| Node { sequence })
                .collect(),
        )
    }

    /// Adds a new node with the given sequence; its id is the next index.
    pub fn push(&mut self, sequence: Sequence) {
        self.0.push(Node { sequence });
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

    /// Returns the sequence of a node for editing.
    pub fn sequence_mut(&mut self, node_id: NodeId) -> Option<&mut Sequence> {
        self.0.get_mut(node_id).map(|node| &mut node.sequence)
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

    /// Removes a node by its ID and returns it if it exists. Every later
    /// node's id drops by one.
    pub(crate) fn remove(&mut self, node_id: NodeId) -> Option<Node> {
        (node_id < self.0.len()).then(|| self.0.remove(node_id))
    }

    /// Returns a parallel iterator over the nodes in the collection.
    #[must_use]
    pub fn par_iter(&self) -> impl ParallelIterator<Item = &Node> {
        self.0.par_iter()
    }
}

impl Index<usize> for Nodes {
    type Output = Node;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_labels_compare_by_bytes() {
        use std::collections::HashSet;
        assert_eq!(NodeLabel::Name(b"7"), NodeLabel::Id(7));
        assert_ne!(NodeLabel::Name(b"07"), NodeLabel::Id(7));
        assert_eq!(NodeLabel::Id(12), "12");
        assert_eq!(NodeLabel::Name(b"s1"), "s1");
        let set: HashSet<NodeLabel<'_>> = [NodeLabel::Name(b"7"), NodeLabel::Id(7)].into();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn node_labels_display_lossy_utf8() {
        assert_eq!(NodeLabel::Name(b"a\xffb").to_string(), "a\u{fffd}b");
        assert_eq!(NodeLabel::Id(3).to_string(), "3");
        assert_eq!(format!("{:?}", NodeLabel::Name(b"x")), "NodeLabel(\"x\")");
    }

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
        assert!(matches!(
            Step::new(Step::MAX_NODE_ID + 1, Orientation::Forward),
            Err(PanGraphXError::NodeIdTooLarge { .. })
        ));
        let step = Step::new(7, Orientation::Reverse).unwrap();
        assert!(step.with_node_id(Step::MAX_NODE_ID + 1).is_err());
        let moved = step.with_node_id(3).unwrap();
        assert_eq!(
            (moved.node_id(), moved.orientation()),
            (3, Orientation::Reverse)
        );
    }
}
