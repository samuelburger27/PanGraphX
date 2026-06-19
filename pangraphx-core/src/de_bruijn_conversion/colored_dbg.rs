use super::de_bruijn_graph::{DbgEdge, DeBruijn};
use crate::Kmer;
use crate::core::core_types::{Edge, Node, Nodes, Path, Step};
use crate::core::graph::CoreGraph;
use crate::core::graph_dto::CoreGraphDTO;
use crate::de_bruijn_conversion::k_mers::OrientedKmer;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

pub struct ColorPath {
    pub sequence: Vec<OrientedKmer>,
}

pub struct ColoredDBG {
    pub dbg: DeBruijn,
    pub paths: Vec<ColorPath>,
}

impl ColoredDBG {
    #[must_use]
    pub fn from_directed_graph(graph: CoreGraphDTO, k: usize) -> Self {
        let lookup_graph = CoreGraph::new(graph);
        let extracted_o_kmers = lookup_graph.extract_kmers_paths(k);

        // Parallelize path processing with fold + reduce for thread-local aggregation
        let (edges, all_kmers, paths) = extracted_o_kmers
            .into_par_iter()
            .fold(
                || (HashSet::new(), HashSet::new(), Vec::new()),
                |(mut edges_local, mut kmers_local, mut paths_local), (_, kmers)| {
                    let mut path = ColorPath {
                        sequence: Vec::new(),
                    };
                    for window in kmers.windows(2) {
                        let from = window[0];
                        let to = window[1];
                        kmers_local.insert(from.kmer);
                        kmers_local.insert(to.kmer);
                        let dbg_edge = DbgEdge { from, to };
                        edges_local.insert(dbg_edge);
                        path.sequence.push(from);
                    }
                    // Add the last kmer to the path
                    if let Some(last_kmer) = kmers.last() {
                        path.sequence.push(*last_kmer);
                    }
                    paths_local.push(path);
                    (edges_local, kmers_local, paths_local)
                },
            )
            .reduce(
                || (HashSet::new(), HashSet::new(), Vec::new()),
                |(mut edges1, mut kmers1, mut paths1), (edges2, kmers2, paths2)| {
                    edges1.extend(edges2);
                    kmers1.extend(kmers2);
                    paths1.extend(paths2);
                    (edges1, kmers1, paths1)
                },
            );

        Self {
            dbg: DeBruijn {
                kmers: all_kmers,
                edges,
                k_size: k as u32,
            },
            paths,
        }
    }
}

/// Conversion from `ColoredDBG` to `CoreGraph`
impl From<ColoredDBG> for CoreGraphDTO {
    fn from(colored_dbg: ColoredDBG) -> Self {
        //Create nodes from kmers
        let node_map: HashMap<Kmer, Node> = colored_dbg
            .dbg
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
        let edges: Vec<Edge> = colored_dbg
            .dbg
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
                    overlap: colored_dbg.dbg.k_size - 1,
                }
            })
            .collect();

        let paths: Vec<Path> = colored_dbg
            .paths
            .into_iter()
            .enumerate()
            .map(|(i, color_path)| {
                let steps: Vec<Step> = color_path
                    .sequence
                    .into_iter()
                    .map(|o_kmer| {
                        let node = node_map.get(&o_kmer.kmer).unwrap();
                        Step {
                            node_id: node.id,
                            orientation: o_kmer.direction,
                        }
                    })
                    .collect();
                // TODO for now, set overlaps to empty
                let overlaps = Vec::new();
                Path {
                    name: vec![b'C', char::from_digit(i as u32, 10).unwrap() as u8],
                    steps,
                    overlaps,
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
            paths,
            node_name_map: None,
        }
    }
}
