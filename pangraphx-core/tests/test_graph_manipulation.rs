use pangraphx_core::{CoreGraph, CoreGraphDTO, Edge, Orientation, Path, Step};

#[test]
fn test_add_node() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let node_id = graph.add_node(b"ACGT".to_vec(), None);
    assert_eq!(node_id, 0);
    assert_eq!(graph.node_count(), 1);
    assert_eq!(graph.nodes[0].sequence, b"ACGT");
}

#[test]
fn test_add_node_with_name() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let node_id = graph.add_node(b"ACGT".to_vec(), Some(b"node1".to_vec()));
    assert_eq!(node_id, 0);
    assert!(graph.node_name_map.is_some());
    assert_eq!(
        graph.node_name_map.as_ref().unwrap().get(&node_id),
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
    assert_eq!(graph.edges[0], edge);

    // Check adjacency list
    assert!(graph.adjacency_list.contains_key(&n1));
    assert_eq!(graph.adjacency_list[&n1], vec![0]);
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
            Step {
                node_id: n1,
                orientation: Orientation::Forward,
            },
            Step {
                node_id: n2,
                orientation: Orientation::Forward,
            },
        ],
        overlaps: vec![],
    };

    graph.add_path(path.clone()).unwrap();
    assert_eq!(graph.path_count(), 1);
    assert_eq!(graph.path_map.get(&b"path1".to_vec()), Some(&path));
}

#[test]
fn test_add_path_duplicate() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());

    let n1 = graph.add_node(b"ACGT".to_vec(), None);

    let path = Path {
        name: b"path1".to_vec(),
        steps: vec![Step {
            node_id: n1,
            orientation: Orientation::Forward,
        }],
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
    assert_eq!(graph.nodes[0].sequence, b"TGCA");
    assert_eq!(graph.nodes[1].sequence, b"GCTA");
    assert_eq!(graph.nodes[0].id, 0);
    assert_eq!(graph.nodes[1].id, 1);
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
            Step {
                node_id: n1,
                orientation: Orientation::Forward,
            },
            Step {
                node_id: n2,
                orientation: Orientation::Forward,
            },
            Step {
                node_id: n3,
                orientation: Orientation::Forward,
            },
        ],
        overlaps: vec![],
    };

    graph.add_path(path).unwrap();
    graph.remove_node(n2).unwrap();

    let updated_path = graph.path_map.get(&b"path1".to_vec()).unwrap();
    assert_eq!(updated_path.steps.len(), 2);
    assert_eq!(updated_path.steps[0].node_id, 0); // n1, still at position 0
    assert_eq!(updated_path.steps[1].node_id, 1); // n3, now at position 1
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
        steps: vec![Step {
            node_id: n1,
            orientation: Orientation::Forward,
        }],
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
            Step {
                node_id: n1,
                orientation: Orientation::Forward,
            },
            Step {
                node_id: n2,
                orientation: Orientation::Forward,
            },
        ],
        overlaps: vec![],
    };

    let path2 = Path {
        name: b"path2".to_vec(),
        steps: vec![
            Step {
                node_id: n2,
                orientation: Orientation::Forward,
            },
            Step {
                node_id: n3,
                orientation: Orientation::Forward,
            },
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
    let p1 = graph.path_map.get(&b"path1".to_vec()).unwrap();
    assert_eq!(p1.steps.len(), 1);
    assert_eq!(p1.steps[0].node_id, 0);

    let p2 = graph.path_map.get(&b"path2".to_vec()).unwrap();
    assert_eq!(p2.steps.len(), 1);
    assert_eq!(p2.steps[0].node_id, 1);
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
fn test_get_node_mut() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    let n1 = graph.add_node(b"ACGT".to_vec(), None);

    {
        let node = graph.get_node_mut(n1).unwrap();
        node.sequence = b"TTTT".to_vec();
    }

    assert_eq!(graph.nodes[0].sequence, b"TTTT");
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
        steps: vec![Step {
            node_id: n1,
            orientation: Orientation::Forward,
        }],
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

    assert!(graph.has_edge(n1, n2));
    assert!(!graph.has_edge(n2, n1));
    assert!(!graph.has_edge(n1, n3));
}
