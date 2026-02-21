use super::graph_dto::CoreGraphDTO;
use crate::core::core_types::{Edge, Node, NodeId, NodeName, Nodes, Path, PathName, Sequence};
use crate::error::{PanGraphXError, PanResult};
use std::collections::HashMap;

/// A CoreGraph type providing efficient lookup and graph manipulation capabilities.
pub struct CoreGraph {
    pub nodes: Nodes,
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

    // ===== Graph Manipulation Methods =====

    /// Adds a new node to the graph with the given sequence.
    /// Returns the NodeId assigned to the new node.
    ///
    /// # Arguments
    /// * `sequence` - DNA sequence for the new node
    /// * `name` - Optional name for the node
    ///
    /// # Examples
    /// ```
    /// # use pangraphx_core::CoreGraph;
    /// # use pangraphx_core::CoreGraphDTO;
    /// let mut graph = CoreGraph::new(CoreGraphDTO::default());
    /// let node_id = graph.add_node(b"ACGT".to_vec(), None);
    /// assert_eq!(node_id, 0);
    /// ```
    pub fn add_node(&mut self, sequence: Sequence, name: Option<NodeName>) -> NodeId {
        self.nodes.push(sequence);
        let node_id = self.nodes.len() - 1;

        if let Some(node_name) = name {
            self.node_name_map
                .get_or_insert_with(HashMap::new)
                .insert(node_id, node_name);
        }
        node_id
    }

    /// Adds a new edge to the graph.
    /// Returns the index of the added edge in the edges vector.
    ///
    /// # Arguments
    /// * `edge` - The edge to add
    ///
    /// # Errors
    /// Returns an error if the from_node or to_node doesn't exist in the graph.
    ///
    /// # Examples
    /// ```
    /// # use pangraphx_core::{CoreGraph, CoreGraphDTO};
    /// # use pangraphx_core::core::core_types::{Edge, Orientation};
    /// let mut graph = CoreGraph::new(CoreGraphDTO::default());
    /// let n1 = graph.add_node(b"ACGT".to_vec(), None);
    /// let n2 = graph.add_node(b"TGCA".to_vec(), None);
    /// let edge = Edge {
    ///     from_node: n1,
    ///     from_orient: Orientation::Forward,
    ///     to_node: n2,
    ///     to_orient: Orientation::Forward,
    ///     overlap: 0,
    /// };
    /// graph.add_edge(edge).unwrap();
    /// ```
    pub fn add_edge(&mut self, edge: Edge) -> PanResult<usize> {
        // Validate that nodes exist
        if edge.from_node >= self.nodes.len() {
            return Err(PanGraphXError::Other(format!(
                "From node {} does not exist",
                edge.from_node
            )));
        }
        if edge.to_node >= self.nodes.len() {
            return Err(PanGraphXError::Other(format!(
                "To node {} does not exist",
                edge.to_node
            )));
        }

        let edge_idx = self.edges.len();
        self.edges.push(edge.clone());

        // Update adjacency list
        self.adjacency_list
            .entry(edge.from_node)
            .or_insert_with(Vec::new)
            .push(edge_idx);

        Ok(edge_idx)
    }

    /// Adds a new path to the graph.
    ///
    /// # Arguments
    /// * `path` - The path to add
    ///
    /// # Errors
    /// Returns an error if:
    /// - A path with the same name already exists
    /// - Any node in the path doesn't exist
    ///
    /// # Examples
    /// ```
    /// # use pangraphx_core::{CoreGraph, CoreGraphDTO};
    /// # use pangraphx_core::core::core_types::{Path, Step, Orientation};
    /// let mut graph = CoreGraph::new(CoreGraphDTO::default());
    /// let n1 = graph.add_node(b"ACGT".to_vec(), None);
    /// let path = Path {
    ///     name: b"path1".to_vec(),
    ///     steps: vec![Step { node_id: n1, orientation: Orientation::Forward }],
    ///     overlaps: vec![],
    /// };
    /// graph.add_path(path).unwrap();
    /// ```
    pub fn add_path(&mut self, path: Path) -> PanResult<()> {
        // Check if path already exists
        if self.path_map.contains_key(&path.name) {
            return Err(PanGraphXError::Other(format!(
                "Path {:?} already exists",
                String::from_utf8_lossy(&path.name)
            )));
        }

        // Validate that all nodes in the path exist
        for step in &path.steps {
            if step.node_id >= self.nodes.len() {
                return Err(PanGraphXError::Other(format!(
                    "Node {} in path does not exist",
                    step.node_id
                )));
            }
        }

        self.path_map.insert(path.name.clone(), path);
        Ok(())
    }

    /// Removes a node from the graph.
    /// Also removes all edges connected to this node and updates paths.
    /// This operation is potentially expensive as it requires updating node IDs and paths.
    ///
    /// # Arguments
    /// * `node_id` - The ID of the node to remove
    ///
    /// # Errors
    /// Returns an error if the node doesn't exist.
    ///
    /// # Note
    /// This operation renumbers node IDs. All nodes after the removed node
    /// will have their IDs decremented by 1. Edges and paths are updated accordingly.
    ///
    /// # Examples
    /// ```
    /// # use pangraphx_core::{CoreGraph, CoreGraphDTO};
    /// let mut graph = CoreGraph::new(CoreGraphDTO::default());
    /// let n1 = graph.add_node(b"ACGT".to_vec(), None);
    /// graph.remove_node(n1).unwrap();
    /// ```
    pub fn remove_node(&mut self, node_id: NodeId) -> PanResult<()> {
        if node_id >= self.nodes.len() {
            return Err(PanGraphXError::Other(format!(
                "Node {} does not exist",
                node_id
            )));
        }

        // Remove the node
        self.nodes.remove(node_id);

        // Update node IDs for all nodes after the removed one
        for i in node_id..self.nodes.len() {
            self.nodes[i].id = i;
        }

        // Remove edges connected to this node and update edge node IDs
        self.edges
            .retain(|edge| edge.from_node != node_id && edge.to_node != node_id);

        for edge in &mut self.edges {
            if edge.from_node > node_id {
                edge.from_node -= 1;
            }
            if edge.to_node > node_id {
                edge.to_node -= 1;
            }
        }

        // Rebuild adjacency list
        self.adjacency_list = Self::build_adjacency_list(&CoreGraphDTO {
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
            paths: Vec::new(),
            node_name_map: None,
        });

        // Update paths: remove steps with deleted node and update node IDs
        let mut paths_to_remove = Vec::new();
        for (name, path) in &mut self.path_map {
            path.steps.retain(|step| step.node_id != node_id);

            // If path is now empty, mark for removal
            if path.steps.is_empty() {
                paths_to_remove.push(name.clone());
            } else {
                // Update node IDs in remaining steps
                for step in &mut path.steps {
                    if step.node_id > node_id {
                        step.node_id -= 1;
                    }
                }
            }
        }

        // Remove empty paths
        for name in paths_to_remove {
            self.path_map.remove(&name);
        }

        // Update node_name_map
        if let Some(ref mut name_map) = self.node_name_map {
            name_map.remove(&node_id);

            // Update IDs in name_map
            let mut updated_map = HashMap::new();
            for (id, name) in name_map.iter() {
                if *id > node_id {
                    updated_map.insert(id - 1, name.clone());
                } else if *id != node_id {
                    updated_map.insert(*id, name.clone());
                }
            }
            *name_map = updated_map;
        }

        Ok(())
    }

    /// Removes an edge from the graph at the specified index.
    ///
    /// # Arguments
    /// * `edge_idx` - The index of the edge to remove in the edges vector
    ///
    /// # Errors
    /// Returns an error if the edge index is out of bounds.
    ///
    /// # Examples
    /// ```
    /// # use pangraphx_core::{CoreGraph, CoreGraphDTO};
    /// # use pangraphx_core::core::core_types::{Edge, Orientation};
    /// let mut graph = CoreGraph::new(CoreGraphDTO::default());
    /// let n1 = graph.add_node(b"ACGT".to_vec(), None);
    /// let n2 = graph.add_node(b"TGCA".to_vec(), None);
    /// let edge = Edge {
    ///     from_node: n1,
    ///     from_orient: Orientation::Forward,
    ///     to_node: n2,
    ///     to_orient: Orientation::Forward,
    ///     overlap: 0,
    /// };
    /// let idx = graph.add_edge(edge).unwrap();
    /// graph.remove_edge(idx).unwrap();
    /// ```
    pub fn remove_edge(&mut self, edge_idx: usize) -> PanResult<()> {
        if edge_idx >= self.edges.len() {
            return Err(PanGraphXError::Other(format!(
                "Edge index {} is out of bounds",
                edge_idx
            )));
        }

        self.edges.remove(edge_idx);

        // Rebuild adjacency list since edge indices have changed
        self.adjacency_list = Self::build_adjacency_list(&CoreGraphDTO {
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
            paths: Vec::new(),
            node_name_map: None,
        });

        Ok(())
    }

    /// Removes a path from the graph.
    ///
    /// # Arguments
    /// * `path_name` - The name of the path to remove
    ///
    /// # Errors
    /// Returns an error if the path doesn't exist.
    ///
    /// # Examples
    /// ```
    /// # use pangraphx_core::{CoreGraph, CoreGraphDTO};
    /// # use pangraphx_core::core::core_types::{Path, Step, Orientation};
    /// let mut graph = CoreGraph::new(CoreGraphDTO::default());
    /// let n1 = graph.add_node(b"ACGT".to_vec(), None);
    /// let path = Path {
    ///     name: b"path1".to_vec(),
    ///     steps: vec![Step { node_id: n1, orientation: Orientation::Forward }],
    ///     overlaps: vec![],
    /// };
    /// graph.add_path(path).unwrap();
    /// graph.remove_path(b"path1").unwrap();
    /// ```
    pub fn remove_path(&mut self, path_name: &[u8]) -> PanResult<()> {
        if self.path_map.remove(path_name).is_none() {
            return Err(PanGraphXError::Other(format!(
                "Path {:?} does not exist",
                String::from_utf8_lossy(path_name)
            )));
        }
        Ok(())
    }

    /// Returns the number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns the number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Returns the number of paths in the graph.
    pub fn path_count(&self) -> usize {
        self.path_map.len()
    }

    /// Gets a reference to a node by its ID.
    ///
    /// # Errors
    /// Returns an error if the node doesn't exist.
    pub fn get_node(&self, node_id: NodeId) -> PanResult<&Node> {
        self.nodes
            .get(node_id)
            .ok_or_else(|| PanGraphXError::Other(format!("Node {} does not exist", node_id)))
    }

    /// Gets a mutable reference to a node by its ID.
    ///
    /// # Errors
    /// Returns an error if the node doesn't exist.
    pub fn get_node_mut(&mut self, node_id: NodeId) -> PanResult<&mut Node> {
        let len = self.nodes.len();
        self.nodes.get_mut(node_id).ok_or_else(|| {
            PanGraphXError::Other(format!(
                "Node {} does not exist (graph has {} nodes)",
                node_id, len
            ))
        })
    }

    /// Gets a reference to an edge by its index.
    ///
    /// # Errors
    /// Returns an error if the edge index is out of bounds.
    pub fn get_edge(&self, edge_idx: usize) -> PanResult<&Edge> {
        self.edges.get(edge_idx).ok_or_else(|| {
            PanGraphXError::Other(format!("Edge index {} is out of bounds", edge_idx))
        })
    }

    /// Gets a reference to a path by its name.
    ///
    /// # Errors
    /// Returns an error if the path doesn't exist.
    pub fn get_path(&self, path_name: &[u8]) -> PanResult<&Path> {
        self.path_map.get(path_name).ok_or_else(|| {
            PanGraphXError::Other(format!(
                "Path {:?} does not exist",
                String::from_utf8_lossy(path_name)
            ))
        })
    }

    /// Gets a mutable reference to a path by its name.
    ///
    /// # Errors
    /// Returns an error if the path doesn't exist.
    pub fn get_path_mut(&mut self, path_name: &[u8]) -> PanResult<&mut Path> {
        self.path_map.get_mut(path_name).ok_or_else(|| {
            PanGraphXError::Other(format!(
                "Path {:?} does not exist",
                String::from_utf8_lossy(path_name)
            ))
        })
    }

    /// Returns all outgoing edges from a node.
    ///
    /// # Errors
    /// Returns an error if the node doesn't exist.
    pub fn get_outgoing_edges(&self, node_id: NodeId) -> PanResult<Vec<&Edge>> {
        if node_id >= self.nodes.len() {
            return Err(PanGraphXError::Other(format!(
                "Node {} does not exist",
                node_id
            )));
        }

        Ok(self
            .adjacency_list
            .get(&node_id)
            .map(|indices| indices.iter().map(|&idx| &self.edges[idx]).collect())
            .unwrap_or_default())
    }

    /// Checks if an edge exists between two nodes.
    pub fn has_edge(&self, from_node: NodeId, to_node: NodeId) -> bool {
        if let Some(edge_indices) = self.adjacency_list.get(&from_node) {
            edge_indices.iter().any(|&idx| {
                let edge = &self.edges[idx];
                edge.to_node == to_node
            })
        } else {
            false
        }
    }
}

impl From<CoreGraph> for CoreGraphDTO {
    fn from(graph: CoreGraph) -> Self {
        CoreGraphDTO {
            nodes: graph.nodes,
            edges: graph.edges,
            paths: graph.path_map.into_values().collect(),
            node_name_map: graph.node_name_map,
        }
    }
}
