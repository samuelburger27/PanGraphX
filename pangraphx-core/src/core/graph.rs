use super::graph_dto::{CoreGraphDTO, NodeId, PathName};
use crate::core::graph_dto::{Edge, Node, NodeName, Path};
use std::{collections::HashMap};

/// A CoreGraph type providing efficient lookup and graph manipulation capabilities.
pub struct CoreGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub path_map: HashMap<PathName, Path>,
    pub node_name_map: Option<HashMap<NodeId, NodeName>>,
    // NodeId to list of indexes of edges originating from that node
    pub adjacency_list: HashMap<NodeId, Vec<usize>>,
}

impl CoreGraph {
    pub fn new(graph: CoreGraphDTO) -> CoreGraph {
        let adjacency_list = Self::build_adjacency_list(&graph);

        let path_map = graph
            .paths
            .into_iter()
            .map(|path| (path.name.clone(), path))
            .collect();

        CoreGraph {
            nodes: graph.nodes,
            edges: graph.edges,
            path_map: path_map,
            node_name_map: graph.node_name_map,
            adjacency_list: adjacency_list,
        }
    }

    fn build_adjacency_list(graph: &CoreGraphDTO) -> HashMap<NodeId, Vec<usize>> {
        let mut adjacency_list: HashMap<NodeId, Vec<usize>> = HashMap::new();
        for (i, edge) in graph.edges.iter().enumerate() {
            let from_node = &edge.from_node;
            adjacency_list
                .entry(*from_node)
                .or_insert_with(Vec::new)
                .push(i);
        }
        adjacency_list
    }
}
