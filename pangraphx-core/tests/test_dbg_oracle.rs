//! De Bruijn construction checked against a naive oracle.
//!
//! The oracle works on plain byte strings: it spells the input, slides
//! windows over it and keeps the canonical (`min(s, revcomp(s))`) k-mers and
//! (k+1)-mers. It shares no code with `de_bruijn_conversion`, so the 2-bit
//! words, rolling windows, `u64`/`u128` switch (k = 31/32) and the parallel
//! k-mer sets are all checked against it.
mod common;

use common::{fixture, load};
use pangraphx_core::core::core_types::Nodes;
use pangraphx_core::*;
use rstest::rstest;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

type Seq = Vec<u8>;
/// A node in one orientation: `(node, reverse)`.
type Side = (NodeId, bool);

const fn is_base(b: u8) -> bool {
    matches!(b, b'A' | b'C' | b'G' | b'T')
}

fn revcomp(s: &[u8]) -> Seq {
    s.iter()
        .rev()
        .map(|&b| match b {
            b'A' => b'T',
            b'C' => b'G',
            b'G' => b'C',
            b'T' => b'A',
            other => other,
        })
        .collect()
}

fn canon(s: &[u8]) -> Seq {
    let rc = revcomp(s);
    if rc.as_slice() < s { rc } else { s.to_vec() }
}

/// The node's sequence, upper-case, in orientation `rev`.
fn oriented(g: &CoreGraphDTO, node: NodeId, rev: bool) -> Seq {
    let seq = g.nodes[node].sequence.to_ascii_uppercase();
    if rev { revcomp(&seq) } else { seq }
}

/// Canonical k-mers and (k+1)-mers, the nodes and edges of the graph.
#[derive(Debug, Default, PartialEq, Eq)]
struct KmerSets {
    kmers: BTreeSet<Seq>,
    edges: BTreeSet<Seq>,
}

impl KmerSets {
    /// Adds the windows of length k and k+1 at the start of `walk`.
    fn add_prefix(&mut self, walk: &[u8], k: usize) {
        for (len, set) in [(k, &mut self.kmers), (k + 1, &mut self.edges)] {
            if let Some(window) = walk.get(..len)
                && window.iter().all(|&b| is_base(b))
            {
                set.insert(canon(window));
            }
        }
    }

    /// Adds every window of `seq`.
    fn add_all(&mut self, seq: &[u8], k: usize) {
        for start in 0..seq.len() {
            self.add_prefix(&seq[start..], k);
        }
    }
}

/// The path's sequence: oriented steps, each without the overlap it shares
/// with the previous step.
fn spell(g: &CoreGraphDTO, path: &Path) -> Seq {
    let mut out = Seq::new();
    for (i, step) in path.steps.iter().enumerate() {
        let skip = i
            .checked_sub(1)
            .and_then(|prev| path.overlaps.get(prev))
            .map_or(0, |&o| o as usize);
        let seq = oriented(g, step.node_id(), step.is_reverse());
        out.extend_from_slice(&seq[skip.min(seq.len())..]);
    }
    out
}

/// The non-nucleotide-free pieces of the path's sequence that hold a k-mer.
fn pieces(g: &CoreGraphDTO, path: &Path, k: usize) -> Vec<Seq> {
    spell(g, path)
        .split(|&b| !is_base(b))
        .filter(|piece| piece.len() >= k)
        .map(<[u8]>::to_vec)
        .collect()
}

/// Path mode: the windows of every path's sequence.
fn path_oracle(g: &CoreGraphDTO, k: usize) -> KmerSets {
    let mut sets = KmerSets::default();
    for path in &g.paths {
        sets.add_all(&spell(g, path), k);
    }
    sets
}

/// Topology mode: every window that starts in an oriented node, following
/// all walks through the bidirected graph.
fn topology_oracle(g: &CoreGraphDTO, k: usize) -> KmerSets {
    let mut successors: HashMap<Side, Vec<(Side, usize)>> = HashMap::new();
    for e in &g.edges {
        let (a, ar) = (e.from_node, e.from_orient == Orientation::Reverse);
        let (b, br) = (e.to_node, e.to_orient == Orientation::Reverse);
        let overlap = e.overlap as usize;
        successors
            .entry((a, ar))
            .or_default()
            .push(((b, br), overlap));
        successors
            .entry((b, !br))
            .or_default()
            .push(((a, !ar), overlap));
    }

    let mut sets = KmerSets::default();
    let mut seen = HashSet::new();
    for node in 0..g.nodes.len() {
        for rev in [false, true] {
            let seq = oriented(g, node, rev);
            for start in 0..seq.len() {
                // Depth-first over walks, cut at k+1 bases.
                let mut stack = vec![((node, rev), seq[start..].to_vec())];
                while let Some((side, mut walk)) = stack.pop() {
                    walk.truncate(k + 1);
                    if !seen.insert((side, walk.clone())) {
                        continue;
                    }
                    sets.add_prefix(&walk, k);
                    if walk.len() > k || walk.iter().any(|&b| !is_base(b)) {
                        continue;
                    }
                    for &((next, next_rev), overlap) in successors.get(&side).into_iter().flatten()
                    {
                        let bases = oriented(g, next, next_rev);
                        let mut longer = walk.clone();
                        longer.extend_from_slice(&bases[overlap.min(bases.len())..]);
                        stack.push(((next, next_rev), longer));
                    }
                }
            }
        }
        seen.clear();
    }
    sets
}

/// What the de Bruijn graph holds, as canonical strings.
fn dbg_sets(dbg: &DeBruijn) -> KmerSets {
    let kmers: BTreeSet<Seq> = dbg.kmers().iter().map(Kmer::to_bytes).collect();
    let edges = dbg
        .edges()
        .iter()
        .map(|e| {
            let (from, to) = (e.from.to_bytes(), e.to.to_bytes());
            assert_eq!(
                from[1..],
                to[..to.len() - 1],
                "edge ends do not overlap by k-1"
            );
            let mut edge = from;
            edge.push(*to.last().unwrap());
            canon(&edge)
        })
        .collect();
    KmerSets { kmers, edges }
}

fn show(s: &[u8]) -> String {
    String::from_utf8_lossy(s).into_owned()
}

/// Asserts set equality, listing a few differences.
fn assert_sets(expected: &BTreeSet<Seq>, actual: &BTreeSet<Seq>, what: &str, context: &str) {
    if expected == actual {
        return;
    }
    let missing: Vec<String> = expected
        .difference(actual)
        .take(5)
        .map(|s| show(s))
        .collect();
    let extra: Vec<String> = actual
        .difference(expected)
        .take(5)
        .map(|s| show(s))
        .collect();
    panic!(
        "{context}: {what} differ ({} expected, {} built)\n  missing: {missing:?}\n  extra: {extra:?}",
        expected.len(),
        actual.len()
    );
}

/// Checks the compacted graph: its unitigs hold every k-mer exactly once,
/// and no unitig could be extended into a neighbour.
fn check_compaction(dto: &CoreGraphDTO, expected: &KmerSets, k: usize, context: &str) {
    let mut seen: BTreeMap<Seq, usize> = BTreeMap::new();
    for node in dto.nodes.iter() {
        assert!(node.sequence.len() >= k, "{context}: unitig shorter than k");
        for window in node.sequence.windows(k) {
            *seen.entry(canon(window)).or_default() += 1;
        }
    }
    let repeated: Vec<String> = seen
        .iter()
        .filter(|&(_, &n)| n > 1)
        .take(5)
        .map(|(s, _)| show(s))
        .collect();
    assert!(
        repeated.is_empty(),
        "{context}: k-mers in several unitigs: {repeated:?}"
    );
    let unitig_kmers: BTreeSet<Seq> = seen.into_keys().collect();
    assert_sets(&expected.kmers, &unitig_kmers, "unitig k-mers", context);

    let successors = |kmer: &[u8]| -> Vec<Seq> {
        b"ACGT"
            .iter()
            .filter_map(|&b| {
                let mut edge = kmer.to_vec();
                edge.push(b);
                expected
                    .edges
                    .contains(&canon(&edge))
                    .then(|| edge[1..].to_vec())
            })
            .collect()
    };
    let predecessors = |kmer: &[u8]| successors(&revcomp(kmer));
    for node in dto.nodes.iter() {
        let unitig = &node.sequence;
        let own: BTreeSet<Seq> = unitig.windows(k).map(canon).collect();
        for strand in [unitig.clone(), revcomp(unitig)] {
            let end = &strand[strand.len() - k..];
            let next = successors(end);
            let [next] = next.as_slice() else {
                continue;
            };
            // Compaction never merges through a palindromic k-mer (its own
            // reverse complement), as in BCALM and other compactors.
            let palindrome = |s: &[u8]| revcomp(s) == s;
            let merges = predecessors(next).len() == 1
                && !own.contains(&canon(next))
                && !palindrome(end)
                && !palindrome(next);
            assert!(
                !merges,
                "{context}: unitig {} could be extended by {}",
                show(&strand),
                show(next)
            );
        }
    }
}

/// Checks the colored compacted graph: every gap-free piece of a path that
/// holds a k-mer becomes one path (`name`, or `name.1`.. when the path splits)
/// that walks unitigs spelling exactly that piece.
fn check_colored(g: &CoreGraphDTO, k: usize, context: &str) {
    let dto = ColoredDBG::from_directed_graph(g.clone(), k)
        .unwrap()
        .into_compacted_dto()
        .unwrap();
    let mut expected = BTreeMap::new();
    for path in &g.paths {
        let name = show(&path.name);
        let pieces = pieces(g, path, k);
        if let [piece] = pieces.as_slice() {
            expected.insert(name, piece.clone());
        } else {
            for (i, piece) in pieces.into_iter().enumerate() {
                expected.insert(format!("{name}.{}", i + 1), piece);
            }
        }
    }
    let actual: BTreeMap<String, Seq> = dto
        .paths
        .iter()
        .map(|p| {
            let mut spelled = Seq::new();
            for (i, step) in p.steps.iter().enumerate() {
                let seq = oriented(&dto, step.node_id(), step.is_reverse());
                spelled.extend_from_slice(&seq[if i == 0 { 0 } else { k - 1 }..]);
            }
            (show(&p.name), spelled)
        })
        .collect();
    let readable = |m: &BTreeMap<String, Seq>| {
        m.iter()
            .map(|(n, s)| format!("{n}: {}", show(s)))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        readable(&expected),
        readable(&actual),
        "{context}: colored paths"
    );
}

fn check(g: &CoreGraphDTO, k: usize, topology: bool, context: &str) {
    let (dbg, expected) = if topology {
        (
            DeBruijn::from_directed_graph_full_topology(g.clone(), k).unwrap(),
            topology_oracle(g, k),
        )
    } else {
        (
            DeBruijn::from_directed_graph(g.clone(), k).unwrap(),
            path_oracle(g, k),
        )
    };
    let context = format!(
        "{context}, k={k}, {}",
        if topology { "topology" } else { "paths" }
    );
    assert_eq!(dbg.k_size(), k);
    assert_eq!(
        dbg.node_count(),
        expected.kmers.len(),
        "{context}: node count"
    );
    assert_eq!(
        dbg.edge_count(),
        expected.edges.len(),
        "{context}: edge count"
    );
    let built = dbg_sets(&dbg);
    assert_sets(&expected.kmers, &built.kmers, "k-mers", &context);
    assert_sets(&expected.edges, &built.edges, "edges", &context);

    check_compaction(&dbg.into_compacted_dto().unwrap(), &expected, k, &context);
    if !topology {
        check_colored(g, k, &context);
    }
}

#[rstest]
fn test_dbg_matches_oracle(
    #[values("tiny", "names_mixed", "random", "phix", "escherichia_phage_lambda")] name: &str,
    #[values(3, 5, 21, 31, 32, 33, 63)] k: usize,
    #[values(false, true)] topology: bool,
) {
    let g = load(&fixture(&format!("gfa/{name}.gfa")), GraphFormat::GFA);
    check(&g, k, topology, name);
}

/// A larger genome, around the `u64`/`u128` switch only.
#[rstest]
fn test_dbg_matches_oracle_large(#[values(31, 32)] k: usize) {
    let g = load(&fixture("gfa/mycoplasma_genitalium.gfa"), GraphFormat::GFA);
    check(&g, k, false, "mycoplasma_genitalium");
}

/// Lower-case bases, `N`, a palindrome (`ACGT`), reverse steps, a path
/// overlap and a path broken by `N` into two pieces.
fn edge_case_graph() -> CoreGraphDTO {
    use Orientation::{Forward, Reverse};
    let edge = |from, from_orient, to, to_orient, overlap| Edge {
        from_node: from,
        from_orient,
        to_node: to,
        to_orient,
        overlap,
    };
    let step = |id, o| Step::new(id, o).unwrap();
    CoreGraphDTO {
        nodes: Nodes::from_seq(vec![
            b"acgtACGTac".to_vec(),
            b"GATTACA".to_vec(),
            b"CCNNGGTTA".to_vec(),
            b"TTAGCATG".to_vec(),
        ]),
        edges: vec![
            edge(0, Forward, 1, Reverse, 0),
            edge(1, Reverse, 2, Forward, 0),
            edge(2, Forward, 3, Forward, 3),
            edge(3, Forward, 0, Reverse, 0),
        ],
        paths: vec![
            Path {
                name: b"main".to_vec(),
                steps: vec![
                    step(0, Forward),
                    step(1, Reverse),
                    step(2, Forward),
                    step(3, Forward),
                ],
                overlaps: vec![0, 0, 3],
            },
            Path {
                name: b"back".to_vec(),
                steps: vec![step(3, Forward), step(0, Reverse)],
                overlaps: vec![],
            },
        ],
        node_name_map: None,
    }
}

#[rstest]
fn test_dbg_edge_cases(
    #[values(1, 2, 3, 4, 5, 8, 31, 32)] k: usize,
    #[values(false, true)] topology: bool,
) {
    check(&edge_case_graph(), k, topology, "edge cases");
}
