mod common;

use common::{load, round_trip};
use pangraphx_core::*;
use rstest::rstest;
use std::path::PathBuf;

/// GFA -> GFA reproduces the parsed graph exactly.
#[rstest]
fn test_gfa_format(#[files("tests/test_files/gfa/*.gfa")] path: PathBuf) {
    let graph = load(&path, GraphFormat::GFA);
    let reloaded = round_trip(&graph, GraphFormat::GFA);
    assert_eq!(
        graph,
        reloaded,
        "Graphs do not match after save/load cycle for {}",
        path.display()
    );
}
