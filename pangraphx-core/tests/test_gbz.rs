use pangraphx_core::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use tempfile::NamedTempFile;

/// Builds a name -> sequence map for every node in the graph.
fn name_to_seq(g: &CoreGraphDTO) -> HashMap<String, Vec<u8>> {
    (0..g.nodes.len())
        .map(|id| (g.get_name_from_id(id), g.nodes[id].sequence.clone()))
        .collect()
}

/// The undirected adjacency as a set of {node name, node name} pairs, ignoring
/// orientation and overlap.
fn undirected_adjacency(g: &CoreGraphDTO) -> HashSet<(String, String)> {
    g.edges
        .iter()
        .map(|e| {
            let x = g.get_name_from_id(e.from_node);
            let y = g.get_name_from_id(e.to_node);
            if x < y { (x, y) } else { (y, x) }
        })
        .collect()
}

/// Returns true when every edge of the graph lies on some path.
///
/// GBZ is a path-first format: it reconstructs the graph from the GBWT, so edges
/// that are not traversed by any path cannot be represented. Synthetic inputs
/// such as `random.gfa` contain such off-path links and are therefore skipped.
fn is_path_complete(g: &CoreGraphDTO) -> bool {
    let mut adjacent = HashSet::new();
    for path in &g.paths {
        for window in path.steps.windows(2) {
            let (a, b) = (window[0].node_id, window[1].node_id);
            adjacent.insert(if a < b { (a, b) } else { (b, a) });
        }
    }
    g.edges.iter().all(|e| {
        let (a, b) = (e.from_node, e.to_node);
        adjacent.contains(&(if a < b { (a, b) } else { (b, a) }))
    })
}

/// Checks whether two graphs carry the same biological content after a GBZ
/// round-trip.
///
/// GBZ is a path-first format: it stores node sequences and paths and derives
/// edges from them, so a `GFA -> GBZ -> GFA` round-trip is faithful only up to:
///
/// - **overlaps** — GBZ does not store overlap information, so every edge comes
///   back with `overlap == 0` (only `tiny.gfa` has a non-zero overlap);
/// - **edge direction** — GBZ reconstructs a bidirected graph, so each undirected
///   link is emitted in its canonical orientation, which may differ from the
///   explicit `L`-line orientation in the source GFA.
///
/// This helper therefore compares node sequences (by name), paths (steps and
/// orientations), and the undirected adjacency (node pairs, ignoring overlap).
fn gbz_equivalent(a: &CoreGraphDTO, b: &CoreGraphDTO) -> bool {
    if a.nodes.len() != b.nodes.len()
        || a.edges.len() != b.edges.len()
        || a.paths.len() != b.paths.len()
    {
        return false;
    }

    let seq_a = name_to_seq(a);
    let seq_b = name_to_seq(b);
    if seq_a.len() != a.nodes.len() || seq_b.len() != b.nodes.len() {
        return false; // duplicate names make the mapping ambiguous
    }
    for (name, seq) in &seq_a {
        if seq_b.get(name) != Some(seq) {
            return false;
        }
    }

    if undirected_adjacency(a) != undirected_adjacency(b) {
        return false;
    }

    let paths_b: HashMap<&[u8], &Path> = b.paths.iter().map(|p| (p.name.as_slice(), p)).collect();
    for pa in &a.paths {
        let Some(pb) = paths_b.get(pa.name.as_slice()) else {
            return false;
        };
        if pa.steps.len() != pb.steps.len() {
            return false;
        }
        for (sa, sb) in pa.steps.iter().zip(&pb.steps) {
            let same_node = a.get_name_from_id(sa.node_id) == b.get_name_from_id(sb.node_id);
            if !same_node || sa.orientation != sb.orientation {
                return false;
            }
        }
    }
    true
}

/// Round-trip test: parse each GFA test file -> serialize to GBZ -> parse GBZ
/// back -> verify the graphs carry the same content.
#[test]
fn test_gbz_roundtrip_from_gfa() {
    let test_files = fs::read_dir("tests/test_files/gfa").unwrap();
    for entry in test_files {
        let path = entry.unwrap().path();
        let path_string = path.to_str().unwrap();
        println!("Testing GBZ round-trip from GFA file: {path_string}...");

        let original = CoreGraphDTO::load_from_file(path_string, GraphFormat::GFA).unwrap();

        if !is_path_complete(&original) {
            println!("Skipping {path_string}: contains edges not covered by any path.");
            continue;
        }

        let gbz_file = NamedTempFile::new().unwrap();
        original
            .save_to_file(gbz_file.path().to_str().unwrap(), GraphFormat::GBZ)
            .unwrap();
        assert!(gbz_file.path().exists());

        let reloaded =
            CoreGraphDTO::load_from_file(gbz_file.path().to_str().unwrap(), GraphFormat::GBZ)
                .unwrap();

        assert!(
            gbz_equivalent(&original, &reloaded),
            "Graphs not equivalent after GBZ round-trip for {path_string}"
        );
    }
}
