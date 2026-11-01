use super::*;
use crate::core::core_types::{Edge, NodeId, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::{CoreGraphDTO, GraphParts};
use crate::formats::odgi::encoding::{Fields, push_packed, push_u64};
use crate::formats::odgi::reader::{BLOCK_BYTES, scan_record};
use crate::formats::odgi::writer::{MAX_RANK_HINTS, SMALL_DECODING, SMALL_NODE_DEGREE, write_odgi};
use crate::traits::{GraphParser, GraphSerializer};
use Orientation::{Forward, Reverse};
use std::collections::HashMap;
use std::io::{self, Cursor, Read, Seek, SeekFrom};

fn edge(from: NodeId, from_orient: Orientation, to: NodeId, to_orient: Orientation) -> Edge {
    Edge {
        from_node: from,
        from_orient,
        to_node: to,
        to_orient,
        overlap: 0,
    }
}

fn path(name: &str, steps: &[(NodeId, Orientation)]) -> Path {
    Path {
        name: name.as_bytes().to_vec(),
        steps: steps
            .iter()
            .map(|&(node, orientation)| Step::new(node, orientation).unwrap())
            .collect(),
        overlaps: Vec::new(),
    }
}

fn roundtrip(graph: &CoreGraphDTO) -> CoreGraphDTO {
    let mut buf = Vec::new();
    ODGICodec.serialize(graph, &mut buf).unwrap();
    ODGICodec.parse(&mut Cursor::new(buf)).unwrap()
}

/// Names nodes by their 1-based id, as they come back from odgi.
fn one_based(graph: CoreGraphDTO) -> CoreGraphDTO {
    let mut parts = GraphParts::from(graph);
    parts.node_name_map = Some(
        (0..parts.nodes.len())
            .map(|id| (id, (id + 1).to_string().into_bytes()))
            .collect(),
    );
    CoreGraphDTO::try_from(parts).unwrap()
}

#[test]
fn roundtrip_keeps_orientations() {
    let original = CoreGraphDTO::try_from(GraphParts {
        nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
        edges: vec![edge(0, Forward, 1, Reverse)],
        paths: vec![path("p", &[(0, Forward), (1, Reverse)])],
        node_name_map: None,
    })
    .unwrap();
    let parsed = roundtrip(&original);
    assert_eq!(parsed.get_name_from_id(0), "1");
    assert_eq!(parsed.get_name_from_id(1), "2");
    assert_eq!(parsed, one_based(original));
}

#[test]
fn roundtrip_self_loops_revisits_and_empty_paths() {
    let original = one_based(
        CoreGraphDTO::try_from(GraphParts {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"CC".to_vec(), b"GGG".to_vec()]),
            edges: vec![
                edge(0, Forward, 0, Forward),
                edge(1, Forward, 1, Reverse),
                edge(2, Reverse, 0, Forward),
                edge(0, Forward, 1, Forward),
            ],
            paths: vec![
                path(
                    "a",
                    &[(0, Forward), (1, Forward), (0, Forward), (0, Reverse)],
                ),
                path("empty", &[]),
                path(
                    "b",
                    &[(2, Reverse), (0, Forward), (1, Forward), (1, Reverse)],
                ),
            ],
            node_name_map: None,
        })
        .unwrap(),
    );
    // Edges come back grouped by node, so compare them as a set.
    let parsed = roundtrip(&original);
    assert!(parsed.eq_by_node_names(&original));
    assert_eq!(parsed.paths(), original.paths());
}

#[test]
fn reverse_complement_edges_are_merged() {
    let graph = CoreGraphDTO::try_from(GraphParts {
        nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec()]),
        edges: vec![
            edge(0, Forward, 1, Forward),
            edge(1, Reverse, 0, Reverse),
            edge(0, Forward, 1, Forward),
        ],
        paths: Vec::new(),
        node_name_map: None,
    })
    .unwrap();
    assert_eq!(
        roundtrip(&graph).edges(),
        vec![edge(0, Forward, 1, Forward)]
    );
}

#[test]
fn numeric_names_are_kept_with_gaps() {
    let names = ["7", "3", "1"];
    let original = CoreGraphDTO::try_from(GraphParts {
        nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec(), b"G".to_vec()]),
        edges: vec![edge(0, Forward, 1, Reverse), edge(2, Forward, 0, Forward)],
        paths: vec![path("p", &[(2, Forward), (0, Forward), (1, Reverse)])],
        node_name_map: Some(
            names
                .iter()
                .enumerate()
                .map(|(id, name)| (id, name.as_bytes().to_vec()))
                .collect(),
        ),
    })
    .unwrap();
    let parsed = roundtrip(&original);
    // Nodes come back in id order; ids 2 and 4..=6 are deleted records.
    assert_eq!(parsed.nodes().len(), 3);
    assert!(parsed.eq_by_node_names(&original));
}

#[test]
fn non_numeric_names_are_replaced() {
    let original = CoreGraphDTO::try_from(GraphParts {
        nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec()]),
        edges: vec![edge(0, Forward, 1, Forward)],
        paths: Vec::new(),
        node_name_map: Some(HashMap::from([(0, b"x".to_vec()), (1, b"01".to_vec())])),
    })
    .unwrap();
    let parsed = roundtrip(&original);
    assert_eq!(parsed.get_name_from_id(0), "1");
    assert_eq!(parsed.get_name_from_id(1), "2");
}

#[test]
fn records_spanning_blocks() {
    // Records larger than a block and records cut by a block boundary.
    let big = |base: u8, len: usize| (0..len).map(|i| b"ACGT"[(i + base as usize) % 4]).collect();
    let original = one_based(
        CoreGraphDTO::try_from(GraphParts {
            nodes: Nodes::from_seq(vec![
                b"A".to_vec(),
                big(0, BLOCK_BYTES + 100),
                big(1, BLOCK_BYTES / 3),
                big(2, BLOCK_BYTES / 3),
                big(3, BLOCK_BYTES / 2),
                b"C".to_vec(),
            ]),
            edges: (0..5).map(|i| edge(i, Forward, i + 1, Forward)).collect(),
            paths: vec![
                path("p", &(0..6).map(|i| (i, Forward)).collect::<Vec<_>>()),
                path("q", &[(5, Reverse), (4, Reverse), (3, Reverse)]),
            ],
            node_name_map: None,
        })
        .unwrap(),
    );
    assert_eq!(roundtrip(&original), original);
}

#[test]
fn high_degree_nodes() {
    // Node 0 has more edges, decoding entries and path neighbors than the
    // small linear-scan limits, with duplicate and reverse-complement edges.
    let n = 3 * SMALL_NODE_DEGREE.max(SMALL_DECODING).max(MAX_RANK_HINTS);
    let mut edges = Vec::new();
    let mut paths = Vec::new();
    for i in 1..n {
        edges.push(edge(0, Forward, i, Forward));
        edges.push(edge(i, Reverse, 0, Reverse));
        edges.push(edge(i, Forward, 0, Forward));
        paths.push(path(
            &format!("p{i}"),
            &[(i, Forward), (0, Forward), (n - i, Forward)],
        ));
    }
    let original = one_based(
        CoreGraphDTO::try_from(GraphParts {
            nodes: Nodes::from_seq((0..n).map(|i| vec![b"ACGT"[i % 4]]).collect()),
            edges,
            paths,
            node_name_map: None,
        })
        .unwrap(),
    );
    let parsed = roundtrip(&original);
    assert_eq!(parsed.edges().len(), 2 * (n - 1));
    assert_eq!(parsed.paths(), original.paths());
}

#[test]
fn step_number_widths_agree() {
    let graph = one_based(
        CoreGraphDTO::try_from(GraphParts {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"CC".to_vec(), b"GGG".to_vec()]),
            edges: vec![edge(0, Forward, 1, Forward), edge(1, Forward, 2, Reverse)],
            paths: vec![
                path(
                    "a",
                    &[(0, Forward), (1, Forward), (2, Reverse), (1, Reverse)],
                ),
                path("b", &[(1, Forward), (2, Reverse)]),
            ],
            node_name_map: None,
        })
        .unwrap(),
    );
    let (mut narrow, mut wide) = (Vec::new(), Vec::new());
    write_odgi::<u32>(&graph, &mut narrow).unwrap();
    write_odgi::<u64>(&graph, &mut wide).unwrap();
    assert_eq!(narrow, wide);
}

/// A source whose end lies `extra` bytes beyond its data.
struct OverstatedLen {
    inner: Cursor<Vec<u8>>,
    extra: u64,
}

impl Read for OverstatedLen {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for OverstatedLen {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::End(offset) => {
                let end = self.inner.get_ref().len() as u64 + self.extra;
                let position = end.checked_add_signed(offset).unwrap();
                self.inner.set_position(position);
                Ok(position)
            }
            pos => self.inner.seek(pos),
        }
    }
}

fn two_path_graph() -> CoreGraphDTO {
    one_based(
        CoreGraphDTO::try_from(GraphParts {
            nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
            edges: vec![edge(0, Forward, 1, Reverse)],
            paths: vec![
                path("a", &[(0, Forward), (1, Reverse)]),
                path("b", &[(1, Forward), (0, Reverse)]),
            ],
            node_name_map: None,
        })
        .unwrap(),
    )
}

#[test]
fn truncated_input_with_overstated_length_is_rejected() {
    let mut buf = Vec::new();
    ODGICodec.serialize(&two_path_graph(), &mut buf).unwrap();
    buf.truncate(buf.len() / 2);
    let mut reader = OverstatedLen {
        inner: Cursor::new(buf),
        extra: 1 << 20,
    };
    assert!(ODGICodec.parse(&mut reader).is_err());
}

#[test]
fn step_count_is_bounded_by_stored_words() {
    // A live record whose step vector claims 6 * 2^27 values in 0 words.
    let mut record = Vec::new();
    push_u64(&mut record, 1);
    record.push(b'A');
    push_u64(&mut record, 1);
    for _ in 0..2 {
        push_packed(&mut record, &[]);
    }
    for value in [0, 0, 6 << 27] {
        push_u64(&mut record, value);
    }
    record.extend_from_slice(&[1, 64]);
    let span = scan_record(&record, 0).unwrap();
    assert_eq!((span.end, span.steps), (record.len(), 0));
    assert!(
        Fields {
            bytes: &record[9 + 8 + 2 * 26..]
        }
        .packed(&mut Vec::new())
        .is_err()
    );
}

#[test]
fn paths_longer_than_the_stored_steps_are_rejected() {
    let mut buf = Vec::new();
    ODGICodec.serialize(&two_path_graph(), &mut buf).unwrap();
    // Each path record: length, first and last step (4 values), name length, name.
    let record = 8 * 6 + 1;
    for start in [buf.len() - 2 * record, buf.len() - record] {
        buf[start..start + 8].copy_from_slice(&4_u64.to_le_bytes());
    }
    let error = ODGICodec.parse(&mut Cursor::new(buf)).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("longer than the number of stored steps")
    );
}

#[test]
fn empty_graph() {
    let parsed = roundtrip(&CoreGraphDTO::default());
    assert_eq!(
        (
            parsed.nodes().len(),
            parsed.edges().len(),
            parsed.paths().len()
        ),
        (0, 0, 0)
    );
}

#[test]
fn rejects_truncated_and_corrupt_input() {
    let graph = one_based(
        CoreGraphDTO::try_from(GraphParts {
            nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
            edges: vec![edge(0, Forward, 1, Reverse)],
            paths: vec![path("p", &[(0, Forward), (1, Reverse)])],
            node_name_map: None,
        })
        .unwrap(),
    );
    let mut buf = Vec::new();
    ODGICodec.serialize(&graph, &mut buf).unwrap();
    for len in [0, 3, 20, buf.len() / 2, buf.len() - 1] {
        assert!(
            ODGICodec.parse(&mut Cursor::new(&buf[..len])).is_err(),
            "{len}"
        );
    }
    assert!(ODGICodec.parse(&mut Cursor::new(vec![0xFF; 256])).is_err());
}
