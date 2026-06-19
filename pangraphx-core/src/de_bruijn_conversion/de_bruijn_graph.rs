use crate::core::core_types::{Edge, Node, Nodes};
use crate::core::graph::CoreGraph;
use crate::de_bruijn_conversion::k_mers::OrientedKmer;
use crate::{CoreGraphDTO, Kmer};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct DbgEdge {
    pub from: OrientedKmer,
    pub to: OrientedKmer,
}

pub struct DeBruijn {
    pub kmers: HashSet<Kmer>,
    pub edges: HashSet<DbgEdge>,
    pub k_size: u32,
}

impl DeBruijn {
    #[must_use]
    pub fn from_directed_graph(graph: CoreGraphDTO, k: usize) -> Self {
        let lookup_graph = CoreGraph::new(graph);
        let extracted_o_kmers = lookup_graph.extract_kmers_paths(k);

        // Parallelize path processing with fold + reduce for thread-local aggregation
        let (edges, all_kmers) = extracted_o_kmers
            .into_par_iter()
            .fold(
                || (HashSet::new(), HashSet::new()),
                |(mut edges_local, mut kmers_local), (_, kmers)| {
                    // Construct de Bruijn edges from kmers
                    for window in kmers.windows(2) {
                        let from = window[0];
                        let to = window[1];
                        kmers_local.insert(from.kmer);
                        kmers_local.insert(to.kmer);
                        edges_local.insert(DbgEdge { from, to });
                    }
                    (edges_local, kmers_local)
                },
            )
            .reduce(
                || (HashSet::new(), HashSet::new()),
                |(mut edges1, mut kmers1), (edges2, kmers2)| {
                    edges1.extend(edges2);
                    kmers1.extend(kmers2);
                    (edges1, kmers1)
                },
            );

        Self {
            kmers: all_kmers,
            edges,
            k_size: k as u32,
        }
    }

    #[must_use]
    pub fn from_directed_graph_full_topography(graph: CoreGraphDTO, k: usize) -> Self {
        let lookup_graph = CoreGraph::new(graph);
        let (kmers, edges) = lookup_graph.extract_kmers_from_full_topology(k);
        Self {
            kmers,
            edges,
            k_size: k as u32,
        }
    }
}

impl From<DeBruijn> for CoreGraphDTO {
    fn from(db_graph: DeBruijn) -> Self {
        //Create nodes from kmers
        let node_map: HashMap<Kmer, Node> = db_graph
            .kmers
            .into_iter()
            .enumerate()
            .map(|(i, kmer)| {
                (
                    kmer,
                    Node {
                        id: i,
                        sequence: kmer.to_bytes(),
                    },
                )
            })
            .collect();

        //Create edges from dbg edges
        let edges: Vec<Edge> = db_graph
            .edges
            .into_iter()
            .map(|dbg_edge| {
                let from_node = node_map.get(&dbg_edge.from.kmer).unwrap();
                let to_node = node_map.get(&dbg_edge.to.kmer).unwrap();
                Edge {
                    from_node: from_node.id,
                    from_orient: dbg_edge.from.direction,
                    to_node: to_node.id,
                    to_orient: dbg_edge.to.direction,
                    overlap: db_graph.k_size - 1,
                }
            })
            .collect();

        // Create nodes in the order of their IDs
        let mut seq = vec![Vec::new(); node_map.len()];

        for node in node_map.values() {
            seq[node.id] = node.sequence.clone();
        }

        let nodes = Nodes::from_seq(seq);

        Self {
            nodes,
            edges,
            paths: Vec::new(), // Paths are not represented in de Bruijn graph
            node_name_map: None,
        }
    }
}
