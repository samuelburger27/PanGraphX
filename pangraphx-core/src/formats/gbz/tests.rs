use crate::core::core_types::{Edge, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::{CoreGraphDTO, GraphParts};
use crate::traits::{GraphParser, GraphSerializer};
use gbz_construct::sequences::Sequences;
use gbz_construct::{
    FullPathName, GBWT, GBWTBuilder, GBZ, GENERIC_HAPLOTYPE, GraphBuilder,
    Orientation as GbzOrientation, Tags, support,
};
use simple_sds::serialize::SerializeVersion;
use std::collections::HashMap;
use std::io::Cursor;

use super::flat_bwt::FlatBwt;
use super::writer::{GBWT_BUFFER_SIZE, GBZ_SERIALIZE_VERSION, build_gbz};
use super::*;

/// Two nodes, one edge and one path, without node names.
fn unnamed_graph() -> CoreGraphDTO {
    CoreGraphDTO::try_from(GraphParts {
        nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
        edges: vec![Edge {
            from_node: 0,
            from_orient: Orientation::Forward,
            to_node: 1,
            to_orient: Orientation::Forward,
            overlap: 0,
        }],
        paths: vec![Path {
            name: b"p".to_vec(),
            steps: vec![
                Step::new(0, Orientation::Forward).unwrap(),
                Step::new(1, Orientation::Forward).unwrap(),
            ],
            overlaps: Vec::new(),
        }],
        node_name_map: None,
    })
    .unwrap()
}

#[test]
fn writes_vg_compatible_version() {
    let mut buf = Vec::new();
    GBZCodec.serialize(&unnamed_graph(), &mut buf).unwrap();
    let version = GBZ::determine_version(&mut Cursor::new(&buf)).unwrap();
    assert_eq!(version, GBZ_SERIALIZE_VERSION);
}

#[test]
fn nodes_without_translation_are_named_by_gbz_id() {
    // Without names the serializer writes GBZ node ids 1..=n and no translation.
    let mut buf = Vec::new();
    GBZCodec.serialize(&unnamed_graph(), &mut buf).unwrap();
    let parsed = GBZCodec.parse(&mut Cursor::new(&buf)).unwrap();
    assert_eq!(parsed.node_label(0), "1");
    assert_eq!(parsed.node_label(1), "2");
}

#[test]
fn id_names_do_not_add_a_translation() {
    let mut buf = Vec::new();
    GBZCodec.serialize(&unnamed_graph(), &mut buf).unwrap();
    let parsed = GBZCodec.parse(&mut Cursor::new(&buf)).unwrap();

    let mut rewritten = Vec::new();
    GBZCodec.serialize(&parsed, &mut rewritten).unwrap();
    assert!(!build_gbz(&parsed).unwrap().has_translation());
    assert_eq!(rewritten, buf);

    // Other names are kept in a translation.
    let named = CoreGraphDTO::try_from(GraphParts {
        node_name_map: Some(HashMap::from([(0, b"a".to_vec()), (1, b"b".to_vec())])),
        ..GraphParts::from(unnamed_graph())
    })
    .unwrap();
    assert!(build_gbz(&named).unwrap().has_translation());
}

#[test]
fn reads_reference_and_haplotype_paths() {
    // Nodes 1 and 2; every path is 1+ 2+.
    let mut graph_builder = GraphBuilder::new();
    graph_builder.add_node(1, b"ACGT").unwrap();
    graph_builder.add_node(2, b"GGA").unwrap();
    let mut gbwt_builder = GBWTBuilder::new(true, true, GBWT_BUFFER_SIZE);
    let encoded = [
        support::encode_node(1, GbzOrientation::Forward),
        support::encode_node(2, GbzOrientation::Forward),
    ];
    let no_haplotype = GENERIC_HAPLOTYPE as usize;
    for name in [
        FullPathName::generic("p"),
        FullPathName::haplotype("_gbwt_ref", "q", 0, 5),
        FullPathName::reference("GRCh38", "chr22"),
        FullPathName::haplotype("GRCh38", "chrM", 0, 10),
        FullPathName::haplotype("CHM13", "chr22", no_haplotype, 0),
        FullPathName::haplotype("HG1", "ctg#0", 1, 0),
        FullPathName::haplotype("HG1", "ctg#0", 2, 100),
    ] {
        gbwt_builder.insert(&encoded, Some(name)).unwrap();
    }
    let gbwt = gbwt_builder.build().unwrap();
    let sequences = Sequences::from_graph(&gbwt, &graph_builder).unwrap();
    let mut gbz = GBZ::from_parts(gbwt, sequences, Tags::new()).unwrap();
    gbz.set_reference_samples(&["GRCh38".to_string()]);
    let mut buf = Vec::new();
    gbz.serialize_version(&mut buf, GBZ_SERIALIZE_VERSION)
        .unwrap();

    let parsed = GBZCodec.parse(&mut Cursor::new(&buf)).unwrap();
    let names: Vec<&[u8]> = parsed.paths().iter().map(|p| p.name.as_slice()).collect();
    assert_eq!(
        names,
        [
            b"p".as_slice(),
            b"q[5]",
            b"GRCh38#0#chr22",
            b"GRCh38#0#chrM[10]",
            b"CHM13#chr22",
            b"HG1#1#ctg#0#0",
            b"HG1#2#ctg#0#100",
        ]
    );
    let expected_steps = [
        Step::new(0, Orientation::Forward).unwrap(),
        Step::new(1, Orientation::Forward).unwrap(),
    ];
    for path in parsed.paths() {
        assert_eq!(path.steps, expected_steps);
    }
}

/// Serializes a GBZ over the given segments and paths of encoded GBZ nodes.
fn gbz_bytes_with_segments(
    segments: &[(&str, &[u8], usize)],
    paths: &[(&str, Vec<usize>)],
) -> Vec<u8> {
    let mut graph_builder = GraphBuilder::new();
    for (name, sequence, max_length) in segments {
        graph_builder
            .translate_segment(name, sequence, *max_length)
            .unwrap();
    }
    let mut gbwt_builder = GBWTBuilder::new(true, true, GBWT_BUFFER_SIZE);
    for (name, path) in paths {
        gbwt_builder
            .insert(path, Some(FullPathName::generic(name)))
            .unwrap();
    }
    let gbwt = gbwt_builder.build().unwrap();
    let sequences = Sequences::from_graph(&gbwt, &graph_builder).unwrap();
    let gbz = GBZ::from_parts(gbwt, sequences, Tags::new()).unwrap();
    let mut buf = Vec::new();
    gbz.serialize_version(&mut buf, GBZ_SERIALIZE_VERSION)
        .unwrap();
    buf
}

fn forward(node: usize) -> usize {
    support::encode_node(node, GbzOrientation::Forward)
}

fn reverse(node: usize) -> usize {
    support::encode_node(node, GbzOrientation::Reverse)
}

#[test]
fn segments_of_several_nodes_become_one_step() {
    // Segment a is the GBZ nodes 1..=3 and b is node 4.
    let segments: [(&str, &[u8], usize); 2] = [("a", b"GATTACA", 3), ("b", b"CG", 3)];
    let paths = [
        ("fw", vec![forward(1), forward(2), forward(3), forward(4)]),
        ("rv", vec![reverse(4), reverse(3), reverse(2), reverse(1)]),
        (
            "mixed",
            vec![forward(4), reverse(3), reverse(2), reverse(1)],
        ),
    ];
    let buf = gbz_bytes_with_segments(&segments, &paths);
    let parsed = GBZCodec.parse(&mut Cursor::new(&buf)).unwrap();

    assert_eq!(parsed.nodes().len(), 2);
    assert_eq!(parsed.nodes()[0].sequence, b"GATTACA");
    let step = |node, orientation| Step::new(node, orientation).unwrap();
    let (fw, rv) = (Orientation::Forward, Orientation::Reverse);
    assert_eq!(parsed.paths()[0].steps, [step(0, fw), step(1, fw)]);
    assert_eq!(parsed.paths()[1].steps, [step(1, rv), step(0, rv)]);
    assert_eq!(parsed.paths()[2].steps, [step(1, fw), step(0, rv)]);
}

#[test]
fn path_over_part_of_a_segment_is_an_error() {
    let segments: [(&str, &[u8], usize); 2] = [("a", b"GATTACA", 3), ("b", b"CG", 3)];
    for (description, path) in [
        ("skips a node", vec![forward(1), forward(3), forward(4)]),
        ("starts inside", vec![forward(2), forward(3), forward(4)]),
        ("leaves early", vec![forward(1), forward(4)]),
    ] {
        // The valid path keeps all nodes in the GBWT.
        let valid = vec![forward(1), forward(2), forward(3), forward(4)];
        let buf = gbz_bytes_with_segments(&segments, &[("valid", valid), ("p", path)]);
        let result = GBZCodec.parse(&mut Cursor::new(&buf));
        assert!(result.is_err(), "the path {description}");
    }
}

#[test]
fn flat_bwt_follows_the_same_positions_as_the_gbwt() {
    // Branching paths with a repeated node, so that records have several runs.
    let paths = [
        ("p1", vec![forward(1), forward(2), forward(4), forward(5)]),
        ("p2", vec![forward(1), forward(3), forward(4), reverse(5)]),
        (
            "p3",
            vec![forward(1), forward(2), forward(4), forward(1), forward(3)],
        ),
        ("p4", vec![forward(1), forward(3), forward(4), forward(5)]),
        ("p5", vec![forward(1), forward(2), forward(4), reverse(5)]),
    ];
    let segments: [(&str, &[u8], usize); 5] = [
        ("a", b"A", 0),
        ("b", b"C", 0),
        ("c", b"G", 0),
        ("d", b"T", 0),
        ("e", b"AA", 0),
    ];
    let buf = gbz_bytes_with_segments(&segments, &paths);
    let gbz: GBZ = simple_sds::serialize::Serialize::load(&mut Cursor::new(&buf)).unwrap();
    let gbwt: &GBWT = gbz.as_ref();
    let flat = FlatBwt::new(gbwt).unwrap();

    let mut positions = 0;
    for sequence in 0..gbwt.sequences() {
        let mut position = gbwt.start(sequence);
        while let Some(pos) = position {
            let next = gbwt.forward(pos);
            assert_eq!(
                flat.lf(pos.node, pos.offset).unwrap(),
                next.map(|next| (next.node, next.offset)),
                "at {pos:?}"
            );
            position = next;
            positions += 1;
        }
    }
    assert_eq!(positions + gbwt.sequences(), gbwt.len());
    assert!(flat.lf(gbwt.alphabet_size(), 0).is_err());
}

/// A graph over the named nodes a, b, c, d with a path over the given node ids.
fn named_graph_with_path(steps: &[usize]) -> CoreGraphDTO {
    let names = [b"a", b"b", b"c", b"d"];
    CoreGraphDTO::try_from(GraphParts {
        nodes: Nodes::from_seq(vec![
            b"AA".to_vec(),
            b"CC".to_vec(),
            b"GG".to_vec(),
            b"TT".to_vec(),
        ]),
        edges: Vec::new(),
        paths: vec![Path {
            name: b"p".to_vec(),
            steps: steps
                .iter()
                .map(|&node| Step::new(node, Orientation::Forward).unwrap())
                .collect(),
            overlaps: Vec::new(),
        }],
        node_name_map: Some(
            names
                .iter()
                .enumerate()
                .map(|(i, name)| (i, name.to_vec()))
                .collect(),
        ),
    })
    .unwrap()
}

#[test]
fn segments_off_the_paths_are_dropped_wherever_they_are() {
    // The first segment is off the paths, in the middle, and at the end.
    for (visited, expected) in [
        (&[1, 3][..], &["b", "d"]),
        (&[0, 3], &["a", "d"]),
        (&[1, 2], &["b", "c"]),
    ] {
        let graph = named_graph_with_path(visited);
        let mut buf = Vec::new();
        GBZCodec.serialize(&graph, &mut buf).unwrap();
        let parsed = GBZCodec.parse(&mut Cursor::new(&buf)).unwrap();

        assert_eq!(parsed.nodes().len(), 2, "visiting {visited:?}");
        for (id, name) in expected.iter().enumerate() {
            assert_eq!(parsed.node_label(id), *name, "visiting {visited:?}");
        }
        let names: Vec<String> = parsed.paths()[0]
            .steps
            .iter()
            .map(|step| parsed.node_label(step.node_id()).to_string())
            .collect();
        assert_eq!(names, expected.map(String::from), "visiting {visited:?}");
    }
}

#[test]
fn graph_without_path_steps_is_an_error() {
    let mut buf = Vec::new();
    let error = GBZCodec
        .serialize(&named_graph_with_path(&[]), &mut buf)
        .unwrap_err();
    assert!(error.to_string().contains("without path steps"), "{error}");
}
