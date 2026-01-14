use std::collections::{HashMap, HashSet};

use crate::core::graph::Node;
use crate::core::lookup_graph::LookUpGraph;
use crate::de_bruijn_conversion::k_mers::OrientedKmer;
use crate::{CoreGraph, Kmer};

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
    pub fn from_directed_graph(graph: &CoreGraph, k: usize) -> Self {
        let lookup_graph = LookUpGraph::new(graph);
        let extracted_o_kmers = lookup_graph.extract_kmers_paths(k);
        let mut edges = HashSet::new();
        let mut all_kmers = HashSet::new();
        for (_, kmers) in extracted_o_kmers {
            // Construct de Bruijn edges from kmers
            for window in kmers.windows(2) {
                let from = window[0];
                let to = window[1];
                all_kmers.insert(from.kmer);
                all_kmers.insert(to.kmer);
                edges.insert(DbgEdge { from, to });
            }
        }
        DeBruijn {
            kmers: all_kmers,
            edges,
            k_size: k as u32,
        }
    }

    pub fn from_directed_graph_full_topography(graph: &CoreGraph, k: usize) -> Self {
        let lookup_graph = LookUpGraph::new(graph);
        let extracted_o_kmers = lookup_graph.extract_kmers_from_full_topology(k);
        //let mut edges = HashSet::new();
        let mut all_kmers = HashSet::new();
        
        todo!("");
    }
}

impl From<DeBruijn> for CoreGraph {
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
                        id: i.to_string().into_bytes(),
                        sequence: kmer.to_bytes(),
                    },
                )
            })
            .collect();

        //Create edges from dbg edges
        let edges: Vec<crate::core::graph::Edge> = db_graph
            .edges
            .into_iter()
            .map(|dbg_edge| {
                let from_node = node_map.get(&dbg_edge.from.kmer).unwrap();
                let to_node = node_map.get(&dbg_edge.to.kmer).unwrap();
                crate::core::graph::Edge {
                    from_node: from_node.id.clone(),
                    from_orient: dbg_edge.from.direction,
                    to_node: to_node.id.clone(),
                    to_orient: dbg_edge.to.direction,
                    overlap: db_graph.k_size - 1,
                }
            })
            .collect();
        let nodes = node_map.values().cloned().collect();

        CoreGraph {
            nodes,
            edges,
            paths: Vec::new(), // Paths are not represented in de Bruijn graph
        }
    }
}
