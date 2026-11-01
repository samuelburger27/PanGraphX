use super::framing::{read_vg_graphs, write_vg_graphs, write_vg_group};
use super::reader::*;
use super::writer::*;
use super::*;
use crate::core::core_types::{Edge, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::proto_gen::vg as vg_proto;
use crate::traits::{GraphParser, GraphSerializer};
use std::collections::HashMap;
use std::io::Cursor;

// ---- Orientation mapping tests ----

#[test]
fn test_edge_orientation_mapping() {
    // Default: from_start=false, to_end=false → Forward, Forward
    assert_eq!(
        vg_edge_to_orientations(false, false),
        (Orientation::Forward, Orientation::Forward)
    );
    // from_start=true, to_end=false → Reverse, Forward
    assert_eq!(
        vg_edge_to_orientations(true, false),
        (Orientation::Reverse, Orientation::Forward)
    );
    // from_start=false, to_end=true → Forward, Reverse
    assert_eq!(
        vg_edge_to_orientations(false, true),
        (Orientation::Forward, Orientation::Reverse)
    );
    // from_start=true, to_end=true → Reverse, Reverse
    assert_eq!(
        vg_edge_to_orientations(true, true),
        (Orientation::Reverse, Orientation::Reverse)
    );
}

#[test]
fn test_orientation_roundtrip() {
    let cases = [
        (Orientation::Forward, Orientation::Forward),
        (Orientation::Forward, Orientation::Reverse),
        (Orientation::Reverse, Orientation::Forward),
        (Orientation::Reverse, Orientation::Reverse),
    ];
    for (from_o, to_o) in cases {
        let (fs, te) = orientations_to_vg_edge(from_o, to_o);
        let (from_o2, to_o2) = vg_edge_to_orientations(fs, te);
        assert_eq!(from_o, from_o2);
        assert_eq!(to_o, to_o2);
    }
}

// ---- Parse / Serialize round-trip tests ----

fn make_simple_graph() -> CoreGraphDTO {
    let nodes = Nodes::from_seq(vec![b"ACGT".to_vec(), b"TTGG".to_vec(), b"CCAA".to_vec()]);
    let mut name_map = HashMap::new();
    name_map.insert(0, b"1".to_vec());
    name_map.insert(1, b"2".to_vec());
    name_map.insert(2, b"3".to_vec());

    CoreGraphDTO {
        nodes,
        edges: vec![
            Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 1,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
            Edge {
                from_node: 1,
                from_orient: Orientation::Forward,
                to_node: 2,
                to_orient: Orientation::Forward,
                overlap: 2,
            },
        ],
        paths: vec![Path {
            name: b"path1".to_vec(),
            steps: vec![
                Step::new(0, Orientation::Forward).unwrap(),
                Step::new(1, Orientation::Forward).unwrap(),
                Step::new(2, Orientation::Reverse).unwrap(),
            ],
            overlaps: vec![],
        }],
        node_name_map: Some(name_map),
    }
}

#[test]
fn test_serialize_then_parse_roundtrip() {
    let original = make_simple_graph();
    let codec = VGCodec;

    // Serialize
    let mut buf = Vec::new();
    codec.serialize(&original, &mut buf).unwrap();
    assert!(!buf.is_empty(), "Serialized VG should not be empty");

    // Parse back
    let mut cursor = Cursor::new(&buf);
    let parsed = codec.parse(&mut cursor).unwrap();

    // Verify nodes
    assert_eq!(parsed.nodes.len(), original.nodes.len());
    for (i, node) in parsed.nodes.iter().enumerate() {
        assert_eq!(
            node.sequence, original.nodes[i].sequence,
            "Node {i} sequence mismatch"
        );
    }

    // Verify edges
    assert_eq!(parsed.edges.len(), original.edges.len());
    for (i, edge) in parsed.edges.iter().enumerate() {
        assert_eq!(edge.from_node, original.edges[i].from_node);
        assert_eq!(edge.to_node, original.edges[i].to_node);
        assert_eq!(edge.from_orient, original.edges[i].from_orient);
        assert_eq!(edge.to_orient, original.edges[i].to_orient);
        assert_eq!(edge.overlap, original.edges[i].overlap);
    }

    // Verify paths
    assert_eq!(parsed.paths.len(), original.paths.len());
    assert_eq!(parsed.paths[0].name, original.paths[0].name);
    assert_eq!(parsed.paths[0].steps.len(), original.paths[0].steps.len());
    for (i, step) in parsed.paths[0].steps.iter().enumerate() {
        assert_eq!(step.node_id(), original.paths[0].steps[i].node_id());
        assert_eq!(step.orientation(), original.paths[0].steps[i].orientation());
    }
}

#[test]
fn test_empty_graph_roundtrip() {
    let graph = CoreGraphDTO::default();
    let codec = VGCodec;

    let mut buf = Vec::new();
    codec.serialize(&graph, &mut buf).unwrap();

    let mut cursor = Cursor::new(&buf);
    let parsed = codec.parse(&mut cursor).unwrap();

    assert_eq!(parsed.nodes.len(), 0);
    assert_eq!(parsed.edges.len(), 0);
    assert_eq!(parsed.paths.len(), 0);
}

#[test]
fn test_single_node_no_edges() {
    let nodes = Nodes::from_seq(vec![b"GATTACA".to_vec()]);
    let mut name_map = HashMap::new();
    name_map.insert(0, b"42".to_vec());

    let graph = CoreGraphDTO {
        nodes,
        edges: vec![],
        paths: vec![],
        node_name_map: Some(name_map),
    };

    let codec = VGCodec;
    let mut buf = Vec::new();
    codec.serialize(&graph, &mut buf).unwrap();

    let mut cursor = Cursor::new(&buf);
    let parsed = codec.parse(&mut cursor).unwrap();

    assert_eq!(parsed.nodes.len(), 1);
    assert_eq!(parsed.nodes[0].sequence, b"GATTACA");
    assert_eq!(parsed.edges.len(), 0);
}

/// A chain of single-base nodes named `names`, with a path through all of them.
fn named_chain(names: &[&str]) -> CoreGraphDTO {
    let bases = [b"A", b"C", b"G", b"T"];
    CoreGraphDTO {
        nodes: Nodes::from_seq((0..names.len()).map(|i| bases[i % 4].to_vec()).collect()),
        edges: (1..names.len())
            .map(|i| Edge {
                from_node: i - 1,
                from_orient: Orientation::Forward,
                to_node: i,
                to_orient: Orientation::Forward,
                overlap: 0,
            })
            .collect(),
        paths: vec![Path {
            name: b"p".to_vec(),
            steps: (0..names.len())
                .map(|i| Step::new(i, Orientation::Forward).unwrap())
                .collect(),
            overlaps: vec![],
        }],
        node_name_map: Some(
            names
                .iter()
                .enumerate()
                .map(|(i, name)| (i, name.as_bytes().to_vec()))
                .collect(),
        ),
    }
}

/// The VG ids `graph` is written with; also checks the graph round-trips.
fn written_ids(graph: &CoreGraphDTO) -> Vec<i64> {
    let mut buf = Vec::new();
    VGCodec.serialize(graph, &mut buf).unwrap();
    let parsed = VGCodec.parse(&mut Cursor::new(&buf)).unwrap();
    assert!(parsed.isomorphic(graph), "VG round-trip changed the graph");
    VgWriteContext::new(graph).vg_ids
}

#[test]
fn test_distinct_positive_names_keep_ids() {
    assert_eq!(written_ids(&named_chain(&["7", "3", "1"])), vec![7, 3, 1]);
}

#[test]
fn test_mixed_names_are_renumbered() {
    // Per-node ids would give both `2` and `x` (index 1) the VG id 2.
    assert_eq!(written_ids(&named_chain(&["2", "x"])), vec![1, 2]);
}

/// Parses a stream of one-node chunks.
fn parse_chunks(nodes: &[(i64, &str)]) -> PanResult<CoreGraphDTO> {
    let graphs: Vec<vg_proto::Graph> = nodes
        .iter()
        .map(|&(id, sequence)| vg_proto::Graph {
            node: vec![vg_proto::Node {
                id,
                sequence: sequence.to_string(),
                name: String::new(),
            }],
            ..Default::default()
        })
        .collect();
    let mut buf = Vec::new();
    write_vg_graphs(&mut buf, &graphs).unwrap();
    VGCodec.parse(&mut Cursor::new(&buf))
}

#[test]
fn test_identical_repeated_node_is_merged() {
    let parsed = parse_chunks(&[(1, "ACGT"), (2, "GG"), (1, "ACGT")]).unwrap();
    assert_eq!(parsed.nodes.len(), 2);
}

#[test]
fn test_conflicting_duplicate_id_is_rejected() {
    let error = parse_chunks(&[(1, "ACGT"), (1, "CCCC")]).unwrap_err();
    assert!(
        error.to_string().contains("Duplicate VG node ID 1"),
        "{error}"
    );
}

#[test]
fn test_invalid_or_repeated_ids_force_renumbering() {
    for names in [&["0", "5"][..], &["-3", "5"], &["007", "5"], &["7", "007"]] {
        assert_eq!(written_ids(&named_chain(names)), vec![1, 2], "{names:?}");
    }
}

#[test]
fn test_all_orientation_combinations() {
    let nodes = Nodes::from_seq(vec![b"AA".to_vec(), b"CC".to_vec()]);
    let mut name_map = HashMap::new();
    name_map.insert(0, b"1".to_vec());
    name_map.insert(1, b"2".to_vec());

    let orientations = [
        (Orientation::Forward, Orientation::Forward),
        (Orientation::Forward, Orientation::Reverse),
        (Orientation::Reverse, Orientation::Forward),
        (Orientation::Reverse, Orientation::Reverse),
    ];

    for (from_o, to_o) in orientations {
        let graph = CoreGraphDTO {
            nodes: nodes.clone(),
            edges: vec![Edge {
                from_node: 0,
                from_orient: from_o,
                to_node: 1,
                to_orient: to_o,
                overlap: 0,
            }],
            paths: vec![],
            node_name_map: Some(name_map.clone()),
        };

        let codec = VGCodec;
        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.edges.len(), 1);
        assert_eq!(
            parsed.edges[0].from_orient, from_o,
            "from_orient mismatch for {from_o:?} → {to_o:?}"
        );
        assert_eq!(
            parsed.edges[0].to_orient, to_o,
            "to_orient mismatch for {from_o:?} → {to_o:?}"
        );
    }
}

#[test]
fn test_path_with_reverse_steps() {
    let nodes = Nodes::from_seq(vec![b"AAA".to_vec(), b"CCC".to_vec(), b"GGG".to_vec()]);
    let mut name_map = HashMap::new();
    name_map.insert(0, b"1".to_vec());
    name_map.insert(1, b"2".to_vec());
    name_map.insert(2, b"3".to_vec());

    let graph = CoreGraphDTO {
        nodes,
        edges: vec![],
        paths: vec![Path {
            name: b"haplotype_1".to_vec(),
            steps: vec![
                Step::new(0, Orientation::Forward).unwrap(),
                Step::new(1, Orientation::Reverse).unwrap(),
                Step::new(2, Orientation::Forward).unwrap(),
            ],
            overlaps: vec![],
        }],
        node_name_map: Some(name_map),
    };

    let codec = VGCodec;
    let mut buf = Vec::new();
    codec.serialize(&graph, &mut buf).unwrap();

    let mut cursor = Cursor::new(&buf);
    let parsed = codec.parse(&mut cursor).unwrap();

    assert_eq!(parsed.paths.len(), 1);
    assert_eq!(parsed.paths[0].steps[0].orientation(), Orientation::Forward);
    assert_eq!(parsed.paths[0].steps[1].orientation(), Orientation::Reverse);
    assert_eq!(parsed.paths[0].steps[2].orientation(), Orientation::Forward);
}

#[test]
fn test_graph_without_name_map() {
    let nodes = Nodes::from_seq(vec![b"AT".to_vec(), b"GC".to_vec()]);
    let graph = CoreGraphDTO {
        nodes,
        edges: vec![Edge {
            from_node: 0,
            from_orient: Orientation::Forward,
            to_node: 1,
            to_orient: Orientation::Forward,
            overlap: 0,
        }],
        paths: vec![],
        node_name_map: None,
    };

    let codec = VGCodec;
    let mut buf = Vec::new();
    codec.serialize(&graph, &mut buf).unwrap();

    let mut cursor = Cursor::new(&buf);
    let parsed = codec.parse(&mut cursor).unwrap();

    assert_eq!(parsed.nodes.len(), 2);
    assert_eq!(parsed.nodes[0].sequence, b"AT");
    assert_eq!(parsed.nodes[1].sequence, b"GC");
    assert_eq!(parsed.edges.len(), 1);
}

#[test]
fn test_multiple_paths() {
    let nodes = Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec(), b"G".to_vec()]);
    let mut name_map = HashMap::new();
    name_map.insert(0, b"1".to_vec());
    name_map.insert(1, b"2".to_vec());
    name_map.insert(2, b"3".to_vec());

    let graph = CoreGraphDTO {
        nodes,
        edges: vec![],
        paths: vec![
            Path {
                name: b"ref".to_vec(),
                steps: vec![
                    Step::new(0, Orientation::Forward).unwrap(),
                    Step::new(1, Orientation::Forward).unwrap(),
                ],
                overlaps: vec![],
            },
            Path {
                name: b"alt".to_vec(),
                steps: vec![
                    Step::new(0, Orientation::Forward).unwrap(),
                    Step::new(2, Orientation::Forward).unwrap(),
                ],
                overlaps: vec![],
            },
        ],
        node_name_map: Some(name_map),
    };

    let codec = VGCodec;
    let mut buf = Vec::new();
    codec.serialize(&graph, &mut buf).unwrap();

    let mut cursor = Cursor::new(&buf);
    let parsed = codec.parse(&mut cursor).unwrap();

    assert_eq!(parsed.paths.len(), 2);
    assert_eq!(parsed.paths[0].name, b"ref");
    assert_eq!(parsed.paths[1].name, b"alt");
    assert_eq!(parsed.paths[0].steps.len(), 2);
    assert_eq!(parsed.paths[1].steps.len(), 2);
}

#[test]
fn test_edge_overlap_preserved() {
    let nodes = Nodes::from_seq(vec![b"ACGT".to_vec(), b"GTAA".to_vec()]);
    let mut name_map = HashMap::new();
    name_map.insert(0, b"1".to_vec());
    name_map.insert(1, b"2".to_vec());

    let graph = CoreGraphDTO {
        nodes,
        edges: vec![Edge {
            from_node: 0,
            from_orient: Orientation::Forward,
            to_node: 1,
            to_orient: Orientation::Forward,
            overlap: 2,
        }],
        paths: vec![],
        node_name_map: Some(name_map),
    };

    let codec = VGCodec;
    let mut buf = Vec::new();
    codec.serialize(&graph, &mut buf).unwrap();

    let mut cursor = Cursor::new(&buf);
    let parsed = codec.parse(&mut cursor).unwrap();

    assert_eq!(parsed.edges[0].overlap, 2);
}

#[test]
fn test_large_graph_is_chunked_and_roundtrips() {
    // Enough nodes, edges and steps to span several messages and framing groups.
    // Nodes, edges and steps each take a third of the messages.
    let node_count = VG_CHUNK_SIZE * VG_GROUP_SIZE / 3 + 1500;
    let nodes = Nodes::from_seq(vec![b"A".to_vec(); node_count]);
    let edges: Vec<Edge> = (1..node_count)
        .map(|i| Edge {
            from_node: i - 1,
            from_orient: Orientation::Forward,
            to_node: i,
            to_orient: Orientation::Forward,
            overlap: 0,
        })
        .collect();
    let steps: Vec<Step> = (0..node_count)
        .map(|i| Step::new(i, (i % 3 == 0).into()).unwrap())
        .collect();
    let original = CoreGraphDTO {
        nodes,
        edges,
        paths: vec![
            Path {
                name: b"long".to_vec(),
                steps,
                overlaps: vec![],
            },
            Path {
                name: b"empty".to_vec(),
                steps: vec![],
                overlaps: vec![],
            },
        ],
        node_name_map: None,
    };

    let mut buf = Vec::new();
    VGCodec.serialize(&original, &mut buf).unwrap();

    let graphs = read_vg_graphs(&mut Cursor::new(&buf)).unwrap();
    assert!(graphs.len() > VG_GROUP_SIZE);
    assert!(graphs.iter().all(|g| g.node.len() <= VG_CHUNK_SIZE
        && g.edge.len() <= VG_CHUNK_SIZE
        && g.path.iter().all(|p| p.mapping.len() <= VG_CHUNK_SIZE)));

    let parsed = VGCodec.parse(&mut Cursor::new(&buf)).unwrap();
    assert!(parsed.isomorphic(&CoreGraphDTO {
        node_name_map: parsed.node_name_map.clone(),
        ..original
    }));
}

#[test]
fn test_parallel_parse_is_deterministic() {
    let node_count = 5 * VG_CHUNK_SIZE + 7;
    let original = CoreGraphDTO {
        nodes: Nodes::from_seq((0..node_count).map(|i| vec![b"ACGT"[i % 4]]).collect()),
        edges: (1..node_count)
            .map(|i| Edge {
                from_node: i - 1,
                from_orient: Orientation::Forward,
                to_node: i,
                to_orient: Orientation::Forward,
                overlap: 0,
            })
            .collect(),
        paths: (0..3)
            .map(|p| Path {
                name: format!("p{p}").into_bytes(),
                steps: (p..node_count)
                    .step_by(p + 1)
                    .map(|i| Step::new(i, (i % 3 == 0).into()).unwrap())
                    .collect(),
                overlaps: vec![],
            })
            .collect(),
        node_name_map: None,
    };
    let mut buf = Vec::new();
    VGCodec.serialize(&original, &mut buf).unwrap();

    let parse = |threads: usize, window: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| parse_vg(&mut Cursor::new(&buf), window).unwrap())
    };
    let expected = parse(1, VG_READ_WINDOW);
    assert!(expected.isomorphic(&CoreGraphDTO {
        node_name_map: expected.node_name_map.clone(),
        ..original
    }));
    // Windows of one message each, and of a few messages, on several threads.
    for (threads, window) in [(1, 1), (4, 1), (4, 50_000), (3, VG_READ_WINDOW)] {
        assert_eq!(
            parse(threads, window),
            expected,
            "{threads} threads, window {window}"
        );
    }
}

#[test]
fn test_out_of_range_path_node_id_is_rejected() {
    let graph = vg_proto::Graph {
        path: vec![vg_proto::Path {
            name: "p".to_string(),
            mapping: vec![vg_proto::Mapping {
                position: Some(vg_proto::Position {
                    node_id: i64::MAX,
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut buf = Vec::new();
    write_vg_graphs(&mut buf, &[graph]).unwrap();
    let error = VGCodec.parse(&mut Cursor::new(&buf)).unwrap_err();
    assert!(
        error.to_string().contains("outside the supported range"),
        "{error}"
    );
}

#[test]
fn test_out_of_range_node_id_is_rejected() {
    let error = parse_chunks(&[(1, "A"), (1 << 62, "C")]).unwrap_err();
    assert!(
        error.to_string().contains("outside the supported range"),
        "{error}"
    );
    assert!(parse_chunks(&[(-(1 << 62), "A")]).is_ok());
}

#[test]
fn test_negative_edge_overlap_is_rejected() {
    let graph = vg_proto::Graph {
        node: vec![vg_proto::Node {
            id: 1,
            sequence: "ACGT".to_string(),
            name: String::new(),
        }],
        edge: vec![vg_proto::Edge {
            from: 1,
            to: 1,
            overlap: -1,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut buf = Vec::new();
    write_vg_graphs(&mut buf, &[graph]).unwrap();
    let error = VGCodec.parse(&mut Cursor::new(&buf)).unwrap_err();
    assert!(error.to_string().contains("negative overlap -1"), "{error}");
}

#[test]
fn test_invalid_utf8_name_is_rejected() {
    // A Graph holding one node: name (field 2) = `name_byte`, id (field 3) = 1.
    let parse = |name_byte: u8| {
        let mut buf = Vec::new();
        let graph = vec![0x0A, 0x05, 0x12, 0x01, name_byte, 0x18, 0x01];
        write_vg_group(&mut buf, &[graph]).unwrap();
        VGCodec.parse(&mut Cursor::new(&buf))
    };
    assert_eq!(parse(b'x').unwrap().node_name_map.unwrap()[&0], b"x");
    let error = parse(0xFF).unwrap_err();
    assert!(matches!(error, PanGraphXError::Parse(_)), "{error}");
}

#[test]
fn test_truncated_message_is_rejected() {
    let mut buf = Vec::new();
    write_vg_graphs(&mut buf, &[make_simple_graph_proto()]).unwrap();
    buf.truncate(buf.len() - 3);
    let error = VGCodec.parse(&mut Cursor::new(&buf)).unwrap_err();
    assert!(
        error.to_string().contains("Unexpected EOF in a VG message"),
        "{error}"
    );
}

fn make_simple_graph_proto() -> vg_proto::Graph {
    vg_proto::Graph {
        node: vec![vg_proto::Node {
            id: 1,
            sequence: "ACGTACGT".to_string(),
            name: String::new(),
        }],
        ..Default::default()
    }
}
