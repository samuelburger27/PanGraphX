use pangraphx_core::{CoreGraph, CoreGraphDTO, Edge, GraphFormat, Orientation, Path, Step};

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

const fn fwd(node_id: usize) -> Step {
    Step {
        node_id,
        orientation: Orientation::Forward,
    }
}

const fn edge(from_node: usize, to_node: usize, overlap: u32) -> Edge {
    Edge {
        from_node,
        from_orient: Orientation::Forward,
        to_node,
        to_orient: Orientation::Forward,
        overlap,
    }
}

/// Builds a small graph with known statistics:
/// - nodes 0..2 form one weakly connected component (lengths 4, 6, 2)
/// - nodes 3..4 form a second component (lengths 8, 10)
/// - node 5 is isolated (length 1)
/// - two paths: p1 over 0->1->2 (no overlaps), p2 over 3->4 (overlap 3)
fn build_test_graph() -> CoreGraph {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    graph.add_node(b"AAAA".to_vec(), None); // 0: 4 bp
    graph.add_node(b"CCCCCC".to_vec(), None); // 1: 6 bp
    graph.add_node(b"GG".to_vec(), None); // 2: 2 bp
    graph.add_node(b"TTTTTTTT".to_vec(), None); // 3: 8 bp
    graph.add_node(b"ACGTACGTAC".to_vec(), None); // 4: 10 bp
    graph.add_node(b"A".to_vec(), None); // 5: 1 bp (isolated)

    graph.add_edge(edge(0, 1, 0)).unwrap();
    graph.add_edge(edge(1, 2, 0)).unwrap();
    graph.add_edge(edge(3, 4, 0)).unwrap();

    graph
        .add_path(Path {
            name: b"p1".to_vec(),
            steps: vec![fwd(0), fwd(1), fwd(2)],
            overlaps: vec![],
        })
        .unwrap();
    graph
        .add_path(Path {
            name: b"p2".to_vec(),
            steps: vec![fwd(3), fwd(4)],
            overlaps: vec![3],
        })
        .unwrap();

    graph
}

#[test]
fn test_node_length_stats() {
    let stats = build_test_graph().compute_stats();
    let len = stats.node_len;

    assert_eq!(stats.node_count, 6);
    assert_eq!(len.count, 6);
    assert_eq!(len.total, 31); // 4 + 6 + 2 + 8 + 10 + 1
    assert_eq!(len.min, 1);
    assert_eq!(len.max, 10);
    assert!(approx(len.mean, 31.0 / 6.0));
    // Sorted desc [10, 8, 6, 4, 2, 1], total 31: cumulative reaches half (>= 15.5) at 8.
    assert_eq!(len.n50, 8);
}

#[test]
fn test_component_stats() {
    let stats = build_test_graph().compute_stats();
    // {0,1,2}, {3,4}, and isolated {5}.
    assert_eq!(stats.component_count, 3);
    assert_eq!(stats.largest_component, 3);
    assert_eq!(stats.isolated_nodes, 1);
}

#[test]
fn test_degree_stats() {
    let stats = build_test_graph().compute_stats();

    assert_eq!(stats.edge_count, 3);
    // in-degrees: [0,1,1,0,1,0]; out-degrees: [1,1,0,1,0,0]
    assert_eq!(stats.in_degree.min, 0);
    assert_eq!(stats.in_degree.max, 1);
    assert!(approx(stats.in_degree.mean, 0.5));
    assert_eq!(stats.out_degree.min, 0);
    assert_eq!(stats.out_degree.max, 1);
    assert!(approx(stats.out_degree.mean, 0.5));

    // total degrees per node: [1,2,1,1,1,0] -> deg 0:1, deg 1:4, deg 2:1
    assert_eq!(stats.total_degree_hist, vec![(0, 1), (1, 4), (2, 1)]);
}

#[test]
fn test_path_stats() {
    let stats = build_test_graph().compute_stats();
    let paths = stats.path_len;

    assert_eq!(stats.path_count, 2);
    assert_eq!(paths.count, 2);
    // p1: 4+6+2 = 12 bp (no overlap); p2: 8+10 - 3 = 15 bp
    assert_eq!(paths.bp_total, 27);
    assert_eq!(paths.bp_min, 12);
    assert_eq!(paths.bp_max, 15);
    assert!(approx(paths.bp_mean, 13.5));
    assert_eq!(paths.steps_min, 2);
    assert_eq!(paths.steps_max, 3);
    assert!(approx(paths.steps_mean, 2.5));
}

#[test]
fn test_empty_graph_stats() {
    let stats = CoreGraph::new(CoreGraphDTO::default()).compute_stats();

    assert_eq!(stats.node_count, 0);
    assert_eq!(stats.component_count, 0);
    assert_eq!(stats.largest_component, 0);
    assert_eq!(stats.node_len.n50, 0);
    assert!(approx(stats.node_len.mean, 0.0));
    assert_eq!(stats.path_count, 0);
    assert!(approx(stats.path_len.bp_mean, 0.0));
    assert!(stats.total_degree_hist.is_empty());
}

#[test]
fn test_stats_from_tiny_gfa_fixture() {
    let dto =
        CoreGraphDTO::load_from_file("tests/test_files/gfa/tiny.gfa", GraphFormat::GFA).unwrap();
    let stats = CoreGraph::new(dto).compute_stats();

    // tiny.gfa: nodes ACCTT (5) and TGGGA (5), one link 1+ -> 2- (5M),
    // one path 1+,2- with overlap 5M.
    assert_eq!(stats.node_count, 2);
    assert_eq!(stats.node_len.total, 10);
    assert_eq!(stats.node_len.min, 5);
    assert_eq!(stats.node_len.max, 5);
    assert_eq!(stats.node_len.n50, 5);

    assert_eq!(stats.edge_count, 1);
    assert_eq!(stats.component_count, 1);
    assert_eq!(stats.largest_component, 2);
    assert_eq!(stats.isolated_nodes, 0);
    assert!(approx(stats.in_degree.mean, 0.5));
    assert!(approx(stats.out_degree.mean, 0.5));

    assert_eq!(stats.path_count, 1);
    // 10 bp of node sequence minus the 5 bp overlap.
    assert_eq!(stats.path_len.bp_total, 5);
    assert_eq!(stats.path_len.steps_max, 2);
}
