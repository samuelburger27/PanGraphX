//! Cross-validation against the reference tools, in both directions:
//!
//! - **A, our writer → their reader**: we write VG/GBZ/ODGI, the tool
//!   validates the file and converts it to GFA, and that GFA must match the
//!   source graph.
//! - **B, their writer → our reader**: the tool builds VG/GBZ/ODGI from the
//!   source GFA, and our parse of it must match the source graph.
//!
//! Needs `vg` and `odgi` (see `common::tools`); without them each test is
//! skipped with a `SKIPPED` line, or fails under `PANGRAPHX_REQUIRE_EXTERNAL=1`.
//! `random_medium.gfa` is left out to keep the run short.
mod common;

use common::tools::{notice, require_tool};
use common::{Fidelity, assert_equivalent, from_bytes, has_numeric_names, load, stem, to_bytes};
use pangraphx_core::*;
use rstest::rstest;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Writes `graph` as `format` into `dir` and returns the file path.
fn write(dir: &TempDir, graph: &CoreGraphDTO, format: GraphFormat) -> PathBuf {
    let path = dir.path().join(format!("graph.{}", format.get_extension()));
    fs::write(&path, to_bytes(graph, format)).unwrap();
    path
}

/// The fixture has overlaps, which `vg convert` rejects.
fn has_overlaps(graph: &CoreGraphDTO) -> bool {
    graph.edges.iter().any(|e| e.overlap != 0)
}

/// The fixture has GFA 1.1 walks (`W` lines).
fn has_walks(gfa: &Path) -> bool {
    fs::read_to_string(gfa)
        .unwrap()
        .lines()
        .any(|line| line.starts_with("W\t"))
}

fn context(check: &str, gfa: &Path) -> String {
    format!("{check}, {}", stem(gfa))
}

/// A: `vg validate` accepts our VG file and `vg view` shows the source graph.
#[rstest]
fn vg_reads_our_vg(
    #[files("tests/test_files/gfa/*.gfa")]
    #[exclude("random_medium")]
    gfa: PathBuf,
) {
    let Some(vg) = require_tool("vg", "vg_reads_our_vg") else {
        return;
    };
    let source = load(&gfa, GraphFormat::GFA);
    let dir = TempDir::new().unwrap();
    let file = write(&dir, &source, GraphFormat::VG);
    vg.run(|c| c.arg("validate").arg(&file));
    let viewed = from_bytes(vg.run(|c| c.arg("view").arg(&file)), GraphFormat::GFA);
    assert_equivalent(
        &source,
        &viewed,
        Fidelity::VG_VIEW,
        &context("vg view of our VG", &gfa),
    );
}

/// B: we read the VG file that `vg convert` builds from the GFA.
#[rstest]
fn we_read_vg_vg(
    #[files("tests/test_files/gfa/*.gfa")]
    #[exclude("random_medium")]
    gfa: PathBuf,
) {
    let Some(vg) = require_tool("vg", "we_read_vg_vg") else {
        return;
    };
    let source = load(&gfa, GraphFormat::GFA);
    if has_overlaps(&source) {
        notice(&format!(
            "not applicable for {}: vg convert rejects overlaps",
            stem(&gfa)
        ));
        return;
    }
    let bytes = vg.run(|c| c.args(["convert", "--gfa-in", "--vg-out"]).arg(&gfa));
    let parsed = from_bytes(bytes, GraphFormat::VG);
    assert_equivalent(
        &source,
        &parsed,
        Fidelity::written_by_vg(&source),
        &context("our parse of vg convert output", &gfa),
    );
}

/// A: `vg convert -f` turns our GBZ file back into the source graph.
#[rstest]
fn vg_reads_our_gbz(
    #[files("tests/test_files/gfa/*.gfa")]
    #[exclude("random_medium")]
    gfa: PathBuf,
) {
    let Some(vg) = require_tool("vg", "vg_reads_our_gbz") else {
        return;
    };
    let source = load(&gfa, GraphFormat::GFA);
    let dir = TempDir::new().unwrap();
    let file = write(&dir, &source, GraphFormat::GBZ);
    let bytes = vg.run(|c| c.args(["convert", "-f"]).arg(&file));
    let viewed = from_bytes(bytes, GraphFormat::GFA);
    assert_equivalent(
        &source,
        &viewed,
        Fidelity::GBZ,
        &context("vg convert -f of our GBZ", &gfa),
    );
}

/// B: we read the GBZ file that `vg gbwt` builds from the GFA.
#[rstest]
fn we_read_vg_gbz(
    #[files("tests/test_files/gfa/*.gfa")]
    #[exclude("random_medium")]
    gfa: PathBuf,
) {
    let Some(vg) = require_tool("vg", "we_read_vg_gbz") else {
        return;
    };
    if has_walks(&gfa) {
        notice(&format!(
            "not applicable for {}: vg gbwt keeps only the start of walk subranges",
            stem(&gfa)
        ));
        return;
    }
    let source = load(&gfa, GraphFormat::GFA);
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("graph.gbz");
    // `--max-node 0` keeps nodes unchopped, as our GBZ writer does.
    vg.run(|c| {
        c.arg("gbwt")
            .arg("-G")
            .arg(&gfa)
            .args(["--max-node", "0", "--gbz-format", "-g"])
            .arg(&file)
    });
    let parsed = load(&file, GraphFormat::GBZ);
    assert_equivalent(
        &source,
        &parsed,
        Fidelity::GBZ,
        &context("our parse of vg gbwt output", &gfa),
    );
}

/// A: `odgi validate` accepts our ODGI file and `odgi view` shows the source
/// graph.
#[rstest]
fn odgi_reads_our_og(
    #[files("tests/test_files/gfa/*.gfa")]
    #[exclude("random_medium")]
    gfa: PathBuf,
) {
    let Some(odgi) = require_tool("odgi", "odgi_reads_our_og") else {
        return;
    };
    let source = load(&gfa, GraphFormat::GFA);
    let dir = TempDir::new().unwrap();
    let file = write(&dir, &source, GraphFormat::ODGI);
    odgi.run(|c| c.args(["validate", "-i"]).arg(&file));
    let bytes = odgi.run(|c| c.args(["view", "-g", "-i"]).arg(&file));
    let viewed = from_bytes(bytes, GraphFormat::GFA);
    assert_equivalent(
        &source,
        &viewed,
        Fidelity::ODGI,
        &context("odgi view of our ODGI", &gfa),
    );
}

/// B: we read the ODGI file that `odgi build` builds from the GFA.
#[rstest]
fn we_read_odgi_og(
    #[files("tests/test_files/gfa/*.gfa")]
    #[exclude("random_medium")]
    gfa: PathBuf,
) {
    let Some(odgi) = require_tool("odgi", "we_read_odgi_og") else {
        return;
    };
    let source = load(&gfa, GraphFormat::GFA);
    if !has_numeric_names(&source) {
        notice(&format!(
            "not applicable for {}: odgi build needs integer segment names",
            stem(&gfa)
        ));
        return;
    }
    if has_walks(&gfa) {
        notice(&format!(
            "not applicable for {}: odgi build ignores W lines",
            stem(&gfa)
        ));
        return;
    }
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("graph.og");
    odgi.run(|c| c.args(["build", "-g"]).arg(&gfa).arg("-o").arg(&file));
    let parsed = load(&file, GraphFormat::ODGI);
    assert_equivalent(
        &source,
        &parsed,
        Fidelity::ODGI,
        &context("our parse of odgi build output", &gfa),
    );
}
