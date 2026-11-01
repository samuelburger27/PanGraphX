//! FASTG tests.
//!
//! FASTG does not carry path or overlap information, so a `GFA -> FASTG -> GFA`
//! round-trip compares node names, sequences and links only
//! ([`Fidelity::FASTG`]).
mod common;

use common::{Fidelity, assert_equivalent, canonical, fixture, link, load, round_trip, to_bytes};
use pangraphx_core::*;
use rstest::rstest;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// GFA -> FASTG -> back keeps node names, sequences and (canonical) edges,
/// including links that leave a node on its reverse strand.
#[rstest]
fn test_fastg_roundtrip_from_gfa(#[files("tests/test_files/gfa/*.gfa")] path: PathBuf) {
    let original = load(&path, GraphFormat::GFA);
    let reloaded = round_trip(&original, GraphFormat::FASTG);
    assert_equivalent(
        &original,
        &reloaded,
        Fidelity::FASTG,
        &format!("GFA -> FASTG -> back, {}", path.display()),
    );
    assert_eq!(reloaded.paths(), Vec::new());
}

/// A hand-authored SPAdes-style file: both strands, multi-line sequences,
/// headers without an edge list and every edge listed from both ends.
#[test]
fn test_fastg_spades_fixture() {
    let graph = load(&fixture("fastg/assembly_graph.fastg"), GraphFormat::FASTG);
    let g = canonical(&graph, Fidelity::FASTG);

    let sequences: BTreeMap<&str, &str> = g
        .segments
        .keys()
        .map(|(name, seq)| (name.as_str(), seq.as_str()))
        .collect();
    assert_eq!(g.segments.len(), 4);
    assert_eq!(
        sequences["1"],
        "TGGCAAGGGGTCCCTAATTATGATGCGCCATGAACTCGACATCGTAGTTGAGCGCTGACCCTAGGATGAGGAGTT"
    );
    assert_eq!(sequences["2"], "GAAGCTAAAGCG");
    assert_eq!(
        sequences["3"],
        "AATTTACGACCTCGTAGACAAGCCTTACGGATCGTCTGCCGACATACTGGACATAGCCACTATA"
    );
    assert_eq!(sequences["4"], "CGCATGGCTCCAGCCGTTGACAACATATAC");

    let expected = BTreeMap::from([
        (link("1", false, "2", false, 0), 1),
        (link("1", false, "3", true, 0), 1),
        (link("2", false, "4", false, 0), 1),
        (link("3", true, "4", false, 0), 1),
    ]);
    assert_eq!(g.links, expected);
}

/// FASTG -> FASTG reproduces the same file once names are normalized.
#[test]
fn test_fastg_self_roundtrip() {
    let original = load(&fixture("fastg/assembly_graph.fastg"), GraphFormat::FASTG);
    let once = round_trip(&original, GraphFormat::FASTG);
    let twice = round_trip(&once, GraphFormat::FASTG);
    assert_equivalent(
        &original,
        &twice,
        Fidelity::FASTG,
        "FASTG -> FASTG -> FASTG",
    );
    assert_eq!(
        to_bytes(&once, GraphFormat::FASTG),
        to_bytes(&twice, GraphFormat::FASTG)
    );
}
