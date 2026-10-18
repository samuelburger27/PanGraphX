use pangraphx_core::*;
use std::fs;
use std::process::Command;
use tempfile::NamedTempFile;

fn gfa_test_files() -> Vec<String> {
    let mut files: Vec<String> = fs::read_dir("tests/test_files/gfa")
        .unwrap()
        .map(|entry| entry.unwrap().path().to_str().unwrap().to_string())
        .collect();
    files.sort();
    files
}

/// Whether every node is named by a positive integer, as odgi requires.
fn has_numeric_names(graph: &CoreGraphDTO) -> bool {
    (0..graph.nodes.len()).all(|id| {
        let name = graph.get_name_from_id(id);
        name.parse::<u64>()
            .is_ok_and(|n| n > 0 && n.to_string() == name)
    })
}

/// What `original` looks like after a trip through ODGI: overlaps are dropped,
/// and nodes are named `1..=n` unless their names are already positive integers.
fn as_stored_in_odgi(original: &CoreGraphDTO) -> CoreGraphDTO {
    let mut graph = original.clone();
    if !has_numeric_names(&graph) {
        graph.node_name_map = Some(
            (0..graph.nodes.len())
                .map(|id| (id, (id + 1).to_string().into_bytes()))
                .collect(),
        );
    }
    for edge in &mut graph.edges {
        edge.overlap = 0;
    }
    for path in &mut graph.paths {
        path.overlaps.clear();
    }
    graph
}

/// Writes every edge from the end odgi may pick, so graphs compare equal
/// regardless of which of the two equivalent directions an edge is stored in.
fn with_canonical_edges(mut graph: CoreGraphDTO) -> CoreGraphDTO {
    let flip = |o: Orientation| match o {
        Orientation::Forward => Orientation::Reverse,
        Orientation::Reverse => Orientation::Forward,
    };
    let key = |e: &Edge, g: &CoreGraphDTO| {
        (
            g.get_name_from_id(e.from_node),
            e.from_orient == Orientation::Reverse,
            g.get_name_from_id(e.to_node),
            e.to_orient == Orientation::Reverse,
        )
    };
    let edges = graph
        .edges
        .iter()
        .map(|e| {
            let flipped = Edge {
                from_node: e.to_node,
                from_orient: flip(e.to_orient),
                to_node: e.from_node,
                to_orient: flip(e.from_orient),
                overlap: e.overlap,
            };
            if key(&flipped, &graph) < key(e, &graph) {
                flipped
            } else {
                e.clone()
            }
        })
        .collect();
    graph.edges = edges;
    graph
}

/// Compares graphs as odgi stores them (see [`as_stored_in_odgi`]).
fn same_in_odgi(original: &CoreGraphDTO, other: &CoreGraphDTO) -> bool {
    with_canonical_edges(as_stored_in_odgi(original))
        .isomorphic(&with_canonical_edges(other.clone()))
}

fn odgi_available() -> bool {
    Command::new("odgi").arg("version").output().is_ok()
}

/// Round-trip test: parse each GFA test file → serialize to ODGI → parse ODGI
/// back → verify the graphs are isomorphic.
#[test]
fn test_odgi_roundtrip_from_gfa() {
    for path_string in gfa_test_files() {
        println!("Testing ODGI round-trip from GFA file: {path_string}...");
        let original = CoreGraphDTO::load_from_file(&path_string, GraphFormat::GFA).unwrap();

        let og_file = NamedTempFile::new().unwrap();
        let og_path = og_file.path().to_str().unwrap();
        original.save_to_file(og_path, GraphFormat::ODGI).unwrap();
        let reloaded = CoreGraphDTO::load_from_file(og_path, GraphFormat::ODGI).unwrap();

        assert!(
            as_stored_in_odgi(&original).isomorphic(&reloaded),
            "Graphs not isomorphic after ODGI round-trip for {path_string}"
        );
    }
}

/// Our ODGI files load in `odgi` and convert back to the same graph, and files
/// written by `odgi build` parse to the same graph. Skipped without `odgi`.
#[test]
fn test_odgi_cross_validation() {
    if !odgi_available() {
        println!("'odgi' not found on PATH; skipping ODGI cross-validation.");
        return;
    }
    for path_string in gfa_test_files() {
        println!("Cross-validating ODGI with odgi for {path_string}...");
        let original = CoreGraphDTO::load_from_file(&path_string, GraphFormat::GFA).unwrap();

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
        let viewed =
            CoreGraphDTO::load_from_file(gfa_file.path().to_str().unwrap(), GraphFormat::GFA)
                .unwrap();
        assert!(
            same_in_odgi(&original, &viewed),
            "odgi view of our ODGI file differs for {path_string}"
        );

        // odgi → pangraphx; `odgi build` only accepts integer segment names.
        if !has_numeric_names(&original) {
            continue;
        }
        let reference = NamedTempFile::new().unwrap();
        let reference_path = reference.path().to_str().unwrap();
        let build = Command::new("odgi")
            .args(["build", "-g", &path_string, "-o", reference_path])
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "odgi build failed for {path_string}: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let parsed = CoreGraphDTO::load_from_file(reference_path, GraphFormat::ODGI).unwrap();
        assert!(
            same_in_odgi(&original, &parsed),
            "odgi build output parses to a different graph for {path_string}"
        );
    }
}
