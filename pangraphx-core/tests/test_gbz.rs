//! GBZ is a path-first format: it stores node sequences and paths and derives
//! the edges from them. A round-trip therefore keeps only the links that some
//! path traverses, without overlaps or repeats ([`Fidelity::GBZ`]).
mod common;

use common::{Fidelity, assert_equivalent, load, round_trip};
use pangraphx_core::*;
use rstest::rstest;
use std::path::PathBuf;

/// GFA -> GBZ -> back keeps nodes, paths and the path-covered links.
#[rstest]
fn test_gbz_roundtrip_from_gfa(#[files("tests/test_files/gfa/*.gfa")] path: PathBuf) {
    let original = load(&path, GraphFormat::GFA);
    let reloaded = round_trip(&original, GraphFormat::GBZ);
    assert_equivalent(
        &original,
        &reloaded,
        Fidelity::GBZ,
        &format!("GFA -> GBZ -> back, {}", path.display()),
    );
}
