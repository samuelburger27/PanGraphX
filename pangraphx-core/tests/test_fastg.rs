//! FASTG tests.
//!
//! FASTG does not carry path or overlap information, so a `GFA -> FASTG -> GFA`
//! round-trip is inherently lossy. These tests therefore compare node names,
//! sequences and edges only, not `CoreGraphDTO::isomorphic`. Edges are compared
//! in canonical form, since `a -> b` and its mirror `b' -> a'` are one edge.
use pangraphx_core::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use tempfile::NamedTempFile;

type EdgeKey = (String, bool, String, bool);

/// The edges of `graph` by node name, each in canonical orientation.
fn edge_set(graph: &CoreGraphDTO) -> HashSet<EdgeKey> {
    graph
        .edges
        .iter()
        .map(|e| {
            let from = graph.get_name_from_id(e.from_node);
            let to = graph.get_name_from_id(e.to_node);
            let from_rev = e.from_orient == Orientation::Reverse;
            let to_rev = e.to_orient == Orientation::Reverse;
            let forward = (from.clone(), from_rev, to.clone(), to_rev);
            let mirror = (to, !to_rev, from, !from_rev);
            forward.min(mirror)
        })
        .collect()
}

fn sequences_by_name(graph: &CoreGraphDTO) -> HashMap<String, Vec<u8>> {
    graph
        .nodes
        .iter()
        .map(|node| (graph.get_node_name(node), node.sequence.clone()))
        .collect()
}

fn round_trip(graph: &CoreGraphDTO) -> CoreGraphDTO {
    let file = NamedTempFile::new().unwrap();
    let path = file.path().to_str().unwrap();
    graph.save_to_file(path, GraphFormat::FASTG).unwrap();
    CoreGraphDTO::load_from_file(path, GraphFormat::FASTG).unwrap()
}

/// GFA -> FASTG -> back keeps node names, sequences and (canonical) edges,
/// including links that leave a node on its reverse strand.
#[test]
fn test_fastg_roundtrip_from_gfa() {
    for entry in fs::read_dir("tests/test_files/gfa").unwrap() {
        let path = entry.unwrap().path();
        let path_string = path.to_str().unwrap();
        let original = CoreGraphDTO::load_from_file(path_string, GraphFormat::GFA).unwrap();
        let reloaded = round_trip(&original);

        assert_eq!(
            sequences_by_name(&original),
            sequences_by_name(&reloaded),
            "Nodes differ after FASTG round-trip for {path_string}"
        );
        let edges = edge_set(&original);
        assert_eq!(
            edges,
            edge_set(&reloaded),
            "Edges differ after FASTG round-trip for {path_string}"
        );
        assert_eq!(
            edges.len(),
            reloaded.edges.len(),
            "Duplicate edges after FASTG round-trip for {path_string}"
        );
        assert_eq!(reloaded.paths, Vec::new());
    }
}

/// A hand-authored SPAdes-style file: both strands, multi-line sequences,
/// headers without an edge list and every edge listed from both ends.
#[test]
fn test_fastg_spades_fixture() {
    let graph = CoreGraphDTO::load_from_file(
        "tests/test_files/fastg/assembly_graph.fastg",
        GraphFormat::FASTG,
    )
    .unwrap();

    let sequences = sequences_by_name(&graph);
    assert_eq!(sequences.len(), 4);
    assert_eq!(
        sequences["1"],
        b"TGGCAAGGGGTCCCTAATTATGATGCGCCATGAACTCGACATCGTAGTTGAGCGCTGACCCTAGGATGAGGAGTT"
    );
    assert_eq!(sequences["2"], b"GAAGCTAAAGCG");
    assert_eq!(
        sequences["3"],
        b"AATTTACGACCTCGTAGACAAGCCTTACGGATCGTCTGCCGACATACTGGACATAGCCACTATA"
    );
    assert_eq!(sequences["4"], b"CGCATGGCTCCAGCCGTTGACAACATATAC");

    let edge = |from: &str, from_rev, to: &str, to_rev| -> EdgeKey {
        (from.to_string(), from_rev, to.to_string(), to_rev)
    };
    let expected: HashSet<EdgeKey> = [
        edge("1", false, "2", false),
        edge("1", false, "3", true),
        edge("2", false, "4", false),
        edge("3", true, "4", false),
    ]
    .into_iter()
    .map(|(a, ar, b, br)| (a.clone(), ar, b.clone(), br).min((b, !br, a, !ar)))
    .collect();
    assert_eq!(edge_set(&graph), expected);
    assert_eq!(graph.edges.len(), 4);
}

/// FASTG -> FASTG reproduces the same file once names are normalized.
#[test]
fn test_fastg_self_roundtrip() {
    let original = CoreGraphDTO::load_from_file(
        "tests/test_files/fastg/assembly_graph.fastg",
        GraphFormat::FASTG,
    )
    .unwrap();
    let once = round_trip(&original);
    let twice = round_trip(&once);
    assert_eq!(sequences_by_name(&original), sequences_by_name(&twice));
    assert_eq!(edge_set(&original), edge_set(&twice));

    let mut first = Vec::new();
    let mut second = Vec::new();
    once.save(&mut first, GraphFormat::FASTG).unwrap();
    twice.save(&mut second, GraphFormat::FASTG).unwrap();
    assert_eq!(first, second);
}
