//! Graph statistics computation for [`CoreGraph`].
//!
//! Provides [`CoreGraph::compute_stats`], which returns a [`GraphStats`] summary
//! describing the sequence-length distribution (including N50), connected-component
//! structure, in/out degree distribution, and path-length distribution of a graph.

use super::graph::CoreGraph;
use std::collections::BTreeMap;

/// A summary of structural statistics for a [`CoreGraph`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GraphStats {
    /// Total number of nodes.
    pub node_count: usize,
    /// Total number of edges.
    pub edge_count: usize,
    /// Total number of paths.
    pub path_count: usize,
    /// Sequence-length metrics across all node sequences.
    pub node_len: LenStats,
    /// Number of weakly connected components (edges treated as undirected).
    pub component_count: usize,
    /// Node count of the largest weakly connected component.
    pub largest_component: usize,
    /// Number of nodes with total degree zero.
    pub isolated_nodes: usize,
    /// In-degree summary across all nodes.
    pub in_degree: DegreeStats,
    /// Out-degree summary across all nodes.
    pub out_degree: DegreeStats,
    /// Histogram of total degree (in + out): `(degree, number of nodes)`, sorted by degree.
    pub total_degree_hist: Vec<(usize, usize)>,
    /// Path-length distribution (in base pairs and steps).
    pub path_len: PathStats,
}

/// Distribution of node sequence lengths.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LenStats {
    /// Number of sequences considered.
    pub count: usize,
    /// Total sequence length in base pairs.
    pub total: usize,
    /// Shortest node sequence length.
    pub min: usize,
    /// Longest node sequence length.
    pub max: usize,
    /// Mean node sequence length.
    pub mean: f64,
    /// N50 of the node sequences.
    pub n50: usize,
}

/// Min/max/mean summary of a degree distribution.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DegreeStats {
    /// Smallest degree.
    pub min: usize,
    /// Largest degree.
    pub max: usize,
    /// Mean degree.
    pub mean: f64,
}

/// Distribution of path lengths, in base pairs and in steps (node count).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PathStats {
    /// Number of paths.
    pub count: usize,
    /// Total base pairs across all paths (overlap-adjusted).
    pub bp_total: usize,
    /// Shortest path length in base pairs.
    pub bp_min: usize,
    /// Longest path length in base pairs.
    pub bp_max: usize,
    /// Mean path length in base pairs.
    pub bp_mean: f64,
    /// Fewest steps in a path.
    pub steps_min: usize,
    /// Most steps in a path.
    pub steps_max: usize,
    /// Mean number of steps per path.
    pub steps_mean: f64,
}

impl CoreGraph {
    /// Computes structural statistics for the graph.
    ///
    /// Connected components are weakly connected (edges are treated as undirected).
    /// Path lengths are overlap-adjusted: the summed length of the node sequences along
    /// the path minus the path's recorded overlaps.
    #[must_use]
    pub fn compute_stats(&self) -> GraphStats {
        let node_len = node_length_stats(self);
        let (component_count, largest_component) = component_stats(self);
        let degree = degree_stats(self);
        let path_len = path_stats(self);

        GraphStats {
            node_count: self.node_count(),
            edge_count: self.edge_count(),
            path_count: self.path_count(),
            node_len,
            component_count,
            largest_component,
            isolated_nodes: degree.isolated,
            in_degree: degree.in_stats,
            out_degree: degree.out_stats,
            total_degree_hist: degree.hist,
            path_len,
        }
    }
}

/// Mean of `total / count`, returning `0.0` for an empty population.
#[allow(clippy::cast_precision_loss)]
fn mean(total: usize, count: usize) -> f64 {
    if count == 0 {
        0.0
    } else {
        total as f64 / count as f64
    }
}

fn node_length_stats(graph: &CoreGraph) -> LenStats {
    let mut lengths: Vec<usize> = graph.nodes.iter().map(|node| node.sequence.len()).collect();
    let count = lengths.len();
    if count == 0 {
        return LenStats::default();
    }

    let total: usize = lengths.iter().sum();
    // Sort descending so the largest sequences come first (for N50 and min/max).
    lengths.sort_unstable_by(|a, b| b.cmp(a));
    let max = lengths[0];
    let min = lengths[count - 1];

    // N50: the length at which the cumulative sum first reaches half of the total.
    let mut acc = 0usize;
    let mut n50 = 0usize;
    for &len in &lengths {
        acc += len;
        if acc * 2 >= total {
            n50 = len;
            break;
        }
    }

    LenStats {
        count,
        total,
        min,
        max,
        mean: mean(total, count),
        n50,
    }
}

/// Returns `(component_count, largest_component_size)` for weakly connected components.
fn component_stats(graph: &CoreGraph) -> (usize, usize) {
    let n = graph.nodes.len();
    if n == 0 {
        return (0, 0);
    }

    let mut uf = UnionFind::new(n);
    for edge in &graph.edges {
        uf.union(edge.from_node, edge.to_node);
    }

    let mut count = 0usize;
    let mut largest = 0usize;
    for node_id in 0..n {
        if uf.find(node_id) == node_id {
            count += 1;
            largest = largest.max(uf.size[node_id]);
        }
    }
    (count, largest)
}

/// Bundle of degree-related statistics computed in a single pass.
struct DegreeBundle {
    in_stats: DegreeStats,
    out_stats: DegreeStats,
    hist: Vec<(usize, usize)>,
    isolated: usize,
}

fn degree_stats(graph: &CoreGraph) -> DegreeBundle {
    let n = graph.nodes.len();
    let mut in_deg = vec![0usize; n];
    let mut out_deg = vec![0usize; n];
    for edge in &graph.edges {
        out_deg[edge.from_node] += 1;
        in_deg[edge.to_node] += 1;
    }

    let mut hist: BTreeMap<usize, usize> = BTreeMap::new();
    let mut isolated = 0usize;
    for (&i_deg, &o_deg) in in_deg.iter().zip(&out_deg) {
        let total = i_deg + o_deg;
        if total == 0 {
            isolated += 1;
        }
        *hist.entry(total).or_default() += 1;
    }

    DegreeBundle {
        in_stats: degree_summary(&in_deg),
        out_stats: degree_summary(&out_deg),
        hist: hist.into_iter().collect(),
        isolated,
    }
}

fn degree_summary(degrees: &[usize]) -> DegreeStats {
    let total: usize = degrees.iter().sum();
    DegreeStats {
        min: degrees.iter().copied().min().unwrap_or(0),
        max: degrees.iter().copied().max().unwrap_or(0),
        mean: mean(total, degrees.len()),
    }
}

fn path_stats(graph: &CoreGraph) -> PathStats {
    let count = graph.path_map.len();
    if count == 0 {
        return PathStats::default();
    }

    let mut bp_total = 0usize;
    let mut bp_min = usize::MAX;
    let mut bp_max = 0usize;
    let mut steps_total = 0usize;
    let mut steps_min = usize::MAX;
    let mut steps_max = 0usize;

    for path in graph.path_map.values() {
        let steps = path.steps.len();
        let node_bp: usize = path
            .steps
            .iter()
            .map(|step| graph.nodes[step.node_id].sequence.len())
            .sum();
        let overlap: usize = path.overlaps.iter().map(|&o| o as usize).sum();
        let bp = node_bp.saturating_sub(overlap);

        bp_total += bp;
        bp_min = bp_min.min(bp);
        bp_max = bp_max.max(bp);
        steps_total += steps;
        steps_min = steps_min.min(steps);
        steps_max = steps_max.max(steps);
    }

    PathStats {
        count,
        bp_total,
        bp_min,
        bp_max,
        bp_mean: mean(bp_total, count),
        steps_min,
        steps_max,
        steps_mean: mean(steps_total, count),
    }
}

/// Disjoint-set (union-find) over node IDs `0..n` with path compression and union by size.
struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        // Path compression: point every node on the path directly at the root.
        let mut cur = x;
        while self.parent[cur] != cur {
            let next = self.parent[cur];
            self.parent[cur] = root;
            cur = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        let (big, small) = if self.size[ra] >= self.size[rb] {
            (ra, rb)
        } else {
            (rb, ra)
        };
        self.parent[small] = big;
        self.size[big] += self.size[small];
    }
}
