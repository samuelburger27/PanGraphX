use super::compaction::{self, Unitigs};
use super::extraction::DbgSets;
use super::k_mers::{OrientedKmer, validate_k};
use super::word::{KmerWord, MAX_SMALL_K, decode_word, edge_ends};
use crate::core::core_types::{Edge, Node, Nodes};
use crate::core::graph::CoreGraph;
use crate::error::{PanGraphXError, PanResult};
use crate::{CoreGraphDTO, Kmer};
use log::debug;
use rayon::prelude::*;

/// A de Bruijn edge, as the two oriented k-mers it connects.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct DbgEdge {
    pub from: OrientedKmer,
    pub to: OrientedKmer,
}

/// The distinct k-mers and edges of a de Bruijn graph, as sorted words.
///
/// The k-mer at index `i` of `kmers` is node `i`. Each edge is stored once,
/// as its canonical (k+1)-mer: the edge and its reverse-complement twin are
/// the same (k+1)-mer.
pub(crate) struct DbgCore<W> {
    pub kmers: Vec<W>,
    pub edges: Vec<W>,
}

impl<W: KmerWord> DbgCore<W> {
    pub fn from_sets(sets: DbgSets<W>) -> Self {
        let DbgSets { kmers, edges } = sets;
        Self {
            kmers: kmers.into_sorted_vec(),
            edges: edges.into_sorted_vec(),
        }
    }

    /// Node id of a canonical k-mer.
    #[inline]
    pub fn id_of(&self, canonical: W) -> PanResult<usize> {
        self.kmers.binary_search(&canonical).map_err(|_| {
            PanGraphXError::DeBruijn("k-mer missing from the de Bruijn node table".into())
        })
    }

    /// Builds the DTO nodes and edges, consuming the tables.
    pub fn into_dto_parts(self, k: usize) -> PanResult<(Nodes, Vec<Edge>)> {
        let overlap = edge_overlap(k)?;
        let edges = self
            .edges
            .par_iter()
            .map(|&edge| {
                let (from, to) = edge_ends(edge, k);
                Ok(Edge {
                    from_node: self.id_of(from.canonical())?,
                    from_orient: from.orientation(),
                    to_node: self.id_of(to.canonical())?,
                    to_orient: to.orientation(),
                    overlap,
                })
            })
            .collect::<PanResult<Vec<Edge>>>()?;
        let Self {
            kmers,
            edges: words,
        } = self;
        drop(words);
        let nodes = kmers
            .par_iter()
            .enumerate()
            .map(|(id, &kmer)| Node {
                sequence: decode_word(kmer, k),
                id,
            })
            .collect();
        drop(kmers);
        Ok((Nodes::from_node_vec(nodes)?, edges))
    }
}

/// Overlap of consecutive k-mers (`k - 1`) as stored on edges.
pub(crate) fn edge_overlap(k: usize) -> PanResult<u32> {
    u32::try_from(k.saturating_sub(1))
        .map_err(|_| PanGraphXError::DeBruijn(format!("k-mer size {k} too large")))
}

/// [`DbgCore`] in the word width chosen for k.
pub(crate) enum DbgTables {
    /// k <= 31: (k+1)-mers fit in a `u64`.
    Small(DbgCore<u64>),
    /// 31 < k <= 63.
    Large(DbgCore<u128>),
}

/// Runs a generic function on the [`DbgCore`] of a [`DbgTables`] value.
macro_rules! with_core {
    ($tables:expr, $core:ident => $body:expr) => {
        match $tables {
            DbgTables::Small($core) => $body,
            DbgTables::Large($core) => $body,
        }
    };
}
pub(crate) use with_core;

/// A node-centric de Bruijn graph of order k built from a variation graph.
///
/// Nodes are the distinct canonical k-mers, edges the distinct canonical
/// (k+1)-mers. Node ids follow the sorted order of the 2-bit k-mer codes, so
/// the output does not depend on the thread count.
pub struct DeBruijn {
    pub(crate) k: usize,
    pub(crate) tables: DbgTables,
}

impl DeBruijn {
    /// Builds the de Bruijn graph of the sequences spelled by the paths of
    /// `graph`.
    ///
    /// # Errors
    /// Returns an error if `k` is not in `1..=63` or the graph refers to
    /// missing nodes.
    pub fn from_directed_graph(graph: CoreGraphDTO, k: usize) -> PanResult<Self> {
        let lookup = Self::prepare(graph, k)?;
        let tables = if k <= MAX_SMALL_K {
            DbgTables::Small(build_from_paths(&lookup, k))
        } else {
            DbgTables::Large(build_from_paths(&lookup, k))
        };
        Ok(Self::finish(k, tables))
    }

    /// Builds the de Bruijn graph of every walk through `graph`, following
    /// edges in both orientations, regardless of the paths.
    ///
    /// # Errors
    /// Returns an error if `k` is not in `1..=63` or the graph refers to
    /// missing nodes.
    pub fn from_directed_graph_full_topology(graph: CoreGraphDTO, k: usize) -> PanResult<Self> {
        let lookup = Self::prepare(graph, k)?;
        let tables = if k <= MAX_SMALL_K {
            DbgTables::Small(build_from_topology(&lookup, k))
        } else {
            DbgTables::Large(build_from_topology(&lookup, k))
        };
        Ok(Self::finish(k, tables))
    }

    pub(crate) fn prepare(graph: CoreGraphDTO, k: usize) -> PanResult<CoreGraph> {
        validate_k(k)?;
        let lookup = CoreGraph::new(graph);
        lookup.validate_node_references()?;
        Ok(lookup)
    }

    pub(crate) fn finish(k: usize, tables: DbgTables) -> Self {
        let dbg = Self { k, tables };
        debug!(
            "de Bruijn graph (k={k}): {} nodes, {} edges",
            dbg.node_count(),
            dbg.edge_count()
        );
        dbg
    }

    /// The k-mer size.
    #[must_use]
    pub const fn k_size(&self) -> usize {
        self.k
    }

    /// Number of nodes (distinct canonical k-mers).
    #[must_use]
    pub const fn node_count(&self) -> usize {
        with_core!(&self.tables, core => core.kmers.len())
    }

    /// Number of edges (distinct canonical (k+1)-mers).
    #[must_use]
    pub const fn edge_count(&self) -> usize {
        with_core!(&self.tables, core => core.edges.len())
    }

    /// The canonical k-mers, indexed by node id.
    #[must_use]
    pub fn kmers(&self) -> Vec<Kmer> {
        let k = self.k;
        with_core!(&self.tables, core => core
            .kmers
            .iter()
            .map(|w| Kmer { code: w.to_u128(), k })
            .collect())
    }

    /// The edges, each listed once (in the orientation of its canonical
    /// (k+1)-mer).
    #[must_use]
    pub fn edges(&self) -> Vec<DbgEdge> {
        let k = self.k;
        with_core!(&self.tables, core => core
            .edges
            .iter()
            .map(|&e| {
                let (from, to) = edge_ends(e, k);
                DbgEdge {
                    from: OrientedKmer::from_spelled(from, k),
                    to: OrientedKmer::from_spelled(to, k),
                }
            })
            .collect())
    }

    /// Converts to a graph whose nodes are the unitigs (maximal non-branching
    /// chains of k-mers) instead of single k-mers.
    ///
    /// # Errors
    /// Returns an error if the graph is too large for 32-bit unitig ids.
    pub fn into_compacted_dto(self) -> PanResult<CoreGraphDTO> {
        let k = self.k;
        let unitigs = with_core!(self.tables, core => compaction::compact(core, k, &[])?);
        unitigs_to_dto(unitigs, k, Vec::new())
    }
}

/// Builds the DTO of a compacted graph.
pub(crate) fn unitigs_to_dto(
    unitigs: Unitigs,
    k: usize,
    paths: Vec<crate::Path>,
) -> PanResult<CoreGraphDTO> {
    let overlap = edge_overlap(k)?;
    let edges = unitigs
        .edges
        .into_iter()
        .map(|e| Edge {
            from_node: e.from.0,
            from_orient: e.from.1,
            to_node: e.to.0,
            to_orient: e.to.1,
            overlap,
        })
        .collect();
    Ok(CoreGraphDTO {
        nodes: Nodes::from_seq(unitigs.sequences),
        edges,
        paths,
        node_name_map: None,
    })
}

pub(crate) fn build_from_paths<W: KmerWord>(graph: &CoreGraph, k: usize) -> DbgCore<W> {
    let sets = DbgSets::new();
    graph.extract_path_windows(k, &sets);
    DbgCore::from_sets(sets)
}

fn build_from_topology<W: KmerWord>(graph: &CoreGraph, k: usize) -> DbgCore<W> {
    let sets = DbgSets::new();
    graph.extract_topology_windows(k, &sets);
    DbgCore::from_sets(sets)
}

impl TryFrom<DeBruijn> for CoreGraphDTO {
    type Error = PanGraphXError;

    fn try_from(db_graph: DeBruijn) -> PanResult<Self> {
        let k = db_graph.k;
        let (nodes, edges) = with_core!(db_graph.tables, core => core.into_dto_parts(k)?);
        Ok(Self {
            nodes,
            edges,
            paths: Vec::new(), // Paths are not represented in de Bruijn graph
            node_name_map: None,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::core::core_types::{Orientation, Path, Step};
    use crate::de_bruijn_conversion::k_mers::MAX_K;
    use std::collections::HashSet;

    /// Builds a DTO from node sequences, edges `(from, from_rev, to, to_rev)`
    /// and paths of `(node, reverse)` steps.
    pub fn graph(
        seqs: &[&[u8]],
        edges: &[(usize, bool, usize, bool)],
        paths: &[(&str, &[(usize, bool)])],
    ) -> CoreGraphDTO {
        CoreGraphDTO {
            nodes: Nodes::from_seq(seqs.iter().map(|s| s.to_vec()).collect()),
            edges: edges
                .iter()
                .map(|&(a, ar, b, br)| Edge {
                    from_node: a,
                    from_orient: Orientation::from(ar),
                    to_node: b,
                    to_orient: Orientation::from(br),
                    overlap: 0,
                })
                .collect(),
            paths: paths
                .iter()
                .map(|(name, steps)| Path {
                    name: name.as_bytes().to_vec(),
                    steps: steps
                        .iter()
                        .map(|&(node_id, rev)| Step::new(node_id, Orientation::from(rev)).unwrap())
                        .collect(),
                    overlaps: vec![],
                })
                .collect(),
            node_name_map: None,
        }
    }

    pub fn canon(s: &[u8]) -> String {
        String::from_utf8(Kmer::from_bases(s).canonical().to_bytes()).unwrap()
    }

    pub fn kmer_set(dbg: &DeBruijn) -> HashSet<String> {
        dbg.kmers().iter().map(ToString::to_string).collect()
    }

    /// Edges as canonical (k+1)-mer strings.
    pub fn edge_set(dbg: &DeBruijn) -> HashSet<String> {
        dbg.edges()
            .iter()
            .map(|e| {
                let mut s = e.from.to_bytes();
                s.push(*e.to.to_bytes().last().unwrap());
                canon(&s)
            })
            .collect()
    }

    pub fn strs(items: &[&[u8]]) -> HashSet<String> {
        items.iter().map(|s| canon(s)).collect()
    }

    #[test]
    fn test_k_out_of_range() {
        let g = graph(&[b"ACGT"], &[], &[("p", &[(0, false)])]);
        assert!(DeBruijn::from_directed_graph(g.clone(), 0).is_err());
        assert!(DeBruijn::from_directed_graph(g.clone(), MAX_K + 1).is_err());
        assert!(DeBruijn::from_directed_graph_full_topology(g, 0).is_err());
    }

    #[test]
    fn test_missing_node_is_error() {
        let g = graph(&[b"ACGT"], &[], &[("p", &[(3, false)])]);
        assert!(DeBruijn::from_directed_graph(g, 3).is_err());
    }

    #[test]
    fn test_single_path() {
        let g = graph(&[b"ACGTA"], &[], &[("path1", &[(0, false)])]);
        let dbg = DeBruijn::from_directed_graph(g.clone(), 4).unwrap();
        assert_eq!(kmer_set(&dbg), strs(&[b"ACGT", b"CGTA"]));
        assert_eq!(edge_set(&dbg), strs(&[b"ACGTA"]));

        // Exact size k-mer: one node, no edges.
        let dbg = DeBruijn::from_directed_graph(g, 5).unwrap();
        assert_eq!(kmer_set(&dbg), strs(&[b"ACGTA"]));
        assert_eq!(dbg.edge_count(), 0);
    }

    #[test]
    fn test_kmers_span_node_boundaries() {
        // AC + GT spells ACGT.
        let g = graph(
            &[b"AC", b"GT"],
            &[(0, false, 1, false)],
            &[("p", &[(0, false), (1, false)])],
        );
        let dbg = DeBruijn::from_directed_graph(g, 3).unwrap();
        assert_eq!(kmer_set(&dbg), strs(&[b"ACG", b"CGT"]));
        assert_eq!(edge_set(&dbg), strs(&[b"ACGT"]));
    }

    #[test]
    fn test_reverse_step_uses_reverse_complement() {
        // AAC- spells GTT, then GA: GTTGA.
        let g = graph(
            &[b"AAC", b"GA"],
            &[(0, true, 1, false)],
            &[("p", &[(0, true), (1, false)])],
        );
        let dbg = DeBruijn::from_directed_graph(g, 3).unwrap();
        assert_eq!(kmer_set(&dbg), strs(&[b"GTT", b"TTG", b"TGA"]));
        assert_eq!(edge_set(&dbg), strs(&[b"GTTG", b"TTGA"]));
    }

    #[test]
    fn test_path_overlaps_are_skipped() {
        // ACG and CGT overlap by 2: the path spells ACGT.
        let mut g = graph(&[b"ACG", b"CGT"], &[], &[("p", &[(0, false), (1, false)])]);
        g.paths[0].overlaps = vec![2];
        let dbg = DeBruijn::from_directed_graph(g, 3).unwrap();
        assert_eq!(kmer_set(&dbg), strs(&[b"ACG", b"CGT"]));
        assert_eq!(edge_set(&dbg), strs(&[b"ACGT"]));
    }

    #[test]
    fn test_no_edge_across_invalid_base() {
        let g = graph(&[b"ACGNTTA"], &[], &[("p", &[(0, false)])]);
        let dbg = DeBruijn::from_directed_graph(g, 3).unwrap();
        assert_eq!(kmer_set(&dbg), strs(&[b"ACG", b"TTA"]));
        assert_eq!(dbg.edge_count(), 0);
    }

    #[test]
    fn test_reverse_complement_twins_merge() {
        // The second path is the reverse complement of the first one.
        let g = graph(
            &[b"AACGG"],
            &[],
            &[("p1", &[(0, false)]), ("p2", &[(0, true)])],
        );
        let dbg = DeBruijn::from_directed_graph(g, 3).unwrap();
        assert_eq!(dbg.node_count(), 3);
        assert_eq!(dbg.edge_count(), 2);
    }

    #[test]
    fn test_palindromic_kmer() {
        // ACGT is its own reverse complement.
        let g = graph(&[b"AACGTT"], &[], &[("p", &[(0, false)])]);
        let dbg = DeBruijn::from_directed_graph(g, 4).unwrap();
        // AACG / CGTT are twins, ACGT is a palindrome.
        assert_eq!(kmer_set(&dbg), strs(&[b"AACG", b"ACGT"]));
        // AACGT and ACGTT are twins.
        assert_eq!(edge_set(&dbg), strs(&[b"AACGT"]));
    }

    #[test]
    fn test_large_k_uses_u128() {
        let seq: Vec<u8> = b"ACGTTGCAAGGCTTACGATCGATTTGCAGG"
            .iter()
            .cycle()
            .take(70)
            .copied()
            .collect();
        let g = graph(&[seq.as_slice()], &[], &[("p", &[(0, false)])]);
        let dbg = DeBruijn::from_directed_graph(g, 63).unwrap();
        let expected: HashSet<String> = seq.windows(63).map(canon).collect();
        assert_eq!(kmer_set(&dbg), expected);
        let expected_edges: HashSet<String> = seq.windows(64).map(canon).collect();
        assert_eq!(edge_set(&dbg), expected_edges);
    }

    #[test]
    fn test_dto_conversion() {
        let g = graph(&[b"ACGTA"], &[], &[("p", &[(0, false)])]);
        let dbg = DeBruijn::from_directed_graph(g, 4).unwrap();
        let dto = CoreGraphDTO::try_from(dbg).unwrap();
        assert_eq!(dto.nodes.len(), 2);
        assert_eq!(dto.edges.len(), 1);
        let edge = &dto.edges[0];
        assert_eq!(edge.overlap, 3);
        // The edge spells ACGTA (or its twin TACGT) along its orientations.
        let spell = |id: usize, o: Orientation| {
            let k = Kmer::from_bases(&dto.nodes[id].sequence);
            if o == Orientation::Forward {
                k
            } else {
                k.rev_comp()
            }
            .to_bytes()
        };
        let mut s = spell(edge.from_node, edge.from_orient);
        s.push(*spell(edge.to_node, edge.to_orient).last().unwrap());
        assert_eq!(canon(&s), canon(b"ACGTA"));
    }

    #[test]
    fn test_output_independent_of_thread_count() {
        let seqs: Vec<Vec<u8>> = (0..40u8)
            .map(|i| {
                b"ACGTTGCAAGGCTTACGATCG"
                    .iter()
                    .cycle()
                    .skip(usize::from(i % 7))
                    .take(9 + usize::from(i % 5))
                    .copied()
                    .collect()
            })
            .collect();
        let seq_refs: Vec<&[u8]> = seqs.iter().map(Vec::as_slice).collect();
        let steps: Vec<(usize, bool)> = (0..40).map(|i| (i, i % 3 == 0)).collect();
        let g = graph(&seq_refs, &[], &[("p", &steps)]);
        let run = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            pool.install(|| {
                CoreGraphDTO::try_from(DeBruijn::from_directed_graph(g.clone(), 7).unwrap())
                    .unwrap()
            })
        };
        let one = run(1);
        let many = run(8);
        assert_eq!(one.nodes, many.nodes);
        assert_eq!(one.edges, many.edges);
    }
    // --- Full topology ---

    fn topology(seqs: &[&[u8]], edges: &[(usize, bool, usize, bool)], k: usize) -> DeBruijn {
        DeBruijn::from_directed_graph_full_topology(graph(seqs, edges, &[]), k).unwrap()
    }

    #[test]
    fn test_linear_topology_simple() {
        // Node1[AAAA] -> Node2[TTTT], k=3: AAAATTTT.
        let dbg = topology(&[b"AAAA", b"TTTT"], &[(0, false, 1, false)], 3);
        assert_eq!(kmer_set(&dbg), strs(&[b"AAA", b"AAT", b"ATT", b"TTT"]));
        assert_eq!(
            edge_set(&dbg),
            strs(&[b"AAAA", b"AAAT", b"AATT", b"ATTT", b"TTTT"])
        );
        // AAAA/TTTT and AAAT/ATTT are twins; AATT is a palindrome.
        assert_eq!(dbg.edge_count(), 3);
    }

    #[test]
    fn test_branching_topology() {
        // Node1[AAA] -> Node2[TTT]
        //            -> Node3[GGG]
        let dbg = topology(
            &[b"AAA", b"TTT", b"GGG"],
            &[(0, false, 1, false), (0, false, 2, false)],
            2,
        );
        assert_eq!(kmer_set(&dbg), strs(&[b"AA", b"AT", b"TT", b"AG", b"GG"]));
    }

    #[test]
    fn test_merging_topology() {
        // Node1[AAA] -> Node3[TTT] <- Node2[GGG]
        let dbg = topology(
            &[b"AAA", b"GGG", b"TTT"],
            &[(0, false, 2, false), (1, false, 2, false)],
            2,
        );
        assert_eq!(kmer_set(&dbg), strs(&[b"AA", b"AT", b"TT", b"GG", b"GT"]));
    }

    #[test]
    fn test_cycle_topology() {
        // Node1[ACGT] <-> Node2[ACGT]: terminates and finds the cyclic k-mers.
        let dbg = topology(
            &[b"ACGT", b"ACGT"],
            &[(0, false, 1, false), (1, false, 0, false)],
            2,
        );
        assert_eq!(kmer_set(&dbg), strs(&[b"AC", b"CG", b"GT", b"TA"]));
        // Long walks around a short cycle stay bounded.
        let dbg = topology(
            &[b"A", b"C"],
            &[(0, false, 0, false), (0, false, 1, false)],
            31,
        );
        assert_eq!(dbg.node_count(), 2);
    }

    #[test]
    fn test_diamond_topology() {
        let dbg = topology(
            &[b"AA", b"TT", b"GG", b"CC"],
            &[
                (0, false, 1, false),
                (0, false, 2, false),
                (1, false, 3, false),
                (2, false, 3, false),
            ],
            2,
        );
        assert_eq!(
            kmer_set(&dbg),
            strs(&[b"AA", b"AT", b"TT", b"TC", b"CC", b"AG", b"GG", b"GC"])
        );
    }

    #[test]
    fn test_invalid_bases_in_topology() {
        // AANA -> TTNT: N resets the window.
        let dbg = topology(&[b"AANA", b"TTNT"], &[(0, false, 1, false)], 2);
        assert_eq!(kmer_set(&dbg), strs(&[b"AA", b"AT", b"TT"]));
        assert_eq!(edge_set(&dbg), strs(&[b"ATT"]));
    }

    #[test]
    fn test_reverse_edge_topology() {
        // AAC+ -> GGA-: AAC followed by TCC.
        let dbg = topology(&[b"AAC", b"GGA"], &[(0, false, 1, true)], 3);
        assert_eq!(kmer_set(&dbg), strs(&[b"AAC", b"ACT", b"CTC", b"TCC"]));
        assert_eq!(edge_set(&dbg), strs(&[b"AACT", b"ACTC", b"CTCC"]));
    }

    #[test]
    fn test_topology_matches_covering_path() {
        // A chain with reverse steps and overlapping edges, walked by one path
        // that uses every edge: both modes must agree.
        let seqs: &[&[u8]] = &[b"ACGGT", b"TTAGC", b"GATCA", b"CCAGT", b"AGGTC"];
        let steps: &[(usize, bool)] = &[(0, false), (1, true), (2, false), (3, true), (4, false)];
        let edges: Vec<(usize, bool, usize, bool)> = steps
            .windows(2)
            .map(|w| (w[0].0, w[0].1, w[1].0, w[1].1))
            .collect();
        let g = graph(seqs, &edges, &[("p", steps)]);
        for k in [2, 3, 5, 8, 12] {
            let paths = DeBruijn::from_directed_graph(g.clone(), k).unwrap();
            let topo = DeBruijn::from_directed_graph_full_topology(g.clone(), k).unwrap();
            assert_eq!(kmer_set(&paths), kmer_set(&topo), "k={k}");
            assert_eq!(edge_set(&paths), edge_set(&topo), "k={k}");
        }
    }
}
