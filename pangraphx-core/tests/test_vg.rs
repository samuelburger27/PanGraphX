mod common;

use common::{Fidelity, assert_equivalent, fixture, load, round_trip};
use pangraphx_core::*;
use rstest::rstest;
use std::collections::HashSet;
use std::path::PathBuf;

/// GFA -> VG -> back keeps the whole graph.
#[rstest]
fn test_vg_roundtrip_from_gfa(#[files("tests/test_files/gfa/*.gfa")] path: PathBuf) {
    let original = load(&path, GraphFormat::GFA);
    let reloaded = round_trip(&original, GraphFormat::VG);
    assert_equivalent(
        &original,
        &reloaded,
        Fidelity::VG,
        &format!("GFA -> VG -> back, {}", path.display()),
    );
}

/// Regression: numeric and non-numeric node names must not be mapped to the
/// same VG id (`2` kept its id while `x` at index 1 also got id 2), and `0` /
/// `007` are not valid VG ids.
#[test]
fn test_vg_names_mixed_regression() {
    let original = load(&fixture("gfa/names_mixed.gfa"), GraphFormat::GFA);
    let reloaded = round_trip(&original, GraphFormat::VG);

    assert_eq!(reloaded.nodes().len(), 6);
    let steps = &reloaded.paths()[0].steps;
    let distinct: HashSet<_> = steps.iter().map(|s| s.node_id()).collect();
    assert_eq!((steps.len(), distinct.len()), (6, 6));
    assert_equivalent(&original, &reloaded, Fidelity::VG, "names_mixed via VG");
}

/// VG -> VG is stable: a second write/read gives the same graph as the first.
#[rstest]
fn test_vg_self_roundtrip(#[files("tests/test_files/gfa/*.gfa")] path: PathBuf) {
    let original = load(&path, GraphFormat::GFA);
    let once = round_trip(&original, GraphFormat::VG);
    let twice = round_trip(&once, GraphFormat::VG);
    assert_equivalent(
        &once,
        &twice,
        Fidelity::VG,
        &format!("VG -> VG, {}", path.display()),
    );
}

/// Cross-format test: GFA -> VG -> GFA and compare.
#[rstest]
fn test_vg_to_gfa_conversion(#[files("tests/test_files/gfa/*.gfa")] path: PathBuf) {
    let original = load(&path, GraphFormat::GFA);
    let converted = round_trip(&round_trip(&original, GraphFormat::VG), GraphFormat::GFA);
    assert_equivalent(
        &original,
        &converted,
        Fidelity::VG,
        &format!("GFA -> VG -> GFA, {}", path.display()),
    );
}
