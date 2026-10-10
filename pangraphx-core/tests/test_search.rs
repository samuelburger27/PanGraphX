use pangraphx_core::{CoreGraph, CoreGraphDTO, GraphFormat, Orientation};

fn load_tiny_graph() -> CoreGraph {
    let dto =
        CoreGraphDTO::load_from_file("tests/test_files/gfa/tiny.gfa", GraphFormat::GFA).unwrap();
    CoreGraph::new(dto)
}

#[test]
fn test_search_forward_match() {
    // tiny.gfa node 1 = "ACCTT"; query "ACCT" occurs at offset 0 forward.
    let graph = load_tiny_graph();
    let hits = graph.search(b"ACCT", 3).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].node_id, 0);
    assert_eq!(hits[0].orientation, Orientation::Forward);
    assert_eq!(hits[0].offset, 0);
}

#[test]
fn test_search_reverse_match() {
    // tiny.gfa node 2 = "TGGGA"; rc = "TCCCA"; query "TCCC" occurs at offset 0
    // on the reverse strand.
    let graph = load_tiny_graph();
    let hits = graph.search(b"TCCC", 3).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].node_id, 1);
    assert_eq!(hits[0].orientation, Orientation::Reverse);
    assert_eq!(hits[0].offset, 0);
}

#[test]
fn test_search_no_match() {
    let graph = load_tiny_graph();
    let hits = graph.search(b"GGGGGG", 3).unwrap();
    assert!(hits.is_empty());
}
