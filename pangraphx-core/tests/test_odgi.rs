mod common;

use common::{Fidelity, assert_equivalent, has_numeric_names, load, round_trip};
use pangraphx_core::*;
use rstest::rstest;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::NamedTempFile;

fn odgi_available() -> bool {
    Command::new("odgi").arg("version").output().is_ok()
}

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

/// Our ODGI files load in `odgi` and convert back to the same graph, and files
/// written by `odgi build` parse to the same graph. Skipped without `odgi`.
#[rstest]
fn test_odgi_cross_validation(#[files("tests/test_files/gfa/*.gfa")] path: PathBuf) {
    if !odgi_available() {
        println!("'odgi' not found on PATH; skipping ODGI cross-validation.");
        return;
    }
    let path_string = path.to_str().unwrap();
    let original = load(&path, GraphFormat::GFA);

    // pangraphx → odgi
    let og_file = NamedTempFile::new().unwrap();
    let og_path = og_file.path().to_str().unwrap();
    original.save_to_file(og_path, GraphFormat::ODGI).unwrap();
    let validate = Command::new("odgi")
        .args(["validate", "-i", og_path])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "odgi validate failed for {path_string}: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    let view = Command::new("odgi")
        .args(["view", "-g", "-i", og_path])
        .output()
        .unwrap();
    assert!(view.status.success(), "odgi view failed for {path_string}");
    let gfa_file = NamedTempFile::new().unwrap();
    fs::write(gfa_file.path(), &view.stdout).unwrap();
    let viewed = load(gfa_file.path(), GraphFormat::GFA);
    assert_equivalent(
        &original,
        &viewed,
        Fidelity::ODGI,
        &format!("odgi view of our ODGI file, {path_string}"),
    );

    // odgi → pangraphx; `odgi build` only accepts integer segment names.
    if !has_numeric_names(&original) {
        return;
    }
    let reference = NamedTempFile::new().unwrap();
    let reference_path = reference.path().to_str().unwrap();
    let build = Command::new("odgi")
        .args(["build", "-g", path_string, "-o", reference_path])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "odgi build failed for {path_string}: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let parsed = load(reference.path(), GraphFormat::ODGI);
    assert_equivalent(
        &original,
        &parsed,
        Fidelity::ODGI,
        &format!("odgi build output, {path_string}"),
    );
}
