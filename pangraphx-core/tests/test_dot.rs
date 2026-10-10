use pangraphx_core::core::core_types::Nodes;
use pangraphx_core::{CoreGraphDTO, Edge, GraphFormat, Orientation};
use std::collections::HashMap;

#[test]
fn test_dot_extension_mapping() {
    assert_eq!(GraphFormat::DOT.get_extension(), "dot");
    assert_eq!(GraphFormat::from_extension("dot").unwrap(), GraphFormat::DOT);
    assert_eq!(GraphFormat::from_extension("DOT").unwrap(), GraphFormat::DOT);
    assert_eq!(GraphFormat::DOT.to_string(), "DOT");
}

#[test]
fn test_dot_serialize() {
    let nodes = Nodes::from_seq(vec![b"ACGT".to_vec(), b"TTGG".to_vec(), b"CCAA".to_vec()]);
    let mut name_map = HashMap::new();
    name_map.insert(0, b"1".to_vec());
    name_map.insert(1, b"2".to_vec());
    name_map.insert(2, b"3".to_vec());

    let graph = CoreGraphDTO {
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
                from_orient: Orientation::Reverse,
                to_node: 2,
                to_orient: Orientation::Forward,
                overlap: 0,
            },
        ],
        paths: vec![],
        node_name_map: Some(name_map),
    };

    let mut out = Vec::new();
    GraphFormat::DOT
        .get_serializer()
        .serialize(&graph, &mut out)
        .unwrap();

    let expected = "digraph G {\n\
        \trankdir=LR;\n\
        \t\"1\" [label=\"1\"];\n\
        \t\"2\" [label=\"2\"];\n\
        \t\"3\" [label=\"3\"];\n\
        \t\"1\" -> \"2\" [label=\"+/+\"];\n\
        \t\"2\" -> \"3\" [label=\"-/+\"];\n\
        }\n";
    assert_eq!(String::from_utf8(out).unwrap(), expected);
}

#[test]
fn test_dot_serialize_empty_graph() {
    let graph = CoreGraphDTO::default();
    let mut out = Vec::new();
    GraphFormat::DOT
        .get_serializer()
        .serialize(&graph, &mut out)
        .unwrap();

    assert_eq!(
        String::from_utf8(out).unwrap(),
        "digraph G {\n\trankdir=LR;\n}\n"
    );
}

#[test]
fn test_dot_serialize_escapes_node_names() {
    let nodes = Nodes::from_seq(vec![b"A".to_vec()]);
    let mut name_map = HashMap::new();
    name_map.insert(0, b"a\"b\\c".to_vec());

    let graph = CoreGraphDTO {
        nodes,
        edges: vec![],
        paths: vec![],
        node_name_map: Some(name_map),
    };

    let mut out = Vec::new();
    GraphFormat::DOT
        .get_serializer()
        .serialize(&graph, &mut out)
        .unwrap();

    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("a\\\"b\\\\c"), "unexpected output: {output}");
}
