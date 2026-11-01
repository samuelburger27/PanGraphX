use crate::core::core_types::{Edge, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::{CoreGraphDTO, GraphParts};
use crate::traits::{GraphParser, GraphSerializer};
use gbz_construct::sequences::Sequences;
use gbz_construct::{
    FullPathName, GBWTBuilder, GBZ, GENERIC_HAPLOTYPE, GraphBuilder, Orientation as GbzOrientation,
    Tags, support,
};
use simple_sds::serialize::SerializeVersion;
use std::collections::HashMap;
use std::io::Cursor;

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
