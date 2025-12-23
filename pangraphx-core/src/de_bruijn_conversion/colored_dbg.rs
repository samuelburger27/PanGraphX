use super::de_bruijn_graph::{DbgEdge, DeBruijn};
use crate::{CoreGraph};
use crate::core::lookup_graph::LookUpGraph;
use crate::{Kmer, core::graph::Node};
use log::error;
use std::collections::{HashMap, HashSet};
use std::path;

type DegreeMap = HashMap<Kmer, (usize, usize)>; // (in_degree, out_degree)

/// Colors are saved as bitsets in u128 integers
/// Only supports up to 128 colors
pub struct Color(u128);

impl Color {
    pub fn new() -> Self {
        Color(0)
    }

    pub fn add_color(&mut self, color_id: u8) {
        self.0 |= 1 << color_id;
    }

    pub fn has_color(&self, color_id: u8) -> bool {
        (self.0 & (1 << color_id)) != 0
    }
}

pub struct ColoredDbgEdge {
    pub dbg_edge: DbgEdge,
    pub colors: Color,
}

pub struct ColoredDBG {
    pub nodes: HashSet<Kmer>,
    pub edges: HashMap<DbgEdge, ColoredDbgEdge>,
    pub k_size: u32,
}

impl ColoredDBG {
    pub fn from_directed_graph(graph: &CoreGraph, k: usize) -> Self {
        let lookup_graph = LookUpGraph::new(graph);
        let extracted_o_kmers = lookup_graph.extract_oriented_kmers(k);
        let mut edges = HashMap::new();
        let mut all_kmers = HashSet::new();
        for (color_id, (_, kmers)) in extracted_o_kmers.into_iter().enumerate() {
            if color_id > 127 {
                error!("PangraphX currently only supports up to 128 colors.");
                panic!("Color ID exceeds maximum of 127");
            }

            for window in kmers.windows(2) {
                let from = window[0];
                let to = window[1];
                all_kmers.insert(from.kmer);
                all_kmers.insert(to.kmer);
                let dbg_edge = DbgEdge { from, to };

                edges
                    .entry(dbg_edge)
                    .or_insert_with(|| ColoredDbgEdge {
                        dbg_edge: DbgEdge { from, to },
                        colors: Color::new(),
                    })
                    .colors
                    .add_color(color_id as u8);
            }
        }
        ColoredDBG {
            nodes: all_kmers,
            edges,
            k_size: k as u32,
        }
    }

    /// Compute in-degree and out-degree for each k-mer node
    pub fn compute_degree_map(&self) -> DegreeMap {
        let mut degree_map: DegreeMap = HashMap::new();

        for edge in self.edges.keys() {
            let from_entry = degree_map.entry(edge.from.kmer).or_insert((0, 0));
            from_entry.1 += 1; // Increment out-degree

            let to_entry = degree_map.entry(edge.to.kmer).or_insert((0, 0));
            to_entry.0 += 1; // Increment in-degree
        }

        degree_map
    }
}
/// Conversion from ColoredDBG to CoreGraph
/// For conversion of colors:
/// Each color in the ColoredDBG is represented as a separate path in the CoreGraph.
///
impl From<ColoredDBG> for CoreGraph {
    fn from(colored_dbg: ColoredDBG) -> Self {
        let degree_map = colored_dbg.compute_degree_map();

        // potential path start nodes (k-mers with in-degree 0 or out-degree 0)
        let start_candidates: Vec<&Kmer> = degree_map
            .iter()
            .filter_map(|(kmer, (in_deg, out_deg))| {
                if *in_deg == 0 || *out_deg == 0 {
                    Some(kmer)
                } else {
                    None
                }
            })
            .collect();

        //Create nodes from kmers
        let node_map: HashMap<Kmer, Node> = colored_dbg
            .nodes
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
        let mut paths: Vec<crate::core::graph::Path> = Vec::new();
        let mut edges: Vec<crate::core::graph::Edge> = Vec::new();
        
        for edge in colored_dbg.edges.values() {

        }

        //Create edges from dbg edges
        // let edges: Vec<crate::core::graph::Edge> = colored_dbg
        //     .edges
        //     .into_iter()
        //     .map(|(_, colored_dbg_edge)| {
        //         let from_node = node_map.get(&colored_dbg_edge.dbg_edge.from.kmer).unwrap();
        //         let to_node = node_map.get(&colored_dbg_edge.dbg_edge.to.kmer).unwrap();

        //         crate::core::graph::Edge {
        //             from_node: from_node.id.clone(),
        //             from_orient: colored_dbg_edge.dbg_edge.from.direction,
        //             to_node: to_node.id.clone(),
        //             to_orient: colored_dbg_edge.dbg_edge.to.direction,
        //             overlap: 0, // Overlap is not defined in de Bruijn graphs
        //         }
        //     })
        //     .collect();

        CoreGraph {
            nodes: node_map.into_values().collect(),
            edges,
            paths: vec![],
        }
    }
}
