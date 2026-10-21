mod common;

use common::{Fidelity, assert_equivalent, from_bytes, load, round_trip, to_bytes};
use pangraphx_core::*;
use rstest::rstest;
use std::path::PathBuf;

/// GFA -> `PackedGraph` -> back keeps the graph up to overlaps and renumbering.
#[rstest]
fn test_packed_graph_roundtrip_from_gfa(#[files("tests/test_files/gfa/*.gfa")] path: PathBuf) {
    let original = load(&path, GraphFormat::GFA);
    let reloaded = round_trip(&original, GraphFormat::PackedGraph);
    assert_equivalent(
        &original,
        &reloaded,
        Fidelity::PACKED_GRAPH,
        &format!("GFA -> PackedGraph -> back, {}", path.display()),
    );
}

/// `PackedGraph` -> `PackedGraph` is stable.
#[rstest]
fn test_packed_graph_self_roundtrip(#[files("tests/test_files/gfa/*.gfa")] path: PathBuf) {
    let once = round_trip(&load(&path, GraphFormat::GFA), GraphFormat::PackedGraph);
    let twice = round_trip(&once, GraphFormat::PackedGraph);
    assert_equivalent(
        &once,
        &twice,
        Fidelity::PACKED_GRAPH,
        &format!("PackedGraph -> PackedGraph, {}", path.display()),
    );
}

/// A `.vg` file may hold `PackedGraph`, as current vg writes it by default: the
/// VG parser reads it like the `PackedGraph` parser.
#[rstest]
fn test_vg_parser_reads_packed_graph(#[files("tests/test_files/gfa/*.gfa")] path: PathBuf) {
    let original = load(&path, GraphFormat::GFA);
    let bytes = to_bytes(&original, GraphFormat::PackedGraph);
    let via_vg = from_bytes(bytes.clone(), GraphFormat::VG);
    let direct = from_bytes(bytes, GraphFormat::PackedGraph);
    assert_equivalent(
        &direct,
        &via_vg,
        Fidelity::GFA,
        &format!("PackedGraph read as VG, {}", path.display()),
    );
}

/// Bases other than ACGT become `N`, lowercase becomes uppercase.
#[test]
fn test_packed_graph_sequence_alphabet() {
    let gfa = "H\tVN:Z:1.0\nS\t1\tacgtRYKM\nS\t2\tNNac\nL\t1\t+\t2\t-\t0M\nP\tp\t1+,2-\t*\n";
    let original = from_bytes(gfa.as_bytes().to_vec(), GraphFormat::GFA);
    let reloaded = round_trip(&original, GraphFormat::PackedGraph);
    let sequences: Vec<&[u8]> = reloaded
        .nodes
        .iter()
        .map(|n| n.sequence.as_slice())
        .collect();
    assert_eq!(sequences, [b"ACGTNNNN".as_slice(), b"NNAC".as_slice()]);
    assert_equivalent(
        &original,
        &reloaded,
        Fidelity::PACKED_GRAPH,
        "IUPAC bases via PackedGraph",
    );
}
