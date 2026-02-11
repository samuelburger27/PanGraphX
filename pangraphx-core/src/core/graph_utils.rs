use super::graph::CoreGraph;
use super::graph_dto::{Orientation, Path};
use bio::alphabets::dna::revcomp as reverse_complement;
use std::borrow::Cow;

impl CoreGraph {
    /// Return an iterator over the node sequences for the given path.
    /// The sequences are returned in the correct orientation
    /// (if orientation is Reverse, the reverse complement is returned).
    pub fn path_node_sequences<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = Cow<'a, [u8]>> + 'a {
        path.steps.iter().map(move |step| {
            let node = &self.nodes[step.node_id];
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
            let node = &self.nodes[step.node_id];
            node.sequence.as_slice()
        })
    }
}
