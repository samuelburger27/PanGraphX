//! Sequence search over the nodes of a genome graph.
//!
//! [`CoreGraph::search`] finds all exact occurrences of a query sequence in the
//! forward and reverse-complement strands of every node, reporting the node,
//! orientation and offset of each match.
//!
//! For queries of length at least `k`, the graph is indexed by canonical k-mers
//! (using the same 2-bit encoding as the de Bruijn graph code) and the query is
//! seeded by its first k-mer before the full match is verified. Shorter queries
//! fall back to a direct substring scan.

use std::collections::HashMap;

use bio::alphabets::dna::revcomp as reverse_complement;
use rayon::prelude::*;

use crate::core::core_types::{Node, NodeId, Orientation};
use crate::core::graph::CoreGraph;
use crate::de_bruijn_conversion::k_mers::{Kmer, encode_base, is_valid_nucleotide, roll_kmer};
use crate::error::{PanGraphXError, PanResult};

/// A single exact occurrence of a query sequence in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchHit {
    /// The node the match was found in.
    pub node_id: NodeId,
    /// The strand of the node the match was found on.
    pub orientation: Orientation,
    /// 0-based start position of the match within the oriented node sequence.
    pub offset: usize,
}

/// Maps a canonical k-mer to the positions where it occurs in the graph.
type Occurrences = HashMap<Kmer, Vec<SearchHit>>;

impl CoreGraph {
    /// Searches for all exact occurrences of `query` across every node.
    ///
    /// Both the forward and reverse-complement strand of each node are searched.
    /// For queries of length at least `k`, a canonical k-mer index is used to
    /// seed the search; shorter queries are located with a direct substring scan.
    ///
    /// The query is matched case-insensitively (it is normalised to upper case).
    ///
    /// # Errors
    /// Returns an error if `query` is empty or contains characters outside A, C,
    /// G and T.
    pub fn search(&self, query: &[u8], k: usize) -> PanResult<Vec<SearchHit>> {
        let query: Vec<u8> = query.iter().map(u8::to_ascii_uppercase).collect();

        if query.is_empty() {
            return Err(PanGraphXError::Other("query is empty".to_string()));
        }
        if query.iter().any(|&base| !is_valid_nucleotide(base)) {
            return Err(PanGraphXError::Other(
                "query contains characters outside A, C, G, T".to_string(),
            ));
        }

        let k = k.max(1);
        if query.len() < k {
            return Ok(self.search_short(&query));
        }

        let index = self.build_kmer_index(k);
        let seed = Kmer::from_bases(&query[..k]).canonical();

        let mut results = Vec::new();
        if let Some(candidates) = index.get(&seed) {
            for hit in candidates {
                if self.verify_hit(&query, hit) {
                    results.push(*hit);
                }
            }
        }
        Ok(results)
    }

    /// Builds a canonical k-mer index over both strands of every node.
    fn build_kmer_index(&self, k: usize) -> Occurrences {
        self.nodes
            .par_iter()
            .fold(HashMap::new, |mut index, node| {
                index_node(&mut index, node, k);
                index
            })
            .reduce(HashMap::new, |mut acc, part| {
                for (kmer, mut hits) in part {
                    acc.entry(kmer).or_default().append(&mut hits);
                }
                acc
            })
    }

    /// Verifies that `query` matches the node strand starting at `hit.offset`.
    fn verify_hit(&self, query: &[u8], hit: &SearchHit) -> bool {
        let node = &self.nodes[hit.node_id];
        match hit.orientation {
            Orientation::Forward => {
                node.sequence.get(hit.offset..hit.offset + query.len()) == Some(query)
            }
            Orientation::Reverse => {
                let rc = reverse_complement(&node.sequence);
                rc.get(hit.offset..hit.offset + query.len()) == Some(query)
            }
        }
    }

    /// Direct substring scan for queries shorter than the k-mer size.
    fn search_short(&self, query: &[u8]) -> Vec<SearchHit> {
        let mut results = Vec::new();
        for node in &self.nodes {
            for (offset, window) in node.sequence.windows(query.len()).enumerate() {
                if window == query {
                    results.push(SearchHit {
                        node_id: node.id,
                        orientation: Orientation::Forward,
                        offset,
                    });
                }
            }
            let rc = reverse_complement(&node.sequence);
            for (offset, window) in rc.windows(query.len()).enumerate() {
                if window == query {
                    results.push(SearchHit {
                        node_id: node.id,
                        orientation: Orientation::Reverse,
                        offset,
                    });
                }
            }
        }
        results
    }
}

/// Indexes both strands of a single node into `index`.
fn index_node(index: &mut Occurrences, node: &Node, k: usize) {
    index_sequence(index, node.id, Orientation::Forward, &node.sequence, k);
    let rc = reverse_complement(&node.sequence);
    index_sequence(index, node.id, Orientation::Reverse, &rc, k);
}

/// Indexes the k-mers of `seq`, recording their canonical form and offset.
fn index_sequence(
    index: &mut Occurrences,
    node_id: NodeId,
    orientation: Orientation,
    seq: &[u8],
    k: usize,
) {
    let mut code: u128 = 0;
    let mut len = 0usize;
    for (pos, &base) in seq.iter().enumerate() {
        if !is_valid_nucleotide(base) {
            code = 0;
            len = 0;
            continue;
        }
        if len < k {
            code = (code << 2) | u128::from(encode_base(base));
            len += 1;
        } else {
            code = roll_kmer(code, k, base);
        }
        if len == k {
            let kmer = Kmer { code, k }.canonical();
            index.entry(kmer).or_default().push(SearchHit {
                node_id,
                orientation,
                offset: pos + 1 - k,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::core_types::Nodes;
    use crate::core::graph_dto::CoreGraphDTO;

    /// Builds a graph with a single node of sequence `GATTACA` (id 0).
    fn single_node_graph(sequence: &[u8]) -> CoreGraph {
        CoreGraph::new(CoreGraphDTO {
            nodes: Nodes::from_seq(vec![sequence.to_vec()]),
            edges: vec![],
            paths: vec![],
            node_name_map: None,
        })
    }

    #[test]
    fn test_forward_match() {
        let graph = single_node_graph(b"GATTACA");
        let hits = graph.search(b"ATT", 3).unwrap();
        assert_eq!(
            hits,
            vec![SearchHit {
                node_id: 0,
                orientation: Orientation::Forward,
                offset: 1,
            }]
        );
    }

    #[test]
    fn test_reverse_match() {
        // rc("GATTACA") = "TGTAATC", which contains "TAA" at offset 2.
        let graph = single_node_graph(b"GATTACA");
        let hits = graph.search(b"TAA", 3).unwrap();
        assert_eq!(
            hits,
            vec![SearchHit {
                node_id: 0,
                orientation: Orientation::Reverse,
                offset: 2,
            }]
        );
    }

    #[test]
    fn test_multiple_occurrences() {
        let graph = single_node_graph(b"CCCCCC");
        let hits = graph.search(b"CCCC", 2).unwrap();
        assert_eq!(hits.len(), 3);
        assert!(
            hits.iter().all(|h| {
                h.node_id == 0 && h.orientation == Orientation::Forward && h.offset < 3
            })
        );
    }

    #[test]
    fn test_short_query_fallback() {
        let graph = single_node_graph(b"GATTACA");
        let hits = graph.search(b"TA", 3).unwrap();
        // Forward "TA" at offset 3; rc "TGTAATC" contains "TA" at offset 2.
        assert_eq!(
            hits,
            vec![
                SearchHit {
                    node_id: 0,
                    orientation: Orientation::Forward,
                    offset: 3,
                },
                SearchHit {
                    node_id: 0,
                    orientation: Orientation::Reverse,
                    offset: 2,
                },
            ]
        );
    }

    #[test]
    fn test_no_match() {
        let graph = single_node_graph(b"GATTACA");
        let hits = graph.search(b"GGGG", 3).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn test_invalid_query_rejected() {
        let graph = single_node_graph(b"GATTACA");
        assert!(graph.search(b"ACN", 3).is_err());
        assert!(graph.search(b"", 3).is_err());
    }

    #[test]
    fn test_case_insensitive() {
        let graph = single_node_graph(b"GATTACA");
        let hits = graph.search(b"att", 3).unwrap();
        assert_eq!(
            hits,
            vec![SearchHit {
                node_id: 0,
                orientation: Orientation::Forward,
                offset: 1,
            }]
        );
    }

    #[test]
    fn test_invalid_bases_not_indexed() {
        // "AA" then N resets the window, then "AA": k-mers spanning the N are
        // never emitted, so only the two valid "AA" windows match.
        let graph = single_node_graph(b"AANAA");
        let hits = graph.search(b"AA", 2).unwrap();
        assert_eq!(
            hits,
            vec![
                SearchHit {
                    node_id: 0,
                    orientation: Orientation::Forward,
                    offset: 0,
                },
                SearchHit {
                    node_id: 0,
                    orientation: Orientation::Forward,
                    offset: 3,
                },
            ]
        );
    }
}
