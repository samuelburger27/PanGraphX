use super::graph_dto::{CoreGraphDTO, GraphParts};
use crate::core::core_types::{
    Edge, Node, NodeId, NodeName, Nodes, Orientation, Path, PathName, Sequence,
};
use crate::error::{PanGraphXError, PanResult};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::OnceLock;

/// A `CoreGraph` type providing efficient lookup and graph manipulation capabilities.
///
/// Like [`CoreGraphDTO`] it is always valid: edges and path steps refer to
/// existing nodes and path names are unique. Its lookup indices are private
/// and kept in sync by every edit.
#[derive(Debug, Clone)]
pub struct CoreGraph {
    nodes: Nodes,
    edges: Vec<Edge>,
    /// Paths in insertion order, so conversion back to a DTO is deterministic.
    paths: Vec<Path>,
    /// Path name to its index in `paths`.
    path_index: HashMap<PathName, usize>,
    node_name_map: Option<HashMap<NodeId, NodeName>>,
    /// For each node, the indices of the edges with it as an endpoint. Built
    /// on the first lookup (the de Bruijn builders never need it), extended by
    /// `add_node`/`add_edge` and dropped by removals.
    incident_edges: OnceLock<Vec<Vec<usize>>>,
}

impl CoreGraph {
    #[must_use]
    pub fn new(graph: CoreGraphDTO) -> Self {
        let GraphParts {
            nodes,
            edges,
            paths,
            node_name_map,
        } = GraphParts::from(graph);
        Self {
            incident_edges: OnceLock::new(),
            path_index: index_paths(&paths),
            nodes,
            edges,
            paths,
            node_name_map,
        }
    }

    // ===== Accessors =====

    /// The nodes, indexed by `NodeId`.
    #[must_use]
    pub const fn nodes(&self) -> &Nodes {
        &self.nodes
    }

    /// The edges; an edge's index is its position here.
    #[must_use]
    pub const fn edges(&self) -> &[Edge] {
        self.edges.as_slice()
    }

    /// The paths, in insertion order.
    #[must_use]
    pub const fn paths(&self) -> &[Path] {
        self.paths.as_slice()
    }

    /// Mapping from `NodeId` to original node names (if available).
    #[must_use]
    pub const fn node_name_map(&self) -> Option<&HashMap<NodeId, NodeName>> {
        self.node_name_map.as_ref()
    }

    /// Indices (into [`CoreGraph::edges`]) of the edges with `node_id` as an
    /// endpoint, in edge order. Empty for a missing node.
    #[must_use]
    pub fn incident_edge_indices(&self, node_id: NodeId) -> &[usize] {
        self.incident_edges()
            .get(node_id)
            .map_or(&[], Vec::as_slice)
    }

    fn incident_edges(&self) -> &[Vec<usize>] {
        self.incident_edges
            .get_or_init(|| build_incident_edges(self.nodes.len(), &self.edges))
    }

    // ===== Graph Manipulation Methods =====

    /// Adds a new node to the graph with the given sequence.
    /// Returns the `NodeId` assigned to the new node.
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
        if let Some(incident) = self.incident_edges.get_mut() {
            incident.push(Vec::new());
        }

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
    /// Returns an error if the `from_node` or `to_node` doesn't exist in the graph.
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
        self.check_node(edge.from_node)?;
        self.check_node(edge.to_node)?;

        let edge_idx = self.edges.len();
        if let Some(incident) = self.incident_edges.get_mut() {
            add_incident(incident, &edge, edge_idx);
        }
        self.edges.push(edge);
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
    ///     steps: vec![Step::new(n1, Orientation::Forward).unwrap()],
    ///     overlaps: vec![],
    /// };
    /// graph.add_path(path).unwrap();
    /// ```
    pub fn add_path(&mut self, path: Path) -> PanResult<()> {
        if self.path_index.contains_key(&path.name) {
            return Err(PanGraphXError::InvalidGraph(format!(
                "path {} already exists",
                String::from_utf8_lossy(&path.name)
            )));
        }
        self.check_steps(&path)?;
        self.path_index.insert(path.name.clone(), self.paths.len());
        self.paths.push(path);
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
        self.check_node(node_id)?;
        self.nodes.remove(node_id);

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
        self.incident_edges = OnceLock::new();

        // Drop the node's steps from every path, and paths left empty.
        for path in &mut self.paths {
            remove_steps_of(path, node_id)?;
        }
        self.paths.retain(|path| !path.steps.is_empty());
        self.path_index = index_paths(&self.paths);

        if let Some(names) = self.node_name_map.take() {
            self.node_name_map = Some(
                names
                    .into_iter()
                    .filter_map(|(id, name)| match id.cmp(&node_id) {
                        Ordering::Less => Some((id, name)),
                        Ordering::Equal => None,
                        Ordering::Greater => Some((id - 1, name)),
                    })
                    .collect(),
            );
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
            return Err(PanGraphXError::InvalidGraph(format!(
                "edge index {edge_idx} is out of bounds"
            )));
        }

        self.edges.remove(edge_idx);

        // Later edge indices shifted; rebuild the index on the next lookup.
        self.incident_edges = OnceLock::new();

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
    ///     steps: vec![Step::new(n1, Orientation::Forward).unwrap()],
    ///     overlaps: vec![],
    /// };
    /// graph.add_path(path).unwrap();
    /// graph.remove_path(b"path1").unwrap();
    /// ```
    pub fn remove_path(&mut self, path_name: &[u8]) -> PanResult<()> {
        let index = self
            .path_index
            .remove(path_name)
            .ok_or_else(|| missing_path(path_name))?;
        self.paths.remove(index);
        for later in self.path_index.values_mut() {
            if *later > index {
                *later -= 1;
            }
        }
        Ok(())
    }

    /// Returns the number of nodes in the graph.
    #[must_use]
    pub const fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns the number of edges in the graph.
    #[must_use]
    pub const fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Returns the number of paths in the graph.
    #[must_use]
    pub const fn path_count(&self) -> usize {
        self.paths.len()
    }

    /// Gets a reference to a node by its ID.
    ///
    /// # Errors
    /// Returns an error if the node doesn't exist.
    pub fn get_node(&self, node_id: NodeId) -> PanResult<&Node> {
        self.nodes.get(node_id).ok_or_else(|| missing_node(node_id))
    }

    /// Replaces the sequence of a node.
    ///
    /// # Errors
    /// Returns an error if the node doesn't exist.
    pub fn set_sequence(&mut self, node_id: NodeId, sequence: Sequence) -> PanResult<()> {
        *self
            .nodes
            .sequence_mut(node_id)
            .ok_or_else(|| missing_node(node_id))? = sequence;
        Ok(())
    }

    /// Gets a reference to an edge by its index.
    ///
    /// # Errors
    /// Returns an error if the edge index is out of bounds.
    pub fn get_edge(&self, edge_idx: usize) -> PanResult<&Edge> {
        self.edges.get(edge_idx).ok_or_else(|| {
            PanGraphXError::InvalidGraph(format!("edge index {edge_idx} is out of bounds"))
        })
    }

    /// Gets a reference to a path by its name.
    ///
    /// # Errors
    /// Returns an error if the path doesn't exist.
    pub fn get_path(&self, path_name: &[u8]) -> PanResult<&Path> {
        self.path_index
            .get(path_name)
            .map(|&index| &self.paths[index])
            .ok_or_else(|| missing_path(path_name))
    }

    /// Replaces the path with the same name as `path`, returning the old one.
    ///
    /// # Errors
    /// Returns an error if no path has that name or a step refers to a
    /// missing node.
    pub fn replace_path(&mut self, path: Path) -> PanResult<Path> {
        let index = *self
            .path_index
            .get(&path.name)
            .ok_or_else(|| missing_path(&path.name))?;
        self.check_steps(&path)?;
        Ok(std::mem::replace(&mut self.paths[index], path))
    }

    /// Returns the edges stored with `node_id` as their `from_node`, in edge
    /// order.
    ///
    /// # Errors
    /// Returns an error if the node doesn't exist.
    pub fn get_outgoing_edges(&self, node_id: NodeId) -> PanResult<Vec<&Edge>> {
        self.check_node(node_id)?;
        Ok(self
            .incident_edge_indices(node_id)
            .iter()
            .map(|&idx| &self.edges[idx])
            .filter(|edge| edge.from_node == node_id)
            .collect())
    }

    /// Checks if the graph links `from_node` (in `from_orient`) to `to_node`
    /// (in `to_orient`), whichever of its two equivalent forms is stored
    /// (`a+ -> b+` or `b- -> a-`; see [`Edge::is_link`]).
    #[must_use]
    pub fn has_edge(
        &self,
        from_node: NodeId,
        from_orient: Orientation,
        to_node: NodeId,
        to_orient: Orientation,
    ) -> bool {
        self.incident_edge_indices(from_node)
            .iter()
            .any(|&idx| self.edges[idx].is_link(from_node, from_orient, to_node, to_orient))
    }

    /// Releases what the path walkers never read (the edges, their index and
    /// the node names), keeping the nodes and paths.
    pub(crate) fn keep_only_paths(&mut self) {
        self.edges = Vec::new();
        self.incident_edges = OnceLock::new();
        self.node_name_map = None;
    }

    fn check_node(&self, node_id: NodeId) -> PanResult<()> {
        if node_id < self.nodes.len() {
            Ok(())
        } else {
            Err(missing_node(node_id))
        }
    }

    fn check_steps(&self, path: &Path) -> PanResult<()> {
        path.steps
            .iter()
            .try_for_each(|step| self.check_node(step.node_id()))
    }
}

fn build_incident_edges(node_count: usize, edges: &[Edge]) -> Vec<Vec<usize>> {
    let mut incident = vec![Vec::new(); node_count];
    for (i, edge) in edges.iter().enumerate() {
        add_incident(&mut incident, edge, i);
    }
    incident
}

/// Records edge `index` at both endpoints (once for a self-loop). Both
/// endpoints exist, since edges are validated before they are added.
fn add_incident(incident: &mut [Vec<usize>], edge: &Edge, index: usize) {
    incident[edge.from_node].push(index);
    if edge.to_node != edge.from_node {
        incident[edge.to_node].push(index);
    }
}

fn index_paths(paths: &[Path]) -> HashMap<PathName, usize> {
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| (path.name.clone(), index))
        .collect()
}

/// Removes the steps visiting `node_id` from `path` and renumbers the steps
/// of later nodes. An overlap is kept while its two steps stay adjacent; the
/// junction of two steps that were not adjacent gets overlap 0.
fn remove_steps_of(path: &mut Path, node_id: NodeId) -> PanResult<()> {
    if !path.steps.iter().any(|step| step.node_id() >= node_id) {
        return Ok(());
    }
    let with_overlaps = !path.overlaps.is_empty();
    let mut steps = Vec::with_capacity(path.steps.len());
    let mut overlaps = Vec::new();
    let mut previous: Option<usize> = None;
    for (i, &step) in path.steps.iter().enumerate() {
        let id = step.node_id();
        if id == node_id {
            continue;
        }
        if with_overlaps && let Some(p) = previous {
            let adjacent = p + 1 == i;
            overlaps.push(if adjacent {
                path.overlaps.get(p).copied().unwrap_or(0)
            } else {
                0
            });
        }
        steps.push(if id > node_id {
            step.with_node_id(id - 1)?
        } else {
            step
        });
        previous = Some(i);
    }
    path.steps = steps;
    path.overlaps = overlaps;
    Ok(())
}

fn missing_node(node_id: NodeId) -> PanGraphXError {
    PanGraphXError::InvalidGraph(format!("node {node_id} does not exist"))
}

fn missing_path(name: &[u8]) -> PanGraphXError {
    PanGraphXError::InvalidGraph(format!(
        "path {} does not exist",
        String::from_utf8_lossy(name)
    ))
}

impl From<CoreGraph> for CoreGraphDTO {
    fn from(graph: CoreGraph) -> Self {
        // Every edit keeps the graph valid, so it needs no second check.
        Self::from_valid_parts(GraphParts {
            nodes: graph.nodes,
            edges: graph.edges,
            paths: graph.paths,
            node_name_map: graph.node_name_map,
        })
    }
}
