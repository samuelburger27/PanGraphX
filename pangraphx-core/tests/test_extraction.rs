use pangraphx_core::{CoreGraph, CoreGraphDTO, Edge, Orientation, Path, Step};

fn chain_graph(n: usize) -> CoreGraph {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    for i in 0..n {
        graph.add_node(format!("N{i}").into_bytes(), None);
    }
    for i in 0..n.saturating_sub(1) {
        graph
            .add_edge(Edge {
                from_node: i,
                from_orient: Orientation::Forward,
                to_node: i + 1,
                to_orient: Orientation::Forward,
                overlap: 0,
            })
            .unwrap();
    }
    graph
}

#[test]
fn test_extract_path_nodes_and_edges() {
    let mut graph = chain_graph(4);
    // Extra edge between 0 and 2 that is not part of the path.
    graph
        .add_edge(Edge {
            from_node: 0,
            from_orient: Orientation::Forward,
            to_node: 2,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();
    graph
        .add_path(Path {
            name: b"p1".to_vec(),
            steps: (0..4)
                .map(|id| Step {
                    node_id: id,
                    orientation: Orientation::Forward,
                })
                .collect(),
            overlaps: vec![0, 0, 0],
        })
        .unwrap();

    let sub = graph.extract_path(b"p1").unwrap();

    assert_eq!(sub.nodes.len(), 4);
    // Only the two edges along the chain, not the extra 0->2 edge.
    assert_eq!(sub.edges.len(), 3);
    assert_eq!(sub.paths.len(), 1);
    assert_eq!(sub.paths[0].steps.len(), 4);
    // Node IDs are renumbered contiguously.
    assert_eq!(sub.paths[0].steps[0].node_id, 0);
    assert_eq!(sub.paths[0].steps[3].node_id, 3);
}

#[test]
fn test_extract_path_matches_reversed_edge() {
    // Edge stored in the opposite direction to the path traversal.
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    graph.add_node(b"A".to_vec(), None);
    graph.add_node(b"B".to_vec(), None);
    graph
        .add_edge(Edge {
            from_node: 1,
            from_orient: Orientation::Forward,
            to_node: 0,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();
    graph
        .add_path(Path {
            name: b"p".to_vec(),
            steps: vec![
                Step {
                    node_id: 0,
                    orientation: Orientation::Forward,
                },
                Step {
                    node_id: 1,
                    orientation: Orientation::Forward,
                },
            ],
            overlaps: vec![0],
        })
        .unwrap();

    let sub = graph.extract_path(b"p").unwrap();
    assert_eq!(sub.edges.len(), 1);
}

#[test]
fn test_extract_path_unknown() {
    let graph = chain_graph(3);
    assert!(graph.extract_path(b"missing").is_err());
}

#[test]
fn test_extract_neighborhood_radius() {
    let graph = chain_graph(5);

    let r0 = graph.extract_neighborhood(2, 0).unwrap();
    assert_eq!(r0.nodes.len(), 1);

    let r1 = graph.extract_neighborhood(2, 1).unwrap();
    assert_eq!(r1.nodes.len(), 3);

    let r2 = graph.extract_neighborhood(2, 2).unwrap();
    assert_eq!(r2.nodes.len(), 5);
}

#[test]
fn test_extract_neighborhood_is_undirected() {
    // Directed edge 0 -> 1; neighbourhood of 1 must reach 0.
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    graph.add_node(b"A".to_vec(), None);
    graph.add_node(b"B".to_vec(), None);
    graph
        .add_edge(Edge {
            from_node: 0,
            from_orient: Orientation::Forward,
            to_node: 1,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();

    let sub = graph.extract_neighborhood(1, 1).unwrap();
    assert_eq!(sub.nodes.len(), 2);
}

#[test]
fn test_extract_neighborhood_keeps_contained_paths() {
    let mut graph = chain_graph(4);
    // Path fully inside the 1-hop neighbourhood of node 1 (nodes 0..2).
    graph
        .add_path(Path {
            name: b"inside".to_vec(),
            steps: vec![
                Step {
                    node_id: 0,
                    orientation: Orientation::Forward,
                },
                Step {
                    node_id: 1,
                    orientation: Orientation::Forward,
                },
            ],
            overlaps: vec![0],
        })
        .unwrap();
    // Path extending beyond the neighbourhood (nodes 0..3).
    graph
        .add_path(Path {
            name: b"outside".to_vec(),
            steps: (0..4)
                .map(|id| Step {
                    node_id: id,
                    orientation: Orientation::Forward,
                })
                .collect(),
            overlaps: vec![0, 0, 0],
        })
        .unwrap();

    let sub = graph.extract_neighborhood(1, 1).unwrap();
    let names: Vec<_> = sub.paths.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec![b"inside".to_vec()]);
}

#[test]
fn test_extract_component() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    for i in 0..5 {
        graph.add_node(format!("N{i}").into_bytes(), None);
    }
    // Component A: 0-1-2
    for (a, b) in [(0, 1), (1, 2)] {
        graph
            .add_edge(Edge {
                from_node: a,
                from_orient: Orientation::Forward,
                to_node: b,
                to_orient: Orientation::Forward,
                overlap: 0,
            })
            .unwrap();
    }
    // Component B: 3-4
    graph
        .add_edge(Edge {
            from_node: 3,
            from_orient: Orientation::Forward,
            to_node: 4,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();

    let sub = graph.extract_component(0).unwrap();
    assert_eq!(sub.nodes.len(), 3);
    assert_eq!(sub.edges.len(), 2);

    let sub_b = graph.extract_component(4).unwrap();
    assert_eq!(sub_b.nodes.len(), 2);
    assert_eq!(sub_b.edges.len(), 1);
}

#[test]
fn test_extract_component_includes_isolated_node() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    graph.add_node(b"A".to_vec(), None);
    graph.add_node(b"B".to_vec(), None);
    graph
        .add_edge(Edge {
            from_node: 0,
            from_orient: Orientation::Forward,
            to_node: 1,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();
    graph.add_node(b"C".to_vec(), None);

    let sub = graph.extract_component(2).unwrap();
    assert_eq!(sub.nodes.len(), 1);
    assert_eq!(sub.edges.len(), 0);
}

#[test]
fn test_extract_renumbers_name_map() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    graph.add_node(b"A".to_vec(), Some(b"node0".to_vec()));
    graph.add_node(b"B".to_vec(), Some(b"node1".to_vec()));
    graph.add_node(b"C".to_vec(), Some(b"node2".to_vec()));
    graph
        .add_edge(Edge {
            from_node: 0,
            from_orient: Orientation::Forward,
            to_node: 1,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();
    graph
        .add_edge(Edge {
            from_node: 1,
            from_orient: Orientation::Forward,
            to_node: 2,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .unwrap();

    // Extract the component containing node 1 (nodes 0,1,2).
    let sub = graph.extract_component(1).unwrap();
    let map = sub.node_name_map.as_ref().unwrap();
    assert_eq!(map.get(&0), Some(&b"node0".to_vec()));
    assert_eq!(map.get(&1), Some(&b"node1".to_vec()));
    assert_eq!(map.get(&2), Some(&b"node2".to_vec()));
}

#[test]
fn test_resolve_node_id() {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    graph.add_node(b"A".to_vec(), Some(b"alpha".to_vec()));
    graph.add_node(b"B".to_vec(), Some(b"beta".to_vec()));

    assert_eq!(graph.resolve_node_id("0").unwrap(), 0);
    assert_eq!(graph.resolve_node_id("alpha").unwrap(), 0);
    assert_eq!(graph.resolve_node_id("beta").unwrap(), 1);
    assert!(graph.resolve_node_id("gamma").is_err());
    assert!(graph.resolve_node_id("42").is_err());
}

#[test]
fn test_extract_neighborhood_unknown_node() {
    let graph = chain_graph(3);
    assert!(graph.extract_neighborhood(10, 1).is_err());
    assert!(graph.extract_component(10).is_err());
}
