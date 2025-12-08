use super::graph::{CoreGraph, NodeId, PathName};
use std::collections::HashMap;

pub struct LookUpGraph<'a> {
    pub(crate) graph: &'a CoreGraph,
    pub node_index: HashMap<&'a NodeId, usize>,
    pub path_index: HashMap<&'a PathName, usize>,
}

impl LookUpGraph<'_> {
    pub fn new(graph: &'_ CoreGraph) -> LookUpGraph<'_> {
        let mut node_index = HashMap::new();
        for (i, node) in graph.nodes.iter().enumerate() {
            node_index.insert(&node.id, i);
        }

        let mut path_index = HashMap::new();
        for (i, path) in graph.paths.iter().enumerate() {
            path_index.insert(&path.name, i);
        }

        LookUpGraph {
            graph,
            node_index,
            path_index,
        }
    }
}
