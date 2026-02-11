use super::de_bruijn_graph::{DbgEdge, DeBruijn};
use crate::Kmer;
use crate::core::graph_dto::{CoreGraphDTO, Node, Path, Step};
use crate::core::graph::CoreGraph;
use crate::de_bruijn_conversion::k_mers::OrientedKmer;
use std::collections::{HashMap, HashSet};

pub struct ColorPath {
    pub sequence: Vec<OrientedKmer>,
}

pub struct ColoredDBG {
    pub dbg: DeBruijn,
    pub paths: Vec<ColorPath>,
}

impl ColoredDBG {
    pub fn from_directed_graph(graph: CoreGraphDTO, k: usize) -> Self {
        let lookup_graph = CoreGraph::new(graph);
        let extracted_o_kmers = lookup_graph.extract_kmers_paths(k);
        let mut edges = HashSet::new();
        let mut all_kmers = HashSet::new();
        let mut paths = Vec::new();
        for (_, kmers) in extracted_o_kmers.into_iter() {
            let mut path = ColorPath {
                sequence: Vec::new(),
            };
            for window in kmers.windows(2) {
                let from = window[0];
                let to = window[1];
                all_kmers.insert(from.kmer);
                all_kmers.insert(to.kmer);
                let dbg_edge = DbgEdge { from, to };
                edges.insert(dbg_edge);
                path.sequence.push(from);
            }
            // Add the last kmer to the path
            if let Some(last_kmer) = kmers.last() {
                path.sequence.push(*last_kmer);
            }
            paths.push(path);
        }

        ColoredDBG {
            dbg: DeBruijn {
                kmers: all_kmers,
                edges,
                k_size: k as u32,
            },
            paths,
        }
    }
}

/// Conversion from ColoredDBG to CoreGraph
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
        let edges: Vec<crate::core::graph_dto::Edge> = colored_dbg
            .dbg
            .edges
            .into_iter()
            .map(|dbg_edge| {
                let from_node = node_map.get(&dbg_edge.from.kmer).unwrap();
                let to_node = node_map.get(&dbg_edge.to.kmer).unwrap();
                crate::core::graph_dto::Edge {
                    from_node: from_node.id.clone(),
                    from_orient: dbg_edge.from.direction,
                    to_node: to_node.id.clone(),
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
                            node_id: node.id.clone(),
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

        let nodes = node_map.values().cloned().collect();

        CoreGraphDTO {
            nodes,
            edges,
            paths,
            node_name_map: None,
        }
    }
}
