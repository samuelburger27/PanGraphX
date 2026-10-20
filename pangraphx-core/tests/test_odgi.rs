mod common;

use common::{Fidelity, assert_equivalent, load, round_trip};
use pangraphx_core::*;
use rstest::rstest;
use std::path::PathBuf;

/// GFA -> ODGI -> back keeps the graph up to overlaps and node renumbering.
#[rstest]
fn test_odgi_roundtrip_from_gfa(#[files("tests/test_files/gfa/*.gfa")] path: PathBuf) {
    let original = load(&path, GraphFormat::GFA);
    let reloaded = round_trip(&original, GraphFormat::ODGI);
    assert_equivalent(
        &original,
        &reloaded,
        Fidelity::ODGI,
        &format!("GFA -> ODGI -> back, {}", path.display()),
    );
}
