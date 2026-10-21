//! The committed VG/`PackedGraph`/`HashGraph`/GBZ/ODGI test files: graphs written
//! by vg and odgi
//! (`utils/generate_test_files.sh`), read with our codecs and compared with
//! the GFA they were built from. Runs without the tools.
//!
//! `PANGRAPHX_REFERENCE_DIR` points the tests at freshly generated files
//! instead of the committed ones, which CI uses to detect tool drift.
mod common;

use common::{Fidelity, assert_equivalent, fixture, load};
use pangraphx_core::*;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

struct Entry {
    file: String,
    source_gfa: String,
    tool: String,
}

/// The subdirectories the manifest covers, named by file extension.
const TOOL_FORMATS: [&str; 5] = ["vg", "pg", "hg", "gbz", "og"];

fn reference_dir() -> PathBuf {
    std::env::var_os("PANGRAPHX_REFERENCE_DIR").map_or_else(|| fixture(""), PathBuf::from)
}

fn manifest() -> Vec<Entry> {
    let path = reference_dir().join("MANIFEST.tsv");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    text.lines()
        .skip(1)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            assert_eq!(fields.len(), 6, "malformed manifest row: {line}");
            Entry {
                file: fields[0].to_owned(),
                source_gfa: fields[1].to_owned(),
                tool: fields[3].to_owned(),
            }
        })
        .collect()
}

/// The format to parse a file with this extension as, and what the tool's
/// file keeps of `source`. `HashGraph` has no format of its own: the VG parser
/// recognizes it.
fn format_and_fidelity(extension: &str, source: &CoreGraphDTO) -> (GraphFormat, Fidelity) {
    match extension {
        "vg" => (GraphFormat::VG, Fidelity::written_by_vg(source)),
        "pg" => (
            GraphFormat::PackedGraph,
            Fidelity::PACKED_GRAPH.named_by_vg(source),
        ),
        "hg" => (GraphFormat::VG, Fidelity::HASH_GRAPH.named_by_vg(source)),
        "gbz" => (GraphFormat::GBZ, Fidelity::GBZ),
        "og" => (GraphFormat::ODGI, Fidelity::ODGI),
        other => panic!("no reference files with extension {other}"),
    }
}

/// Every reference file parses to the graph it was built from.
#[test]
fn test_reference_files_match_source() {
    let dir = reference_dir();
    let entries = manifest();
    assert!(!entries.is_empty(), "empty manifest in {}", dir.display());
    for entry in entries {
        let path = dir.join(&entry.file);
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let source = load(&fixture(&entry.source_gfa), GraphFormat::GFA);
        let (format, fidelity) = format_and_fidelity(extension, &source);
        let parsed = load(&path, format);
        assert_equivalent(
            &source,
            &parsed,
            fidelity,
            &format!(
                "{} (written by {}) vs {}",
                entry.file, entry.tool, entry.source_gfa
            ),
        );
    }
}

/// The manifest lists exactly the files in the `TOOL_FORMATS` directories.
#[test]
fn test_reference_manifest_is_complete() {
    let dir = reference_dir();
    let listed: BTreeSet<String> = manifest().into_iter().map(|e| e.file).collect();
    let mut present = BTreeSet::new();
    for sub in TOOL_FORMATS {
        for file in fs::read_dir(dir.join(sub)).unwrap() {
            let file = file.unwrap().path();
            let rel = file.strip_prefix(&dir).unwrap();
            present.insert(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    assert_eq!(
        listed,
        present,
        "MANIFEST.tsv and {} disagree",
        dir.display()
    );
}

/// Current vg writes `PackedGraph` into `.vg` files by default, and `HashGraph`
/// with `--hash-out`: the VG parser recognizes both and reads them like the
/// `PackedGraph` parser.
#[test]
fn test_vg_parser_reads_vg_handle_graphs() {
    let dir = reference_dir();
    for entry in manifest() {
        if !(entry.file.starts_with("pg/") || entry.file.starts_with("hg/")) {
            continue;
        }
        let path = dir.join(&entry.file);
        let via_vg = load(&path, GraphFormat::VG);
        let direct = load(&path, GraphFormat::PackedGraph);
        assert_equivalent(
            &direct,
            &via_vg,
            Fidelity::GFA,
            &format!("{} read as VG and as PackedGraph", entry.file),
        );
    }
}
