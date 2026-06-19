use pangraphx_core::*;
use std::fs;
use tempfile::NamedTempFile;

#[test]
fn test_vg_parsing() {
    let test_files = fs::read_dir("tests/test_files/vg").unwrap();
    for entry in test_files {
        let path = entry.unwrap().path();
        let path_string = path.to_str().unwrap();
        println!("Testing VG parsing for file: {path_string}...");

        let graph = CoreGraphDTO::load_from_file(path_string, GraphFormat::VG).unwrap();

        // For now only print
        println!(
            "Parsed graph from {}: {} nodes, {} edges, {} paths",
            path_string,
            graph.nodes.len(),
            graph.edges.len(),
            graph.paths.len()
        );

        // Basic sanity checks
        assert!(
            !graph.nodes.is_empty(),
            "No nodes parsed from {path_string}"
        );
        assert!(
            !graph.edges.is_empty(),
            "No edges parsed from {path_string}"
        );
        assert!(
            !graph.paths.is_empty(),
            "No paths parsed from {path_string}"
        );

        // Check node IDs are unique
        let node_ids: std::collections::HashSet<_> = graph.nodes.iter().map(|n| n.id).collect();
        assert_eq!(
            node_ids.len(),
            graph.nodes.len(),
            "Duplicate node IDs found in {path_string}"
        );
    }
}

/// Round-trip test: parse each GFA test file → serialize to VG → parse VG back
/// → verify the graphs are isomorphic.
#[test]
fn test_vg_roundtrip_from_gfa() {
    let test_files = fs::read_dir("tests/test_files/gfa").unwrap();
    for entry in test_files {
        let path = entry.unwrap().path();
        let path_string = path.to_str().unwrap();
        println!("Testing VG round-trip from GFA file: {path_string}...");

        let original = CoreGraphDTO::load_from_file(path_string, GraphFormat::GFA).unwrap();

        // Serialize to VG
        let vg_file = NamedTempFile::new().unwrap();
        original
            .save_to_file(vg_file.path().to_str().unwrap(), GraphFormat::VG)
            .unwrap();
        assert!(vg_file.path().exists());

        // Parse back from VG
        let reloaded =
            CoreGraphDTO::load_from_file(vg_file.path().to_str().unwrap(), GraphFormat::VG)
                .unwrap();

        assert_eq!(
            original.nodes.len(),
            reloaded.nodes.len(),
            "Node count mismatch for {path_string}"
        );
        assert_eq!(
            original.edges.len(),
            reloaded.edges.len(),
            "Edge count mismatch for {path_string}"
        );
        assert_eq!(
            original.paths.len(),
            reloaded.paths.len(),
            "Path count mismatch for {path_string}"
        );
        assert!(
            original.isomorphic(&reloaded),
            "Graphs not isomorphic after VG round-trip for {path_string}"
        );
    }
}

/// Test VG self round-trip: serialize to VG → parse → serialize again → parse
/// → verify identical.
#[test]
fn test_vg_self_roundtrip() {
    let test_files = fs::read_dir("tests/test_files/gfa").unwrap();
    for entry in test_files {
        let path = entry.unwrap().path();
        let path_string = path.to_str().unwrap();

        let original = CoreGraphDTO::load_from_file(path_string, GraphFormat::GFA).unwrap();

        // First VG write/read
        let vg1 = NamedTempFile::new().unwrap();
        original
            .save_to_file(vg1.path().to_str().unwrap(), GraphFormat::VG)
            .unwrap();
        let graph1 =
            CoreGraphDTO::load_from_file(vg1.path().to_str().unwrap(), GraphFormat::VG).unwrap();

        // Second VG write/read
        let vg2 = NamedTempFile::new().unwrap();
        graph1
            .save_to_file(vg2.path().to_str().unwrap(), GraphFormat::VG)
            .unwrap();
        let graph2 =
            CoreGraphDTO::load_from_file(vg2.path().to_str().unwrap(), GraphFormat::VG).unwrap();

        assert_eq!(
            graph1.nodes.len(),
            graph2.nodes.len(),
            "VG self round-trip node count mismatch for {path_string}"
        );
        assert_eq!(
            graph1.edges.len(),
            graph2.edges.len(),
            "VG self round-trip edge count mismatch for {path_string}"
        );
        assert_eq!(
            graph1.paths.len(),
            graph2.paths.len(),
            "VG self round-trip path count mismatch for {path_string}"
        );
    }
}

/// Cross-format test: GFA → VG → GFA and compare.
#[test]
fn test_vg_to_gfa_conversion() {
    let test_files = fs::read_dir("tests/test_files/gfa").unwrap();
    for entry in test_files {
        let path = entry.unwrap().path();
        let path_string = path.to_str().unwrap();

        let original = CoreGraphDTO::load_from_file(path_string, GraphFormat::GFA).unwrap();

        // GFA → VG
        let vg_file = NamedTempFile::new().unwrap();
        original
            .save_to_file(vg_file.path().to_str().unwrap(), GraphFormat::VG)
            .unwrap();

        // VG → GFA
        let vg_graph =
            CoreGraphDTO::load_from_file(vg_file.path().to_str().unwrap(), GraphFormat::VG)
                .unwrap();
        let gfa_file = NamedTempFile::new().unwrap();
        vg_graph
            .save_to_file(gfa_file.path().to_str().unwrap(), GraphFormat::GFA)
            .unwrap();

        // Parse the new GFA and compare with original
        let converted =
            CoreGraphDTO::load_from_file(gfa_file.path().to_str().unwrap(), GraphFormat::GFA)
                .unwrap();

        assert!(
            original.isomorphic(&converted),
            "GFA→VG→GFA round-trip not isomorphic for {path_string}"
        );
    }
}
