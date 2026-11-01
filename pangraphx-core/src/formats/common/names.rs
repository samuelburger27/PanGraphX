//! Node name tables used by the text writers (GFA, FASTG).

use crate::core::graph_dto::CoreGraphDTO;
use rayon::prelude::*;
use std::borrow::Cow;

/// Builds the id -> name table once, so formatting never hashes or allocates per step.
/// Nodes without an entry in `node_name_map` are named by their numeric id.
pub fn node_names(graph: &CoreGraphDTO) -> Vec<Cow<'_, [u8]>> {
    (0..graph.nodes().len())
        .into_par_iter()
        .map(|id| {
            graph
                .node_name_map()
                .as_ref()
                .and_then(|map| map.get(&id))
                .map_or_else(
                    || Cow::Owned(itoa::Buffer::new().format(id).as_bytes().to_vec()),
                    |name| Cow::Borrowed(name.as_slice()),
                )
        })
        .collect()
}

/// Names every node by its 1-based id, ignoring `node_name_map`.
pub fn one_based_names(graph: &CoreGraphDTO) -> Vec<Cow<'_, [u8]>> {
    (1..=graph.nodes().len())
        .into_par_iter()
        .map(|id| Cow::Owned(itoa::Buffer::new().format(id).as_bytes().to_vec()))
        .collect()
}
