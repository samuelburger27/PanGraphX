use super::reader::{canonical_name, edge_key};
use super::*;
use crate::core::core_types::{Edge, NodeId, Nodes, Orientation};
use crate::core::graph_dto::{CoreGraphDTO, GraphParts};
use crate::error::{PanGraphXError, PanResult};
use crate::traits::{GraphParser, GraphSerializer};
use rustc_hash::FxHashSet;
use std::collections::HashMap;
use std::io::Cursor;

fn parse(data: &[u8]) -> PanResult<CoreGraphDTO> {
    FastgCodec.parse(&mut Cursor::new(data))
}

fn serialize(graph: &CoreGraphDTO) -> String {
    let mut out = Vec::new();
    FastgCodec.serialize(graph, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn err(data: &[u8]) -> String {
    match parse(data) {
        Err(PanGraphXError::Parse(msg)) => msg,
        other => panic!("expected a parse error, got {other:?}"),
    }
}

fn name(graph: &CoreGraphDTO, id: NodeId) -> String {
    graph.get_name_from_id(id)
}

fn edge(from: NodeId, fo: Orientation, to: NodeId, to_o: Orientation) -> Edge {
    Edge {
        from_node: from,
        from_orient: fo,
        to_node: to,
        to_orient: to_o,
        overlap: 0,
    }
}

use Orientation::{Forward as F, Reverse as R};

#[test]
fn test_fastg_tiny() {
    let graph = parse(b">EDGE_1:EDGE_2;\nATCGCCCAT\n>EDGE_2:;\nGGATCC\n").unwrap();
    assert_eq!(graph.nodes().len(), 2);
    assert_eq!(graph.nodes()[0].sequence, b"ATCGCCCAT");
    assert_eq!(graph.nodes()[1].sequence, b"GGATCC");
    assert_eq!(graph.edges(), vec![edge(0, F, 1, F)]);
}

#[test]
fn header_without_edge_list() {
    let graph = parse(b">A:B;\nAC\n>B;\nGG\n").unwrap();
    assert_eq!(graph.nodes().len(), 2);
    assert_eq!(graph.nodes()[1].sequence, b"GG");
    assert_eq!(graph.edges().len(), 1);
}

/// A `'` record holds the reverse complement of the node's sequence.
#[test]
fn reverse_record_is_reverse_complemented() {
    let graph = parse(b">A':B;\nCGAT\n>B;\nGGAT\n").unwrap();
    assert_eq!(graph.nodes()[0].sequence, b"ATCG");
    assert_eq!(graph.edges(), vec![edge(0, R, 1, F)]);
}

#[test]
fn both_strands_merge_into_one_node() {
    let graph = parse(b">A:B;\nAACG\n>A';\nCGTT\n>B;\nGG\n>B':A';\nCC\n").unwrap();
    assert_eq!(graph.nodes().len(), 2);
    assert_eq!(graph.nodes()[0].sequence, b"AACG");
    // `B' -> A'` mirrors `A -> B`.
    assert_eq!(graph.edges(), vec![edge(0, F, 1, F)]);
}

#[test]
fn conflicting_strand_sequences_are_rejected() {
    assert!(err(b">A;\nAACG\n>A';\nAACG\n").contains("line 3"));
}

#[test]
fn spades_names_are_normalized() {
    let data = b">EDGE_1_length_4_cov_2.5:EDGE_2_length_2_cov_1';\nAACG\n\
                 >EDGE_2_length_2_cov_1;\nGG\n";
    let graph = parse(data).unwrap();
    assert_eq!(name(&graph, 0), "1");
    assert_eq!(name(&graph, 1), "2");
    assert_eq!(graph.edges(), vec![edge(0, F, 1, R)]);
}

#[test]
fn canonical_name_only_strips_full_spades_names() {
    assert_eq!(canonical_name(b"EDGE_7_length_10_cov_3.2"), b"7");
    assert_eq!(canonical_name(b"NODE_x_length_1_cov_1"), b"x");
    for name in [
        &b"EDGE_1"[..],
        b"EDGE_1_length_x_cov_1",
        b"EDGE_1_length_1_cov_",
        b"EDGE_1_length_1_cov_1_ID_3",
        b"_1_length_1_cov_1",
    ] {
        assert_eq!(canonical_name(name), name);
    }
}

#[test]
fn edge_key_matches_mirror() {
    let a = edge(3, F, 5, R);
    let mirror = edge(5, F, 3, R);
    assert_eq!(edge_key(&a), edge_key(&mirror));
    assert_ne!(edge_key(&a), edge_key(&edge(3, F, 5, F)));
}

#[test]
fn test_fastg_multiple_edges_with_orientations() {
    let data = b">N1:N2,N3',N4;\nATCG\n>N2:;\nTT\n>N3:;\nGG\n>N4:;\nCC\n";
    let graph = parse(data).unwrap();
    assert_eq!(graph.nodes().len(), 4);
    assert_eq!(
        graph.edges(),
        vec![edge(0, F, 1, F), edge(0, F, 2, R), edge(0, F, 3, F)]
    );
}

#[test]
fn test_fastg_multiline_sequence() {
    let graph = parse(b">A:B;\nAT\nCG\nAA\n>B:;\nGG\n").unwrap();
    assert_eq!(graph.nodes()[0].sequence, b"ATCGAA");
    assert_eq!(graph.nodes()[1].sequence, b"GG");
}

#[test]
fn crlf_and_empty_lines() {
    let graph = parse(b">A:B;\r\n\r\nATCG\r\n\n>B;\r\nGG").unwrap();
    assert_eq!(graph.nodes()[0].sequence, b"ATCG");
    assert_eq!(graph.nodes()[1].sequence, b"GG");
}

#[test]
fn duplicate_edges_are_merged() {
    let graph = parse(b">A:B,B;\nAT\n>B:;\nGG\n").unwrap();
    assert_eq!(graph.edges().len(), 1);
}

#[test]
fn invalid_dna_characters() {
    assert!(err(b">A:;\nATXG\n").contains("invalid character 'X'"));
}

#[test]
fn malformed_headers_are_errors() {
    assert!(err(b">A:B\nATCG\n").contains("';'"));
    assert!(err(b">;\nAC\n").contains("empty node name"));
    assert!(err(b">':;\nAC\n").contains("empty node name"));
    assert!(err(b">A:B,,C;\nAC\n").contains("empty node name"));
    assert!(err(b">A B;\nAC\n").contains("invalid node name"));
    assert!(err(b"ACGT\n>A;\nAC\n").contains("before the first header"));
    assert!(err(b">\nAC\n").contains("';'"));
}

#[test]
fn referenced_node_without_record() {
    assert!(err(b">A:B;\nATCG\n").contains("node B"));
}

fn sample_graph() -> CoreGraphDTO {
    CoreGraphDTO::try_from(GraphParts {
        nodes: Nodes::from_seq(vec![b"AACG".to_vec(), b"GT".to_vec(), b"T".to_vec()]),
        edges: vec![edge(0, F, 1, F), edge(1, R, 2, F), edge(2, F, 2, R)],
        paths: Vec::new(),
        node_name_map: Some(
            [
                (0, b"s1".to_vec()),
                (1, b"s2".to_vec()),
                (2, b"s3".to_vec()),
            ]
            .into_iter()
            .collect(),
        ),
    })
    .unwrap()
}

#[test]
fn serializes_both_strands_in_spades_style() {
    let expected = "\
>EDGE_s1_length_4_cov_1:EDGE_s2_length_2_cov_1;\nAACG\n\
>EDGE_s1_length_4_cov_1';\nCGTT\n\
>EDGE_s2_length_2_cov_1;\nGT\n\
>EDGE_s2_length_2_cov_1':EDGE_s1_length_4_cov_1',EDGE_s3_length_1_cov_1;\nAC\n\
>EDGE_s3_length_1_cov_1:EDGE_s3_length_1_cov_1';\nT\n\
>EDGE_s3_length_1_cov_1':EDGE_s2_length_2_cov_1;\nA\n";
    assert_eq!(serialize(&sample_graph()), expected);
}

#[test]
fn round_trip_keeps_names_sequences_and_edges() {
    let graph = sample_graph();
    let back = parse(serialize(&graph).as_bytes()).unwrap();
    assert_eq!(back.nodes(), graph.nodes());
    assert_eq!(back.node_name_map(), graph.node_name_map());
    let keys = |g: &CoreGraphDTO| g.edges().iter().map(edge_key).collect::<FxHashSet<_>>();
    assert_eq!(keys(&back), keys(&graph));
    assert_eq!(back.edges().len(), graph.edges().len());
}

#[test]
fn unsafe_names_fall_back_to_numbers() {
    let mut parts = GraphParts::from(sample_graph());
    parts.node_name_map = Some(HashMap::from([(0, b"a_b".to_vec())]));
    let graph = CoreGraphDTO::try_from(parts).unwrap();
    let text = serialize(&graph);
    assert!(text.starts_with(">EDGE_1_length_4_cov_1:EDGE_2_length_2_cov_1;\n"));
    let back = parse(text.as_bytes()).unwrap();
    assert_eq!(name(&back, 0), "1");
    assert_eq!(back.nodes(), graph.nodes());
}
