use super::graph::{CoreGraph, NodeId, PathName};
use crate::core::graph::{Edge, Node};
use std::collections::HashMap;

pub struct LookUpGraph<'a> {
    pub(crate) graph: &'a CoreGraph,
    pub node_index: HashMap<&'a NodeId, usize>,
    pub path_index: HashMap<&'a PathName, usize>,
}

impl LookUpGraph<'_> {
    pub fn get_adjacency_list(&self) -> HashMap<&NodeId, Vec<&Edge>> {
        let mut adjacency_list: HashMap<&NodeId, Vec<&Edge>> = HashMap::new();
        for edge in &self.graph.edges {
            let from_node = &edge.from_node;
            adjacency_list
                .entry(from_node)
                .or_insert_with(Vec::new)
                .push(edge);
        }
        adjacency_list
    }

    pub fn get_node_by_id(&self, node_id: &NodeId) -> Option<&Node> {
        self.node_index
            .get(node_id)
            .and_then(|&index| self.graph.nodes.get(index))
    }

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
