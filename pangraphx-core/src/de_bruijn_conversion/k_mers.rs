use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::vec;

use crate::core::graph::{Edge, Node, NodeId, Orientation, Path};
use crate::core::lookup_graph::LookUpGraph;
use crate::de_bruijn_conversion::de_bruijn_graph::DbgEdge;

/// Encode 2-bit DNA: A=0, C=1, G=2, T=3, N=0
#[inline(always)]
fn encode_base(b: u8) -> u8 {
    match b {
        b'A' | b'a' => 0,
        b'C' | b'c' => 1,
        b'G' | b'g' => 2,
        b'T' | b't' => 3,
        _ => 0, // N acts as A
    }
}

/// Reverse-complement single base (2-bit)
#[inline(always)]
fn rc_base(x: u8) -> u8 {
    match x {
        0 => 3, // A->T
        1 => 2, // C->G
        2 => 1, // G->C
        3 => 0, // T->A
        _ => 0,
    }
}

#[inline(always)]
fn suffix_mask(k: usize) -> u128 {
    (1u128 << (2 * (k - 1))) - 1
}

/// Roll k-mer by adding next base and removing first base
/// Assumes 1 ≤ k ≤ 63
#[inline(always)]
pub fn roll_kmer(code: u128, k: usize, next_base: u8) -> u128 {
    let mask: u128 = suffix_mask(k - 1);
    let next_code = encode_base(next_base) as u128;
    ((code & mask) << 2) | next_code
}

/// max k-mer can be increased if needed ???
/// 2-bit encoded k-mer stored in u128 (supports k ≤ 63)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Kmer {
    pub code: u128, // 2*k bits
    pub k: usize,
}

impl Kmer {
    /// Create from raw slice of bytes
    #[inline]
    pub fn from_bases(bases: &[u8]) -> Self {
        let mut code: u128 = 0;
        for &b in bases {
            code = (code << 2) | encode_base(b) as u128;
        }
        Self {
            code,
            k: bases.len(),
        }
    }

    /// Compute reverse complement
    #[inline]
    pub fn rev_comp(&self) -> Self {
        let mut x = self.code;
        let mut rc: u128 = 0;
        for _ in 0..self.k {
            let b = (x & 3) as u8;
            rc = (rc << 2) | rc_base(b) as u128;
            x >>= 2;
        }
        Self {
            code: rc,
            k: self.k,
        }
    }

    /// Canonical form = lexicographically smaller among forward and RC
    #[inline]
    pub fn canonical(&self) -> Self {
        let rc = self.rev_comp();
        if self.code <= rc.code { *self } else { rc }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![0; self.k];
        let mut x = self.code;
        for i in (0..self.k).rev() {
            let b = (x & 3) as u8;
            let base = match b {
                0 => b'A',
                1 => b'C',
                2 => b'G',
                3 => b'T',
                _ => b'N',
            };
            bytes[i] = base;
            x >>= 2;
        }
        bytes
    }
}

impl Display for Kmer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.to_bytes();
        write!(f, "{}", String::from_utf8_lossy(&bytes))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct OrientedKmer {
    pub kmer: Kmer,
    pub direction: Orientation,
}

impl OrientedKmer {
    pub fn from_code(code: u128, k: usize) -> Self {
        let raw = Kmer { code, k };
        let canonical = raw.canonical();
        if canonical.code == raw.code {
            return Self {
                kmer: canonical,
                direction: Orientation::Forward,
            };
        }
        return Self {
            kmer: canonical,
            direction: Orientation::Reverse,
        };
    }

    #[inline]
    pub fn from_bases(bases: &[u8]) -> Self {
        let raw = Kmer::from_bases(bases);
        let canonical = raw.canonical();
        if canonical.code == raw.code {
            return Self {
                kmer: canonical,
                direction: Orientation::Forward,
            };
        }
        return Self {
            kmer: canonical,
            direction: Orientation::Reverse,
        };
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct VisitState<'a> {
    pub node: &'a Node,
    pub suffix_code: u128,
    pub code_size: usize,
}

impl LookUpGraph<'_> {
    //pub fn

    /// Extract oriented k-mers from path sequences.
    pub fn extract_kmers_paths(&self, k: usize) -> HashMap<&Path, Vec<OrientedKmer>> {
        let mut kmers = HashMap::new();
        for path in &self.graph.paths {
            let extracted = self.extract_kmers_from_path(path, k);
            kmers.insert(path, extracted);
        }
        kmers
    }

    /// Extract k-mers from a single path
    #[inline]
    fn extract_kmers_from_path(&self, path: &Path, k: usize) -> Vec<OrientedKmer> {
        let mut result = Vec::new();

        // Iterate over node sequences as slices
        for seq in self.path_node_forward_sequence(path) {
            let mut code: u128 = 0;
            for (i, &base) in seq.into_iter().enumerate() {
                // Fill window until size k
                if i < k {
                    code = (code << 2) | encode_base(base) as u128;
                    if i == k - 1 {
                        let kmer = OrientedKmer::from_code(code, k);
                        result.push(kmer);
                    }
                } else {
                    code = roll_kmer(code, k, base);
                    let kmer = OrientedKmer::from_code(code, k);
                    result.push(kmer);
                }
            }
        }
        result
    }

    /// Depth-first traversal of a variation graph that extracts all k-mers
    /// implied by the full graph topology.
    ///
    /// To guarantee termination on cyclic graphs while preserving
    /// full topological coverage, traversal is bounded by *state*
    /// rather than nodes alone. A state is defined as:
    ///
    ///   (node, suffix_code, code_size)
    ///
    /// where `suffix_code` represents the last (k−1) bases of the
    /// current rolling window.
    fn enumerate_kmers_from_topology_dfs<'a>(
        &'a self,
        adjacency_list: &HashMap<&'a NodeId, Vec<&Edge>>,
        kmer_buffer: &mut HashSet<Kmer>,
        edge_buffer: &mut HashSet<DbgEdge>,
        visited_states: &mut HashSet<VisitState<'a>>,
        node: &'a Node,
        mut code: u128,
        mut code_size: usize,
        mut last_kmer: Option<OrientedKmer>,
        k: usize,
    ) {
        // Traverse the sequence of the current node base by base,
        // updating the rolling k-mer window.
        for &base in node.sequence.iter() {
            // Fill the window until it reaches size k
            if code_size < k {
                code = (code << 2) | encode_base(base) as u128;
                code_size += 1;

                if code_size == k {
                    let to_kmer = OrientedKmer::from_code(code, k);
                    kmer_buffer.insert(to_kmer.kmer);
                    // Cross-node edge (previous node → this node)
                    if let Some(last) = last_kmer {
                        edge_buffer.insert(DbgEdge {
                            from: last,
                            to: to_kmer,
                        });
                    }
                    last_kmer = Some(to_kmer);
                }
            } else {
                // Slide the window by one base
                code = roll_kmer(code, k, base);
                let to_kmer = OrientedKmer::from_code(code, k);
                kmer_buffer.insert(to_kmer.kmer);
                edge_buffer.insert(DbgEdge {
                    from: last_kmer.unwrap(),
                    to: to_kmer,
                });
                last_kmer = Some(to_kmer);
            }
        }

        // Compute the (k−1)-suffix context used for state pruning.
        let mut suffix_code = code;
        if code_size == k {
            suffix_code = code & suffix_mask(k);
        }

        // Stop traversal if this state was already explored
        if !visited_states.insert(VisitState {
            node,
            suffix_code,
            code_size,
        }) {
            return;
        }

        // Continue traversal across outgoing edges, carrying the
        if let Some(neighbors) = adjacency_list.get(&node.id) {
            for edge in neighbors {
                let next_node_id = &edge.to_node;
                let next_node = self.get_node_by_id(next_node_id).unwrap();
                self.enumerate_kmers_from_topology_dfs(
                    adjacency_list,
                    kmer_buffer,
                    edge_buffer,
                    visited_states,
                    next_node,
                    code,
                    code_size,
                    last_kmer,
                    k,
                );
            }
        }
    }

    pub fn extract_kmers_from_full_topology(&self, k: usize) -> (HashSet<Kmer>, HashSet<DbgEdge>) {
        let mut kmers = HashSet::new();
        let mut edges = HashSet::new();
        let adjacency_list = self.get_adjacency_list();
        let mut visited_states: HashSet<VisitState> = HashSet::new();
        for node in &self.graph.nodes {
            self.enumerate_kmers_from_topology_dfs(
                &adjacency_list,
                &mut kmers,
                &mut edges,
                &mut visited_states,
                node,
                0,
                0,
                None,
                k,
            );
        }
        (kmers, edges)
    }
}

#[cfg(test)]
mod tests {
    use crate::{CoreGraph, core::graph::Node, core::graph::Step};

    use super::*;

    // --- Basic Encoding Tests ---

    #[test]
    fn test_encode_base() {
        assert_eq!(encode_base(b'A'), 0);
        assert_eq!(encode_base(b'C'), 1);
        assert_eq!(encode_base(b'G'), 2);
        assert_eq!(encode_base(b'T'), 3);
        assert_eq!(encode_base(b'N'), 0); // N treated as A
        assert_eq!(encode_base(b'a'), 0); // Case insensitivity
    }

    #[test]
    fn test_rc_base() {
        // A(0) <-> T(3)
        assert_eq!(rc_base(0), 3);
        assert_eq!(rc_base(3), 0);
        // C(1) <-> G(2)
        assert_eq!(rc_base(1), 2);
        assert_eq!(rc_base(2), 1);
    }

    // --- Kmer Struct Tests ---

    #[test]
    fn test_kmer_round_trip() {
        let seq = b"ACGTACGT";
        let k = 8;
        let kmer = Kmer::from_bases(seq);
        let output = kmer.to_bytes();
        assert_eq!(output, seq, "Round trip encoding/decoding failed");
    }

    #[test]
    fn test_kmer_rev_comp() {
        // Sequence: TTCG (T=3, T=3, C=1, G=2)
        // RC:       CGAA (C=1, G=2, A=0, A=0)
        let seq = b"TTCG";
        let kmer = Kmer::from_bases(seq);
        let rc = kmer.rev_comp();

        assert_eq!(rc.to_bytes(), b"CGAA");
    }

    #[test]
    fn test_canonicalization() {
        // Case 1: Forward is smaller
        // AAAA (00000000) vs TTTT (11111111)
        let kmer_fwd = Kmer::from_bases(b"AAAA");
        assert_eq!(kmer_fwd.canonical().to_bytes(), b"AAAA");

        // Case 2: Reverse is smaller
        // TTTT vs AAAA
        let kmer_rev = Kmer::from_bases(b"TTTT");
        assert_eq!(kmer_rev.canonical().to_bytes(), b"AAAA");

        // Case 3: Mixed
        // TGCA (T=3, G=2, C=1, A=0) -> RC = TGCA
        // This is a palindrome
        let palindrome = Kmer::from_bases(b"TGCA");
        assert_eq!(palindrome.canonical().to_bytes(), b"TGCA");
    }

    // --- OrientedKmer Tests ---

    #[test]
    fn test_orientation_detection() {
        // "AAAA" is canonical (smaller than TTTT), so it should be Forward
        let ok_fwd = OrientedKmer::from_bases(b"AAAA");
        assert_eq!(ok_fwd.direction, Orientation::Forward);
        assert_eq!(ok_fwd.kmer.to_bytes(), b"AAAA");

        // "TTTT" is NOT canonical (larger than AAAA), so it should be Reverse
        // The stored kmer should be the canonical one (AAAA)
        let ok_rev = OrientedKmer::from_bases(b"TTTT");
        assert_eq!(ok_rev.direction, Orientation::Reverse);
        assert_eq!(ok_rev.kmer.to_bytes(), b"AAAA");
    }

    // --- Rolling Hash Tests ---

    #[test]
    fn test_roll_kmer_logic() {
        let k = 4;
        // Start: ACGT
        let start_seq = b"ACGT";
        let start_kmer = Kmer::from_bases(start_seq);

        // Roll in 'A' -> Should become CGTA
        let next_code = roll_kmer(start_kmer.code, k, b'A');
        let next_kmer = Kmer { code: next_code, k };

        assert_eq!(next_kmer.to_bytes(), b"CGTA");
    }

    // --- Extraction Logic Test ---
    #[test]
    fn test_extract_kmers_from_path() {
        let seq = b"ACGTA";
        let k = 4;
        let nodes = vec![Node {
            id: b"1".to_vec(),
            sequence: seq.to_vec(),
        }];
        let graph = CoreGraph {
            nodes,
            edges: vec![],
            paths: vec![Path {
                name: b"path1".to_vec(),
                steps: vec![Step {
                    node_id: b"1".to_vec(),
                    orientation: Orientation::Forward,
                }],
                overlaps: vec![],
            }],
        };
        let lookup = LookUpGraph::new(&graph);
        let result = lookup.extract_kmers_from_path(&graph.paths[0], k);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].kmer.to_bytes(), b"ACGT");
        assert_eq!(result[1].kmer.to_bytes(), b"CGTA");

        // test exact size k-mer
        let k = 5;
        let result_exact = lookup.extract_kmers_from_path(&graph.paths[0], k);
        assert_eq!(result_exact.len(), 1);
        assert_eq!(result_exact[0].kmer.to_bytes(), b"ACGTA");
    }

    #[test]
    fn test_extract_kmers_from_all_paths() {
        let seq1 = b"ACGTA";
        let seq2 = b"TTGCA";
        let k = 4;
        let nodes = vec![
            Node {
                id: b"1".to_vec(),
                sequence: seq1.to_vec(),
            },
            Node {
                id: b"2".to_vec(),
                sequence: seq2.to_vec(),
            },
        ];
        let graph = CoreGraph {
            nodes,
            edges: vec![],
            paths: vec![
                Path {
                    name: b"path1".to_vec(),
                    steps: vec![Step {
                        node_id: b"1".to_vec(),
                        orientation: Orientation::Forward,
                    }],
                    overlaps: vec![],
                },
                Path {
                    name: b"path2".to_vec(),
                    steps: vec![Step {
                        node_id: b"2".to_vec(),
                        orientation: Orientation::Forward,
                    }],
                    overlaps: vec![],
                },
            ],
        };
        let lookup = LookUpGraph::new(&graph);
        let result = lookup.extract_kmers_paths(k);

        assert_eq!(result.len(), 2);
        let path1_kmers = result.get(&graph.paths[0]).unwrap();
        assert_eq!(path1_kmers.len(), 2);
        assert_eq!(path1_kmers[0].kmer, Kmer::from_bases(b"ACGT").canonical());
        assert_eq!(path1_kmers[1].kmer, Kmer::from_bases(b"CGTA").canonical());
        let path2_kmers = result.get(&graph.paths[1]).unwrap();
        assert_eq!(path2_kmers.len(), 2);
        assert_eq!(path2_kmers[0].kmer, Kmer::from_bases(b"TTGC").canonical());
        assert_eq!(path2_kmers[1].kmer, Kmer::from_bases(b"TGCA").canonical());
    }
}
