use pangraphx_core::{CoreGraph, CoreGraphDTO, Edge, Orientation, Path, Step};

#[test]
fn test_add_node() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let node_id = graph.add_node(b"ACGT".to_vec(), None);
    assert_eq!(node_id, 0);
    assert_eq!(graph.node_count(), 1);
    assert_eq!(graph.nodes()[0].sequence, b"ACGT");
}

#[test]
fn test_add_node_with_name() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let node_id = graph.add_node(b"ACGT".to_vec(), Some(b"node1".to_vec()));
    assert_eq!(node_id, 0);
    assert!(graph.node_name_map().is_some());
    assert_eq!(
        graph.node_name_map().as_ref().unwrap().get(&node_id),
        Some(&b"node1".to_vec())
    );
}

#[test]
fn test_add_edge() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let n1 = graph.add_node(b"ACGT".to_vec(), None);
    let n2 = graph.add_node(b"TGCA".to_vec(), None);

    let edge = Edge {
        from_node: n1,
        from_orient: Orientation::Forward,
        to_node: n2,
        to_orient: Orientation::Forward,
        overlap: 0,
    };

    let edge_idx = graph.add_edge(edge.clone()).unwrap();
    assert_eq!(edge_idx, 0);
    assert_eq!(graph.edge_count(), 1);
    assert_eq!(graph.edges()[0], edge);

    // Check adjacency list
    assert_eq!(graph.incident_edge_indices(n1), &[0]);
    assert_eq!(graph.incident_edge_indices(n2), &[0]);
}

#[test]
fn test_add_edge_invalid_node() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let edge = Edge {
        from_node: 0,
        from_orient: Orientation::Forward,
        to_node: 1,
        to_orient: Orientation::Forward,
        overlap: 0,
    };

    let result = graph.add_edge(edge);
    assert!(result.is_err());
}

#[test]
fn test_add_path() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let n1 = graph.add_node(b"ACGT".to_vec(), None);
    let n2 = graph.add_node(b"TGCA".to_vec(), None);

    let path = Path {
        name: b"path1".to_vec(),
        steps: vec![
            Step::new(n1, Orientation::Forward).unwrap(),
            Step::new(n2, Orientation::Forward).unwrap(),
        ],
        overlaps: vec![],
    };

    graph.add_path(path.clone()).unwrap();
    assert_eq!(graph.path_count(), 1);
    assert_eq!(graph.get_path(b"path1").ok(), Some(&path));
}

#[test]
fn test_add_path_duplicate() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let n1 = graph.add_node(b"ACGT".to_vec(), None);

    let path = Path {
        name: b"path1".to_vec(),
        steps: vec![Step::new(n1, Orientation::Forward).unwrap()],
        overlaps: vec![],
    };

    graph.add_path(path.clone()).unwrap();
    let result = graph.add_path(path);
    assert!(result.is_err());
}

#[test]
fn test_remove_node() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let n1 = graph.add_node(b"ACGT".to_vec(), None);
    let _n2 = graph.add_node(b"TGCA".to_vec(), None);
    let _n3 = graph.add_node(b"GCTA".to_vec(), None);

    assert_eq!(graph.node_count(), 3);

    graph.remove_node(n1).unwrap();

    assert_eq!(graph.node_count(), 2);
    assert_eq!(graph.nodes()[0].sequence, b"TGCA");
    assert_eq!(graph.nodes()[1].sequence, b"GCTA");
}

#[test]
fn test_remove_node_with_edges() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let n1 = graph.add_node(b"ACGT".to_vec(), None);
    let n2 = graph.add_node(b"TGCA".to_vec(), None);
    let n3 = graph.add_node(b"GCTA".to_vec(), None);

    graph
        .add_edge(Edge {
            from_node: n1,
            from_orient: Orientation::Forward,
            to_node: n2,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();

    graph
        .add_edge(Edge {
            from_node: n2,
            from_orient: Orientation::Forward,
            to_node: n3,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();

    assert_eq!(graph.edge_count(), 2);

    graph.remove_node(n2).unwrap();

    // Both edges should be removed since they connect to n2
    assert_eq!(graph.edge_count(), 0);
    assert_eq!(graph.node_count(), 2);
}

#[test]
fn test_remove_node_updates_paths() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let n1 = graph.add_node(b"ACGT".to_vec(), None);
    let n2 = graph.add_node(b"TGCA".to_vec(), None);
    let n3 = graph.add_node(b"GCTA".to_vec(), None);

    let path = Path {
        name: b"path1".to_vec(),
        steps: vec![
            Step::new(n1, Orientation::Forward).unwrap(),
            Step::new(n2, Orientation::Forward).unwrap(),
            Step::new(n3, Orientation::Forward).unwrap(),
        ],
        overlaps: vec![],
    };

    graph.add_path(path).unwrap();
    graph.remove_node(n2).unwrap();

    let updated_path = graph.get_path(b"path1").unwrap();
    assert_eq!(updated_path.steps.len(), 2);
    assert_eq!(updated_path.steps[0].node_id(), 0); // n1, still at position 0
    assert_eq!(updated_path.steps[1].node_id(), 1); // n3, now at position 1
}

#[test]
fn test_remove_edge() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let n1 = graph.add_node(b"ACGT".to_vec(), None);
    let n2 = graph.add_node(b"TGCA".to_vec(), None);

    let edge_idx = graph
        .add_edge(Edge {
            from_node: n1,
            from_orient: Orientation::Forward,
            to_node: n2,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();

    assert_eq!(graph.edge_count(), 1);

    graph.remove_edge(edge_idx).unwrap();

    assert_eq!(graph.edge_count(), 0);
}

#[test]
fn test_remove_path() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let n1 = graph.add_node(b"ACGT".to_vec(), None);

    let path = Path {
        name: b"path1".to_vec(),
        steps: vec![Step::new(n1, Orientation::Forward).unwrap()],
        overlaps: vec![],
    };

    graph.add_path(path).unwrap();
    assert_eq!(graph.path_count(), 1);

    graph.remove_path(b"path1").unwrap();
    assert_eq!(graph.path_count(), 0);
}

#[test]
fn test_remove_path_nonexistent() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let result = graph.remove_path(b"nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_complex_graph_manipulation() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    // Build a small graph
    let n1 = graph.add_node(b"ACGT".to_vec(), Some(b"node1".to_vec()));
    let n2 = graph.add_node(b"TGCA".to_vec(), Some(b"node2".to_vec()));
    let n3 = graph.add_node(b"GCTA".to_vec(), Some(b"node3".to_vec()));

    graph
        .add_edge(Edge {
            from_node: n1,
            from_orient: Orientation::Forward,
            to_node: n2,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();

    graph
        .add_edge(Edge {
            from_node: n2,
            from_orient: Orientation::Forward,
            to_node: n3,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();

    let path1 = Path {
        name: b"path1".to_vec(),
        steps: vec![
            Step::new(n1, Orientation::Forward).unwrap(),
            Step::new(n2, Orientation::Forward).unwrap(),
        ],
        overlaps: vec![],
    };

    let path2 = Path {
        name: b"path2".to_vec(),
        steps: vec![
            Step::new(n2, Orientation::Forward).unwrap(),
            Step::new(n3, Orientation::Forward).unwrap(),
        ],
        overlaps: vec![],
    };

    graph.add_path(path1).unwrap();
    graph.add_path(path2).unwrap();

    assert_eq!(graph.node_count(), 3);
    assert_eq!(graph.edge_count(), 2);
    assert_eq!(graph.path_count(), 2);

    // Now remove the middle node
    graph.remove_node(n2).unwrap();

    assert_eq!(graph.node_count(), 2);
    assert_eq!(graph.edge_count(), 0); // Both edges removed

    // Check paths were updated
    let p1 = graph.get_path(b"path1").unwrap();
    assert_eq!(p1.steps.len(), 1);
    assert_eq!(p1.steps[0].node_id(), 0);

    let p2 = graph.get_path(b"path2").unwrap();
    assert_eq!(p2.steps.len(), 1);
    assert_eq!(p2.steps[0].node_id(), 1);
}
#[test]
fn test_get_node() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    let n1 = graph.add_node(b"ACGT".to_vec(), None);

    let node = graph.get_node(n1).unwrap();
    assert_eq!(node.sequence, b"ACGT");

    let result = graph.get_node(999);
    assert!(result.is_err());
}

#[test]
fn test_set_sequence() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    let n1 = graph.add_node(b"ACGT".to_vec(), None);

    graph.set_sequence(n1, b"TTTT".to_vec()).unwrap();
    assert_eq!(graph.nodes()[0].sequence, b"TTTT");
    assert!(graph.set_sequence(5, b"A".to_vec()).is_err());
}

#[test]
fn test_get_edge() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    let n1 = graph.add_node(b"ACGT".to_vec(), None);
    let n2 = graph.add_node(b"TGCA".to_vec(), None);

    let edge = Edge {
        from_node: n1,
        from_orient: Orientation::Forward,
        to_node: n2,
        to_orient: Orientation::Forward,
        overlap: 0,
    };
    let idx = graph.add_edge(edge.clone()).unwrap();

    let retrieved_edge = graph.get_edge(idx).unwrap();
    assert_eq!(retrieved_edge, &edge);

    let result = graph.get_edge(999);
    assert!(result.is_err());
}

#[test]
fn test_get_path() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    let n1 = graph.add_node(b"ACGT".to_vec(), None);

    let path = Path {
        name: b"path1".to_vec(),
        steps: vec![Step::new(n1, Orientation::Forward).unwrap()],
        overlaps: vec![],
    };
    graph.add_path(path.clone()).unwrap();

    let retrieved_path = graph.get_path(b"path1").unwrap();
    assert_eq!(retrieved_path, &path);

    let result = graph.get_path(b"nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_get_outgoing_edges() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    let n1 = graph.add_node(b"ACGT".to_vec(), None);
    let n2 = graph.add_node(b"TGCA".to_vec(), None);
    let n3 = graph.add_node(b"GCTA".to_vec(), None);

    graph
        .add_edge(Edge {
            from_node: n1,
            from_orient: Orientation::Forward,
            to_node: n2,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();

    graph
        .add_edge(Edge {
            from_node: n1,
            from_orient: Orientation::Forward,
            to_node: n3,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();

    let outgoing = graph.get_outgoing_edges(n1).unwrap();
    assert_eq!(outgoing.len(), 2);
    assert_eq!(outgoing[0].to_node, n2);
    assert_eq!(outgoing[1].to_node, n3);

    let outgoing_n2 = graph.get_outgoing_edges(n2).unwrap();
    assert_eq!(outgoing_n2.len(), 0);
}

#[test]
fn test_has_edge() {
    use Orientation::{Forward, Reverse};
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    let n1 = graph.add_node(b"ACGT".to_vec(), None);
    let n2 = graph.add_node(b"TGCA".to_vec(), None);
    let n3 = graph.add_node(b"GCTA".to_vec(), None);

    graph
        .add_edge(Edge {
            from_node: n1,
            from_orient: Orientation::Forward,
            to_node: n2,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();

    assert!(graph.has_edge(n1, Forward, n2, Forward));
    // The same link read from the other side: n2- -> n1-.
    assert!(graph.has_edge(n2, Reverse, n1, Reverse));
    assert!(!graph.has_edge(n2, Forward, n1, Forward));
    assert!(!graph.has_edge(n1, Forward, n2, Reverse));
    assert!(!graph.has_edge(n1, Forward, n3, Forward));
    assert!(!graph.has_edge(7, Forward, n1, Forward));
}

fn forward(id: usize) -> Step {
    Step::new(id, Orientation::Forward).unwrap()
}

#[test]
fn test_remove_node_keeps_overlaps_in_step() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    for seq in [b"AAAA", b"CCCC", b"GGGG", b"TTTT"] {
        graph.add_node(seq.to_vec(), None);
    }
    graph
        .add_path(Path {
            name: b"p".to_vec(),
            steps: vec![forward(0), forward(1), forward(2), forward(3)],
            overlaps: vec![1, 2, 3],
        })
        .unwrap();

    graph.remove_node(1).unwrap();
    let path = graph.get_path(b"p").unwrap();
    assert_eq!(path.steps, vec![forward(0), forward(1), forward(2)]);
    // 0 -> 2 were not adjacent, so their junction has no overlap; 2 -> 3 keeps 3.
    assert_eq!(path.overlaps, vec![0, 3]);

    graph.remove_node(2).unwrap();
    let path = graph.get_path(b"p").unwrap();
    assert_eq!(path.steps, vec![forward(0), forward(1)]);
    assert_eq!(path.overlaps, vec![0]);
}

#[test]
fn test_paths_keep_insertion_order() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    let n = graph.add_node(b"ACGT".to_vec(), None);
    let names: Vec<Vec<u8>> = (0..20)
        .rev()
        .map(|i| format!("p{i}").into_bytes())
        .collect();
    for name in &names {
        graph
            .add_path(Path {
                name: name.clone(),
                steps: vec![forward(n)],
                overlaps: vec![],
            })
            .unwrap();
    }
    graph.remove_path(b"p10").unwrap();
    assert_eq!(graph.get_path(b"p9").unwrap().name, b"p9");

    let dto = CoreGraphDTO::from(graph);
    let expected: Vec<&[u8]> = names
        .iter()
        .map(Vec::as_slice)
        .filter(|name| *name != b"p10")
        .collect();
    let actual: Vec<&[u8]> = dto.paths().iter().map(|p| p.name.as_slice()).collect();
    assert_eq!(actual, expected);
}

#[test]
fn test_replace_path() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    let n1 = graph.add_node(b"ACGT".to_vec(), None);
    let n2 = graph.add_node(b"TGCA".to_vec(), None);
    let path = |steps| Path {
        name: b"p".to_vec(),
        steps,
        overlaps: vec![],
    };
    graph.add_path(path(vec![forward(n1)])).unwrap();

    let old = graph
        .replace_path(path(vec![forward(n1), forward(n2)]))
        .unwrap();
    assert_eq!(old.steps, vec![forward(n1)]);
    assert_eq!(graph.get_path(b"p").unwrap().steps.len(), 2);

    assert!(graph.replace_path(path(vec![forward(7)])).is_err());
    assert!(
        graph
            .replace_path(Path {
                name: b"missing".to_vec(),
                ..path(vec![])
            })
            .is_err()
    );
}

#[test]
fn test_incident_edges_follow_edits() {
    let edge = |from_node, to_node| Edge {
        from_node,
        from_orient: Orientation::Forward,
        to_node,
        to_orient: Orientation::Forward,
        overlap: 0,
    };
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    let a = graph.add_node(b"A".to_vec(), None);
    let b = graph.add_node(b"C".to_vec(), None);
    graph.add_edge(edge(a, b)).unwrap();
    // The first lookup builds the index; later edits must keep it current.
    assert_eq!(graph.incident_edge_indices(b), &[0]);

    let c = graph.add_node(b"G".to_vec(), None);
    graph.add_edge(edge(b, c)).unwrap();
    graph.add_edge(edge(c, c)).unwrap();
    assert_eq!(graph.incident_edge_indices(b), &[0, 1]);
    assert_eq!(graph.incident_edge_indices(c), &[1, 2]);
    assert_eq!(graph.incident_edge_indices(9), &[] as &[usize]);

    graph.remove_edge(0).unwrap();
    assert_eq!(graph.incident_edge_indices(a), &[] as &[usize]);
    assert_eq!(graph.incident_edge_indices(b), &[0]);
    assert_eq!(graph.incident_edge_indices(c), &[0, 1]);

    graph.remove_node(a).unwrap();
    assert_eq!(graph.incident_edge_indices(0), &[0]);
    assert_eq!(graph.incident_edge_indices(1), &[0, 1]);
}
