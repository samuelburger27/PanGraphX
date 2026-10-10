//! Subgraph extraction for [`CoreGraph`].
//!
//! Provides [`CoreGraph::extract_path`], [`CoreGraph::extract_neighborhood`] and
//! [`CoreGraph::extract_component`], each returning a freshly renumbered
//! [`CoreGraphDTO`] containing only the selected nodes, the relevant edges and the
//! paths that lie entirely within the selection.

use super::core_types::{Edge, NodeId, Nodes, Path, Step};
use super::graph::CoreGraph;
use super::graph_dto::CoreGraphDTO;
use super::union_find::UnionFind;
use crate::error::{PanGraphXError, PanResult};
use std::collections::{HashMap, HashSet, VecDeque};

impl CoreGraph {
    /// Resolves a node from either a numeric ID or a node name (via `node_name_map`).
    ///
    /// Numeric strings are always interpreted as raw node IDs first.
    ///
    /// # Errors
    /// Returns an error if no node with the given ID or name exists.
    pub fn resolve_node_id(&self, id_or_name: &str) -> PanResult<NodeId> {
        if let Ok(id) = id_or_name.parse::<NodeId>() {
            if id < self.nodes.len() {
                return Ok(id);
            }
            return Err(PanGraphXError::Other(format!("Node {id} does not exist")));
        }

        if let Some(map) = &self.node_name_map
            && let Some((&id, _)) = map
                .iter()
                .find(|(_, name)| name.as_slice() == id_or_name.as_bytes())
        {
            return Ok(id);
        }

        Err(PanGraphXError::Other(format!(
            "Node '{id_or_name}' not found"
        )))
    }

    /// Extracts the named path into a new graph.
    ///
    /// The result contains the set of nodes traversed by the path, the edges that
    /// connect consecutive steps of the path (matched by node pair regardless of
    /// edge direction), and the path itself, all with renumbered node IDs.
    ///
    /// # Errors
    /// Returns an error if the path does not exist or has no steps.
    pub fn extract_path(&self, path_name: &[u8]) -> PanResult<CoreGraphDTO> {
        let path = self.get_path(path_name)?.clone();

        if path.steps.is_empty() {
            return Err(PanGraphXError::Other(format!(
                "Path {:?} has no steps",
                String::from_utf8_lossy(path_name)
            )));
        }

        let selected: HashSet<NodeId> = path.steps.iter().map(|step| step.node_id).collect();

        // Collect the edges that connect consecutive steps of the path.
        let mut edge_indices: HashSet<usize> = HashSet::new();
        for pair in path.steps.windows(2) {
            let (a, b) = (pair[0].node_id, pair[1].node_id);
            edge_indices.extend(
                self.adjacency_list
                    .get(&a)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|&idx| self.edges[idx].to_node == b),
            );
            edge_indices.extend(
                self.adjacency_list
                    .get(&b)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|&idx| self.edges[idx].to_node == a),
            );
        }

        let edges: Vec<Edge> = edge_indices
            .into_iter()
            .map(|idx| self.edges[idx].clone())
            .collect();

        Ok(self.assemble_subgraph(&selected, &edges, &[path]))
    }

    /// Extracts the `radius`-hop neighborhood of a node.
    ///
    /// Edges are traversed undirectionally (orientation and edge direction ignored):
    /// the neighborhood reaches both upstream and downstream neighbours within
    /// `radius` hops. The result contains the selected nodes, every induced edge
    /// between them, and the paths whose steps all lie within the selection.
    ///
    /// # Errors
    /// Returns an error if the node does not exist.
    pub fn extract_neighborhood(&self, node: NodeId, radius: usize) -> PanResult<CoreGraphDTO> {
        if node >= self.nodes.len() {
            return Err(PanGraphXError::Other(format!("Node {node} does not exist")));
        }

        let mut incoming: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for edge in &self.edges {
            incoming
                .entry(edge.to_node)
                .or_default()
                .push(edge.from_node);
        }

        let mut visited: HashSet<NodeId> = HashSet::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();
        visited.insert(node);
        queue.push_back((node, 0));

        while let Some((current, dist)) = queue.pop_front() {
            if dist >= radius {
                continue;
            }
            let outgoing = self
                .adjacency_list
                .get(&current)
                .into_iter()
                .flatten()
                .map(|&idx| self.edges[idx].to_node);
            let incoming = incoming.get(&current).into_iter().flatten().copied();
            for neighbour in outgoing.chain(incoming) {
                if visited.insert(neighbour) {
                    queue.push_back((neighbour, dist + 1));
                }
            }
        }

        Ok(self.assemble_induced(&visited))
    }

    /// Extracts the weakly connected component that contains the given node.
    /// The result contains every node reachable from `node` ignoring edge direction,
    /// all induced edges between them, and the paths whose steps all lie within the
    /// component.
    ///
    /// # Errors
    /// Returns an error if the node does not exist.
    pub fn extract_component(&self, node: NodeId) -> PanResult<CoreGraphDTO> {
        if node >= self.nodes.len() {
            return Err(PanGraphXError::Other(format!("Node {node} does not exist")));
        }

        let n = self.nodes.len();
        let mut uf = UnionFind::new(n);
        for edge in &self.edges {
            uf.union(edge.from_node, edge.to_node);
        }

        let root = uf.find(node);
        let selected: HashSet<NodeId> = (0..n).filter(|&id| uf.find(id) == root).collect();

        Ok(self.assemble_induced(&selected))
    }

    /// Builds an induced subgraph from the selected node set: all edges whose both
    /// endpoints are selected, and all paths whose steps are contained in the set.
    fn assemble_induced(&self, selected: &HashSet<NodeId>) -> CoreGraphDTO {
        let edges: Vec<Edge> = self
            .edges
            .iter()
            .filter(|edge| selected.contains(&edge.from_node) && selected.contains(&edge.to_node))
            .cloned()
            .collect();

        let paths: Vec<Path> = self
            .path_map
            .values()
            .filter(|path| {
                path.steps
                    .iter()
                    .all(|step| selected.contains(&step.node_id))
            })
            .cloned()
            .collect();

        self.assemble_subgraph(selected, &edges, &paths)
    }

    /// Renumbers the selected nodes to a contiguous `0..n` range and remaps the given
    /// edges and paths onto the new IDs.
    fn assemble_subgraph(
        &self,
        selected: &HashSet<NodeId>,
        edges: &[Edge],
        paths: &[Path],
    ) -> CoreGraphDTO {
        let mut sorted: Vec<NodeId> = selected.iter().copied().collect();
        sorted.sort_unstable();
        let old_to_new: HashMap<NodeId, NodeId> = sorted
            .iter()
            .enumerate()
            .map(|(new, &old)| (old, new))
            .collect();

        let nodes = Nodes::from_seq(
            sorted
                .iter()
                .map(|&old| self.nodes[old].sequence.clone())
                .collect(),
        );

        let remapped_edges: Vec<Edge> = edges
            .iter()
            .filter_map(|edge| {
                let from = old_to_new.get(&edge.from_node)?;
                let to = old_to_new.get(&edge.to_node)?;
                Some(Edge {
                    from_node: *from,
                    from_orient: edge.from_orient,
                    to_node: *to,
                    to_orient: edge.to_orient,
                    overlap: edge.overlap,
                })
            })
            .collect();

        let remapped_paths: Vec<Path> = paths
            .iter()
            .map(|path| Path {
                name: path.name.clone(),
                steps: path
                    .steps
                    .iter()
                    .filter_map(|step| {
                        old_to_new.get(&step.node_id).map(|&new| Step {
                            node_id: new,
                            orientation: step.orientation,
                        })
                    })
                    .collect(),
                overlaps: path.overlaps.clone(),
            })
            .collect();

        let node_name_map = self.node_name_map.as_ref().map(|map| {
            map.iter()
                .filter_map(|(&old, name)| old_to_new.get(&old).map(|&new| (new, name.clone())))
                .collect()
        });

        CoreGraphDTO {
            nodes,
            edges: remapped_edges,
            paths: remapped_paths,
            node_name_map,
        }
    }
}
