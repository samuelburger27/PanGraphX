use super::graph::{Orientation, Path};
use super::lookup_graph::LookUpGraph;
use std::borrow::Cow;
/// Returns the reverse complement of a DNA sequence
pub fn reverse_complement(sequence: &[u8]) -> Vec<u8> {
    sequence
        .iter()
        .rev()
        .map(|b| match b {
            b'A' => b'T',
            b'T' => b'A',
            b'C' => b'G',
            b'G' => b'C',
            _ => *b,
        })
        .collect()
}

impl LookUpGraph<'_> {
    /// Return an iterator over the node sequences for the given path.
    /// The sequences are returned in the correct orientation
    /// (if orientation is Reverse, the reverse complement is returned).
    pub fn path_node_sequences<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = Cow<'a, [u8]>> + 'a {
        path.steps.iter().map(move |step| {
            // Node id should always exist in the lookup graph
            let index = *self
                .node_index
                .get(&step.node_id)
                .expect("unknown node id in path");
            let node = &self.graph.nodes[index];
            match step.orientation {
                Orientation::Forward => Cow::Borrowed(node.sequence.as_slice()),
                Orientation::Reverse => Cow::Owned(reverse_complement(&node.sequence)),
            }
        })
    }
    /// Return the forward sequence of the node for the given step in a path.
    pub(crate) fn path_node_forward_sequence<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = &'a [u8]> + 'a {
        path.steps.iter().map(move |step| {
            // Node id should always exist in the lookup graph
            let index = *self
                .node_index
                .get(&step.node_id)
                .expect("unknown node id in path");
            self.graph.nodes[index].sequence.as_slice()
        })
    }
}
