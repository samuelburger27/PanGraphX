use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::fmt::Display;
use std::vec;

use rayon::prelude::*;

use crate::core::core_types::{Node, Orientation, Path};
use crate::core::graph::CoreGraph;
use crate::de_bruijn_conversion::de_bruijn_graph::DbgEdge;

/// Encodes a single DNA byte into a 2-bit representation.
///
/// # Mapping
/// * 'A' / 'a' / 'N' -> 0 (binary 00)
/// * 'C' / 'c'       -> 1 (binary 01)
/// * 'G' / 'g'       -> 2 (binary 10)
/// * 'T' / 't'       -> 3 (binary 11)
const fn encode_base(b: u8) -> u8 {
    match b {
        b'A' | b'a' => 0,
        b'C' | b'c' => 1,
        b'G' | b'g' => 2,
        _ => 3,
    }
}

/// Checks if a byte represents a valid nucleotide (A, C, G, T).
///
const fn is_valid_nucleotide(b: u8) -> bool {
    matches!(b, b'A' | b'a' | b'C' | b'c' | b'G' | b'g' | b'T' | b't')
}

/// Computes the Reverse Complement of a 2-bit encoded base.
///
/// # Mapping
/// * 0 (A) <-> 3 (T)
/// * 1 (C) <-> 2 (G)
const fn rc_base(x: u8) -> u8 {
    match x {
        0 => 3, // A->T
        1 => 2, // C->G
        2 => 1, // G->C
        _ => 0, // T->A and anything else
    }
}

const fn suffix_mask(n_bases: usize) -> u128 {
    (1u128 << (2 * n_bases)) - 1
}

/// Performs a "rolling" update of a k-mer code.
///
/// This operation simulates a sliding window moving one base to the right.
/// 1. Masks the current code to remove the most significant base (the oldest).
/// 2. Left-shifts the result by 2 bits.
/// 3. ORs the new base into the least significant position.
///
/// # Arguments
/// * `code`: The current 2-bit encoded k-mer.
/// * `k`: The length of the k-mer (must be 1 <= k <= 63).
/// * `next_base`: The ASCII byte of the incoming nucleotide.
#[must_use]
pub fn roll_kmer(code: u128, k: usize, next_base: u8) -> u128 {
    let mask: u128 = suffix_mask(k - 1);
    let next_code = u128::from(encode_base(next_base));
    ((code & mask) << 2) | next_code
}

/// A compact representation of a DNA k-mer.
///
/// The sequence is stored in a `u128` using 2-bit encoding.
/// This allows for efficient storage and hashing of k-mers up to length k=63.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Kmer {
    /// The 2-bit encoded sequence. Uses `2 * k` bits.
    pub code: u128,
    /// The length of the k-mer.
    pub k: usize,
}

impl Debug for Kmer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.to_bytes();
        write!(f, "Kmer({})", String::from_utf8_lossy(&bytes))
    }
}

impl Kmer {
    /// Creates a Kmer from a slice of bytes (e.g., `b"ACGT"`).
    #[inline]
    #[must_use]
    pub fn from_bases(bases: &[u8]) -> Self {
        let mut code: u128 = 0;
        for &b in bases {
            code = (code << 2) | u128::from(encode_base(b));
        }
        Self {
            code,
            k: bases.len(),
        }
    }

    /// Computes the reverse complement of the k-mer.
    ///
    /// This reverses the order of bases and complements each base (A <-> T, C <-> G).
    #[inline]
    #[must_use]
    pub fn rev_comp(&self) -> Self {
        let mut x = self.code;
        let mut rc: u128 = 0;
        for _ in 0..self.k {
            let b = (x & 3) as u8;
            rc = (rc << 2) | u128::from(rc_base(b));
            x >>= 2;
        }
        Self {
            code: rc,
            k: self.k,
        }
    }

    /// Returns the canonical representation of the k-mer.
    ///
    /// The canonical form is defined as the lexicographically smaller sequence
    /// between the forward k-mer and its reverse complement.
    #[inline]
    #[must_use]
    pub fn canonical(&self) -> Self {
        let rc = self.rev_comp();
        if self.code <= rc.code { *self } else { rc }
    }

    /// Decodes the `u128` back into a human-readable byte vector.
    #[must_use]
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

/// A K-mer annotated with its orientation relative to the canonical form.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrientedKmer {
    pub kmer: Kmer,
    pub direction: Orientation,
}

impl Debug for OrientedKmer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.to_bytes();
        write!(
            f,
            "OrientedKmer{{Sequence {}, Orientation {}}}",
            String::from_utf8_lossy(&bytes),
            self.direction
        )
    }
}

impl OrientedKmer {
    /// Constructs an `OrientedKmer` from a raw integer code.
    ///
    /// Determines if the provided code corresponds to the Forward or Reverse
    /// canonical direction.
    #[must_use]
    pub fn from_code(code: u128, k: usize) -> Self {
        let raw = Kmer { code, k };
        let canonical = raw.canonical();
        if canonical.code == raw.code {
            return Self {
                kmer: canonical,
                direction: Orientation::Forward,
            };
        }
        Self {
            kmer: canonical,
            direction: Orientation::Reverse,
        }
    }

    /// Constructs an `OrientedKmer` from a byte slice.
    #[inline]
    #[must_use]
    pub fn from_bases(bases: &[u8]) -> Self {
        let raw = Kmer::from_bases(bases);
        let canonical = raw.canonical();
        if canonical.code == raw.code {
            return Self {
                kmer: canonical,
                direction: Orientation::Forward,
            };
        }
        Self {
            kmer: canonical,
            direction: Orientation::Reverse,
        }
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.direction == Orientation::Forward {
            self.kmer.to_bytes()
        } else {
            self.kmer.rev_comp().to_bytes()
        }
    }
}

/// Tracks the traversal state within the variation graph to prevent infinite loops.
///
/// In a De Bruijn Graph context, visiting a node isn't enough to determine uniqueness;
/// we must know the *context* (incoming history) with which we entered the node.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct VisitState<'a> {
    /// The graph node being visited.
    pub node: &'a Node,
    /// The encoded suffix (length k-1) preceding the current position.
    pub suffix_code: u128,
    /// The current size of the rolling window (may be < k at start of traversal).
    pub code_size: usize,
}

impl CoreGraph {
    /// Extracts oriented k-mers from all predefined paths in the graph.
    ///
    /// Returns a map associating each `Path` with its sequence of k-mers.
    ///
    /// # Implementation
    ///
    /// This operation is parallelized across available cores using Rayon.
    /// Each path's k-mer extraction is independent, allowing near-linear speedup.
    #[must_use]
    pub fn extract_kmers_paths(&self, k: usize) -> HashMap<&Path, Vec<OrientedKmer>> {
        self.path_map
            .values()
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|path| (path, self.extract_kmers_from_path(path, k)))
            .collect()
    }

    /// Extracts k-mers from a single specific path.
    ///
    /// This method linearizes the path nodes and runs a sliding window over the sequence.
    #[inline]
    fn extract_kmers_from_path(&self, path: &Path, k: usize) -> Vec<OrientedKmer> {
        let mut result = Vec::new();

        // Iterate over node sequences as slices
        for seq in self.path_node_forward_sequence(path) {
            let mut code: u128 = 0;
            let mut len = 0;

            for &base in seq {
                if !is_valid_nucleotide(base) {
                    // Reset on invalid base
                    code = 0;
                    len = 0;
                    continue;
                }
                // Fill window until size k
                if len < k {
                    code = (code << 2) | u128::from(encode_base(base));
                    len += 1;

                    if len == k {
                        let kmer = OrientedKmer::from_code(code, k);
                        result.push(kmer);
                    }
                } else {
                    // Slide window by one base
                    code = roll_kmer(code, k, base);
                    let kmer = OrientedKmer::from_code(code, k);
                    result.push(kmer);
                }
            }
        }
        result
    }

    /// Depth-first traversal of a variation graph to extract all topological k-mers.
    ///
    /// # Algorithm
    /// 1. **Traversal:** Moving through the variation graph node by node.
    /// 2. **Sliding Window:** Maintaining a rolling hash of the last `k` bases encountered.
    /// 3. **State Tracking:** To handle cycles (loops in the graph), we track `VisitState`.
    ///    We only stop recursion if we encounter a specific Node *with the exact same incoming (k-1) context*.
    ///
    /// # Arguments
    /// * `adjacency_list`: Map of `NodeId` to outgoing Edges.
    /// * `kmer_buffer`: Collection to store unique canonical K-mers found.
    /// * `edge_buffer`: Collection to store unique De Bruijn edges (`Kmer_A -> Kmer_B`).
    /// * `visited_states`: Set of previously seen (Node + Context) states.
    /// * `node`: The current graph node being traversed.
    /// * `code`: The current rolling hash accumulator.
    /// * `code_size`: How many bases are currently in the accumulator.
    /// * `last_kmer`: The previous k-mer emitted (used to create edges).
    /// * `k`: The target k-mer size.
    fn enumerate_kmers_from_topology<'a>(
        &'a self,
        kmer_buffer: &mut HashSet<Kmer>,
        edge_buffer: &mut HashSet<DbgEdge>,
        visited_states: &mut HashSet<VisitState<'a>>,
        start_node: &'a Node,
        k: usize,
    ) {
        let mut stack = Vec::with_capacity(128);

        // Push the initial node onto the stack
        stack.push(StackItem {
            node: start_node,
            code: 0,
            code_size: 0,
            last_kmer: None,
        });

        while let Some(item) = stack.pop() {
            let StackItem {
                node,
                mut code,
                mut code_size,
                mut last_kmer,
            } = item;

            // --- 1. Process Node Sequence ---
            // Iterate through the sequence of the current node to update the rolling window.
            for &base in &node.sequence {
                if !is_valid_nucleotide(base) {
                    // Reset
                    code = 0;
                    code_size = 0;
                    last_kmer = None;
                    continue;
                }

                if code_size < k {
                    // FILLING BUFFER
                    code = (code << 2) | u128::from(encode_base(base));
                    code_size += 1;

                    if code_size == k {
                        // Buffer just became full -> Emit first k-mer
                        let to_kmer = OrientedKmer::from_code(code, k);
                        kmer_buffer.insert(to_kmer.kmer);

                        // If we had a valid previous k-mer (from incoming edge), connect them
                        if let Some(last) = last_kmer {
                            edge_buffer.insert(DbgEdge {
                                from: last,
                                to: to_kmer,
                            });
                        }
                        last_kmer = Some(to_kmer);
                    }
                } else {
                    code = roll_kmer(code, k, base);

                    let to_kmer = OrientedKmer::from_code(code, k);
                    kmer_buffer.insert(to_kmer.kmer);

                    if let Some(last) = last_kmer {
                        edge_buffer.insert(DbgEdge {
                            from: last,
                            to: to_kmer,
                        });
                    }
                    last_kmer = Some(to_kmer);
                }
            }

            // --- 2. State Checking (Cycle Detection) ---
            // Only mask if we actually have a full k-mer context
            let suffix_code = if code_size == k {
                code & suffix_mask(k - 1)
            } else {
                code
            };
            // we stop to prevent infinite loops in cycles.
            if !visited_states.insert(VisitState {
                node,
                suffix_code,
                code_size,
            }) {
                continue;
            }

            // --- 3. Push Neighbors ---
            // Propagate the current rolling hash state to all outgoing neighbors.
            if let Some(neighbors) = self.adjacency_list.get(&node.id) {
                for edge in neighbors {
                    let next_node_id = self.edges[*edge].to_node;
                    let next_node = &self.nodes[next_node_id];
                    stack.push(StackItem {
                        node: next_node,
                        code,
                        code_size,
                        last_kmer,
                    });
                }
            }
        }
    }

    /// Extracts all k-mers and edges implied by the full graph topology.
    ///
    /// This converts the Sequence Graph (Variation Graph) into a De Bruijn Graph
    /// representation by traversing all valid paths through the nodes.
    ///
    /// Returns a tuple: `(Set of Nodes (Kmers), Set of Edges (DbgEdges))`
    #[must_use]
    pub fn extract_kmers_from_full_topology(&self, k: usize) -> (HashSet<Kmer>, HashSet<DbgEdge>) {
        let mut kmers = HashSet::new();
        let mut edges = HashSet::new();
        let mut visited_states: HashSet<VisitState> = HashSet::new();
        // Start DFS from every node to ensure disconnected components and
        // all possible start points are covered.
        for node in &self.nodes {
            self.enumerate_kmers_from_topology(
                &mut kmers,
                &mut edges,
                &mut visited_states,
                node,
                k,
            );
        }
        (kmers, edges)
    }
}

// A helper struct to manage the stack frame on the heap
struct StackItem<'a> {
    node: &'a Node,
    code: u128,
    code_size: usize,
    last_kmer: Option<OrientedKmer>,
}

#[cfg(test)]
mod tests {
    use crate::{
        CoreGraphDTO,
        core::core_types::{Edge, Nodes, Step},
    };

    use super::*;

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
        let nodes = Nodes::from_seq(vec![seq.to_vec()]);

        let path_name = b"path1".to_vec();
        let graph = CoreGraphDTO {
            nodes,
            edges: vec![],
            paths: vec![Path {
                name: path_name.clone(),
                steps: vec![Step {
                    node_id: 0,
                    orientation: Orientation::Forward,
                }],
                overlaps: vec![],
            }],
            node_name_map: None,
        };
        let lookup = CoreGraph::new(graph);
        let result = lookup.extract_kmers_from_path(&lookup.path_map[&path_name], k);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].kmer.to_bytes(), b"ACGT");
        assert_eq!(result[1].kmer.to_bytes(), b"CGTA");

        // test exact size k-mer
        let k = 5;
        let result_exact = lookup.extract_kmers_from_path(&lookup.path_map[&path_name], k);
        assert_eq!(result_exact.len(), 1);
        assert_eq!(result_exact[0].kmer.to_bytes(), b"ACGTA");
    }

    #[test]
    fn test_extract_kmers_from_all_paths() {
        let seq1 = b"ACGTA";
        let seq2 = b"TTGCA";
        let k = 4;
        let nodes = Nodes::from_seq(vec![seq1.to_vec(), seq2.to_vec()]);

        let graph = CoreGraphDTO {
            nodes,
            edges: vec![],
            paths: vec![
                Path {
                    name: b"path1".to_vec(),
                    steps: vec![Step {
                        node_id: 0,
                        orientation: Orientation::Forward,
                    }],
                    overlaps: vec![],
                },
                Path {
                    name: b"path2".to_vec(),
                    steps: vec![Step {
                        node_id: 1,
                        orientation: Orientation::Forward,
                    }],
                    overlaps: vec![],
                },
            ],
            node_name_map: None,
        };
        let lookup = CoreGraph::new(graph);
        let result = lookup.extract_kmers_paths(k);

        assert_eq!(result.len(), 2);
        let path1_kmers = result.get(&lookup.path_map[&b"path1".to_vec()]).unwrap();
        assert_eq!(path1_kmers.len(), 2);
        assert_eq!(path1_kmers[0].kmer, Kmer::from_bases(b"ACGT").canonical());
        assert_eq!(path1_kmers[1].kmer, Kmer::from_bases(b"CGTA").canonical());
        let path2_kmers = result.get(&lookup.path_map[&b"path2".to_vec()]).unwrap();
        assert_eq!(path2_kmers.len(), 2);
        assert_eq!(path2_kmers[0].kmer, Kmer::from_bases(b"TTGC").canonical());
        assert_eq!(path2_kmers[1].kmer, Kmer::from_bases(b"TGCA").canonical());
    }

    #[test]
    fn test_linear_topology_simple() {
        // Graph: Node1[AAAA] -> Node2[TTTT]
        // k=3
        // Sequence: AAAATTTT
        // Expected Kmers: AAA, AAA, AAT, ATT, TTT, TTT...
        // Unique Kmers: AAA, AAT, ATT, TTT
        let nodes = Nodes::from_seq(vec![b"AAAA".to_vec(), b"TTTT".to_vec()]);

        let edges = vec![Edge {
            from_node: 0,
            from_orient: Orientation::Forward,
            to_node: 1,
            to_orient: Orientation::Forward,
            overlap: 0,
        }];

        let graph = CoreGraphDTO {
            nodes,
            edges,
            paths: vec![],
            node_name_map: None,
        };

        let lookup = CoreGraph::new(graph);
        let (kmers, dbg_edges) = lookup.extract_kmers_from_full_topology(3);
        let expected_kmers: HashSet<Kmer> = vec![
            Kmer::from_bases(b"AAA").canonical(),
            Kmer::from_bases(b"AAT").canonical(),
            Kmer::from_bases(b"ATT").canonical(),
            Kmer::from_bases(b"TTT").canonical(),
        ]
        .into_iter()
        .collect();
        assert_eq!(kmers, expected_kmers);

        let expected_edges: HashSet<DbgEdge> = vec![
            DbgEdge {
                from: OrientedKmer::from_bases(b"AAA"),
                to: OrientedKmer::from_bases(b"AAA"),
            },
            DbgEdge {
                from: OrientedKmer::from_bases(b"AAA"),
                to: OrientedKmer::from_bases(b"AAT"),
            },
            DbgEdge {
                from: OrientedKmer::from_bases(b"AAT"),
                to: OrientedKmer::from_bases(b"ATT"),
            },
            DbgEdge {
                from: OrientedKmer::from_bases(b"ATT"),
                to: OrientedKmer::from_bases(b"TTT"),
            },
            DbgEdge {
                from: OrientedKmer::from_bases(b"TTT"),
                to: OrientedKmer::from_bases(b"TTT"),
            },
        ]
        .into_iter()
        .collect();
        assert_eq!(dbg_edges, expected_edges);
    }

    #[test]
    fn test_branching_topology() {
        // Graph: Node1[AAA] -> Node2[TTT]
        //                   -> Node3[GGG]
        // k=2
        // From Node1: AA
        // Then splits: AT, TT (via Node2) or AG, GG (via Node3)
        let nodes = Nodes::from_seq(vec![b"AAA".to_vec(), b"TTT".to_vec(), b"GGG".to_vec()]);

        let edges = vec![
            Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 1,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
            Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 2,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
        ];

        let graph = CoreGraphDTO {
            nodes,
            edges,
            paths: vec![],
            node_name_map: None,
        };

        let lookup = CoreGraph::new(graph);
        let (kmers, _dbg_edges) = lookup.extract_kmers_from_full_topology(2);

        let expected_kmers: HashSet<Kmer> = vec![
            Kmer::from_bases(b"AA").canonical(),
            Kmer::from_bases(b"AT").canonical(),
            Kmer::from_bases(b"TT").canonical(),
            Kmer::from_bases(b"AG").canonical(),
            Kmer::from_bases(b"GG").canonical(),
        ]
        .into_iter()
        .collect();

        assert_eq!(kmers, expected_kmers);
    }

    #[test]
    fn test_merging_topology() {
        // Graph: Node1[AAA] -> Node3[TTT]
        //        Node2[GGG] -> Node3[TTT]
        // k=2
        // Both paths merge at Node3
        let nodes = Nodes::from_seq(vec![b"AAA".to_vec(), b"GGG".to_vec(), b"TTT".to_vec()]);

        let edges = vec![
            Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 2,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
            Edge {
                from_node: 1,
                from_orient: Orientation::Forward,
                to_node: 2,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
        ];

        let graph = CoreGraphDTO {
            nodes,
            edges,
            paths: vec![],
            node_name_map: None,
        };

        let lookup = CoreGraph::new(graph);
        let (kmers, _dbg_edges) = lookup.extract_kmers_from_full_topology(2);

        let expected_kmers: HashSet<Kmer> = vec![
            Kmer::from_bases(b"AA").canonical(),
            Kmer::from_bases(b"AT").canonical(),
            Kmer::from_bases(b"TT").canonical(),
            Kmer::from_bases(b"GG").canonical(),
            Kmer::from_bases(b"GT").canonical(),
        ]
        .into_iter()
        .collect();

        assert_eq!(kmers, expected_kmers);
    }

    #[test]
    fn test_cycle_topology() {
        // Graph: Node1[ACGT] -> Node2[ACGT] -> Node1[ACGT] (cycle)
        // k=2
        // Should extract kmers without infinite looping
        let nodes = Nodes::from_seq(vec![b"ACGT".to_vec(), b"ACGT".to_vec()]);

        let edges = vec![
            Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 1,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
            Edge {
                from_node: 1,
                from_orient: Orientation::Forward,
                to_node: 0,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
        ];

        let graph = CoreGraphDTO {
            nodes,
            edges,
            paths: vec![],
            node_name_map: None,
        };

        let lookup = CoreGraph::new(graph);
        let (kmers, _dbg_edges) = lookup.extract_kmers_from_full_topology(2);

        // Should contain all 2-mers from ACGTACGTACGT... pattern
        let expected_kmers: HashSet<Kmer> = vec![
            Kmer::from_bases(b"AC").canonical(),
            Kmer::from_bases(b"CG").canonical(),
            Kmer::from_bases(b"GT").canonical(),
            Kmer::from_bases(b"TA").canonical(),
        ]
        .into_iter()
        .collect();

        assert_eq!(kmers, expected_kmers);
    }

    #[test]
    fn test_diamond_topology() {
        // Graph: Node1[AA] -> Node2[TT]
        //                  -> Node3[GG]
        //        Node2[TT] -> Node4[CC]
        //        Node3[GG] -> Node4[CC]
        // k=2
        // Diamond pattern: A->B, A->C, B->D, C->D
        let nodes = Nodes::from_seq(vec![
            b"AA".to_vec(),
            b"TT".to_vec(),
            b"GG".to_vec(),
            b"CC".to_vec(),
        ]);
        let edges = vec![
            Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 1,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
            Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 2,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
            Edge {
                from_node: 1,
                from_orient: Orientation::Forward,
                to_node: 3,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
            Edge {
                from_node: 2,
                from_orient: Orientation::Forward,
                to_node: 3,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
        ];

        let graph = CoreGraphDTO {
            nodes,
            edges,
            paths: vec![],
            node_name_map: None,
        };

        let lookup = CoreGraph::new(graph);
        let (kmers, _dbg_edges) = lookup.extract_kmers_from_full_topology(2);

        let expected_kmers: HashSet<Kmer> = vec![
            Kmer::from_bases(b"AA").canonical(),
            Kmer::from_bases(b"AT").canonical(),
            Kmer::from_bases(b"TT").canonical(),
            Kmer::from_bases(b"TC").canonical(),
            Kmer::from_bases(b"CC").canonical(),
            Kmer::from_bases(b"AG").canonical(),
            Kmer::from_bases(b"GG").canonical(),
            Kmer::from_bases(b"GC").canonical(),
        ]
        .into_iter()
        .collect();

        assert_eq!(kmers, expected_kmers);
    }

    #[test]
    fn test_invalid_bases_in_topology() {
        // Graph: Node1[AANA] -> Node2[TTNT]
        // k=2
        // Should handle N as invalid and reset
        let nodes = Nodes::from_seq(vec![b"AANA".to_vec(), b"TTNT".to_vec()]);

        let edges = vec![Edge {
            from_node: 0,
            from_orient: Orientation::Forward,
            to_node: 1,
            to_orient: Orientation::Forward,
            overlap: 0,
        }];

        let graph = CoreGraphDTO {
            nodes,
            edges,
            paths: vec![],
            node_name_map: None,
        };

        let lookup = CoreGraph::new(graph);
        let (kmers, _dbg_edges) = lookup.extract_kmers_from_full_topology(2);

        // After N, buffer resets. We should get: AA, then N resets, then AT
        // In Node2: TT, then N resets, then only T remains which is not a full k-mer
        let expected_kmers: HashSet<Kmer> = vec![
            Kmer::from_bases(b"AA").canonical(),
            Kmer::from_bases(b"AT").canonical(),
            Kmer::from_bases(b"TT").canonical(),
        ]
        .into_iter()
        .collect();

        assert_eq!(kmers, expected_kmers);
    }
}
