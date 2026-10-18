//! FASTG codec in the `SPAdes` / Bandage dialect.
//!
//! A record is a header `>NAME[']:SUCC1,SUCC2';` (the edge list is optional)
//! followed by sequence lines. `NAME'` is the reverse-complement strand of
//! `NAME`, and its record holds the reverse-complemented sequence.
//!
//! **Output** follows `SPAdes`: every node gets a forward and a `'` record, each
//! listing the successors of that strand, so every edge appears from both
//! ends. Names are wrapped as `EDGE_<name>_length_<len>_cov_1`, the only form
//! for which Bandage resolves edge targets.
//!
//! **Input** strips that wrapper back to `<name>`, reverse-complements `'`
//! records and merges mirrored edges (`A:B` and `B':A'`), so single- and
//! double-stranded files parse to the same graph.
//!
//! FASTG has no paths or overlaps; they are dropped on output.

use crate::core::core_types::{Edge, Node, NodeId, NodeName, Nodes, Orientation, Sequence};
use crate::core::graph_dto::CoreGraphDTO;
use crate::core::graph_utils::oriented_bases;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::gfa_format::{RECORDS_PER_CHUNK, node_names, one_based_names, write_records};
use crate::traits::{GraphParser, GraphSerializer};
use log::warn;
use memchr::memchr;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::Cow;
use std::io::{BufRead, BufReader, Read, Seek, Write};

pub struct FastgCodec;

/// Bytes allowed in a sequence line: IUPAC nucleotide codes in either case.
const SEQUENCE_BYTES: [bool; 256] = {
    let mut table = [false; 256];
    let codes = b"ACGTUNRYKMSWBDHVacgtunrykmswbdhv";
    let mut i = 0;
    while i < codes.len() {
        table[codes[i] as usize] = true;
        i += 1;
    }
    table
};

fn line_error(line_no: usize, message: &str) -> PanGraphXError {
    PanGraphXError::Parse(format!("FASTG line {line_no}: {message}"))
}

fn lossy(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

/// Whether `name` can be written inside a SPAdes-style record name: non-empty
/// printable ASCII without `_` (Bandage splits on it) or FASTG delimiters.
fn is_record_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.iter().all(|&b| {
            b.is_ascii_graphic() && !matches!(b, b'_' | b':' | b',' | b';' | b'\'' | b'>')
        })
}

/// Strips the `SPAdes` wrapper `<prefix>_<id>_length_<len>_cov_<cov>` down to
/// `<id>`, the part Bandage uses as the node name. Other names are unchanged.
fn canonical_name(name: &[u8]) -> &[u8] {
    let mut parts = name.split(|&b| b == b'_');
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some(prefix), Some(id), Some(b"length"), Some(len), Some(b"cov"), Some(cov), None)
            if !prefix.is_empty()
                && !id.is_empty()
                && !len.is_empty()
                && len.iter().all(u8::is_ascii_digit)
                && !cov.is_empty() =>
        {
            id
        }
        _ => name,
    }
}

/// Splits a trailing `'` off a record name.
fn split_strand(name: &[u8]) -> (&[u8], Orientation) {
    name.strip_suffix(b"'")
        .map_or((name, Orientation::Forward), |stripped| {
            (stripped, Orientation::Reverse)
        })
}

/// Parses one name of a header (the record itself or an edge-list entry).
fn parse_name(raw: &[u8], line_no: usize) -> PanResult<(&[u8], Orientation)> {
    let (name, orientation) = split_strand(raw);
    if name.is_empty() {
        return Err(line_error(line_no, "empty node name"));
    }
    if !name
        .iter()
        .all(|&b| b.is_ascii_graphic() && !matches!(b, b':' | b',' | b';' | b'\''))
    {
        return Err(line_error(
            line_no,
            &format!("invalid node name {:?}", lossy(raw)),
        ));
    }
    Ok((canonical_name(name), orientation))
}

/// Canonical key of an oriented edge: `a -> b` and its mirror `b' -> a'` are
/// the same edge and share a key.
fn edge_key(edge: &Edge) -> (NodeId, bool, NodeId, bool) {
    let forward = (
        edge.from_node,
        edge.from_orient == Orientation::Reverse,
        edge.to_node,
        edge.to_orient == Orientation::Reverse,
    );
    let mirror = (
        edge.to_node,
        edge.to_orient == Orientation::Forward,
        edge.from_node,
        edge.from_orient == Orientation::Forward,
    );
    forward.min(mirror)
}

/// Incremental parser state.
#[derive(Default)]
struct FastgReader {
    ids: FxHashMap<NodeName, NodeId>,
    /// Forward-strand sequence per node, set by its first record.
    sequences: Vec<Option<Sequence>>,
    edges: Vec<Edge>,
    seen_edges: FxHashSet<(NodeId, bool, NodeId, bool)>,
    /// Record whose sequence lines are being read: node, strand, header line.
    current: Option<(NodeId, Orientation, usize)>,
    sequence: Sequence,
}

impl FastgReader {
    fn node_id(&mut self, name: &[u8]) -> NodeId {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = self.sequences.len();
        self.ids.insert(name.to_vec(), id);
        self.sequences.push(None);
        id
    }

    fn header(&mut self, line: &[u8], line_no: usize) -> PanResult<()> {
        self.finish_record()?;
        let body = line
            .get(1..)
            .and_then(|rest| rest.strip_suffix(b";"))
            .ok_or_else(|| line_error(line_no, "header must end with ';'"))?;
        let (record, list) =
            memchr(b':', body).map_or((body, &[][..]), |i| (&body[..i], &body[i + 1..]));

        let (name, from_orient) = parse_name(record, line_no)?;
        let from_node = self.node_id(name);
        if !list.is_empty() {
            for entry in list.split(|&b| b == b',') {
                let (name, to_orient) = parse_name(entry, line_no)?;
                let to_node = self.node_id(name);
                let edge = Edge {
                    from_node,
                    from_orient,
                    to_node,
                    to_orient,
                    overlap: 0,
                };
                if self.seen_edges.insert(edge_key(&edge)) {
                    self.edges.push(edge);
                }
            }
        }
        self.current = Some((from_node, from_orient, line_no));
        Ok(())
    }

    fn sequence_line(&mut self, line: &[u8], line_no: usize) -> PanResult<()> {
        if self.current.is_none() {
            return Err(line_error(line_no, "sequence before the first header"));
        }
        if let Some(&b) = line.iter().find(|&&b| !SEQUENCE_BYTES[usize::from(b)]) {
            return Err(line_error(
                line_no,
                &format!("invalid character {:?} in sequence", char::from(b)),
            ));
        }
        self.sequence.extend_from_slice(line);
        Ok(())
    }

    /// Stores the sequence of the record being read, checking it against an
    /// earlier record of the same node (e.g. the other strand).
    fn finish_record(&mut self) -> PanResult<()> {
        let Some((id, orientation, line_no)) = self.current.take() else {
            return Ok(());
        };
        let read = std::mem::take(&mut self.sequence);
        let forward = match orientation {
            Orientation::Forward => read,
            Orientation::Reverse => oriented_bases(&read, Orientation::Reverse, 0).collect(),
        };
        match &self.sequences[id] {
            Some(existing) if *existing != forward => Err(line_error(
                line_no,
                "sequence differs from an earlier record of the same node",
            )),
            Some(_) => Ok(()),
            None => {
                self.sequences[id] = Some(forward);
                Ok(())
            }
        }
    }

    fn into_graph(mut self) -> PanResult<CoreGraphDTO> {
        self.finish_record()?;
        let mut sequences = Vec::with_capacity(self.sequences.len());
        for (id, sequence) in self.sequences.into_iter().enumerate() {
            let Some(sequence) = sequence else {
                let name = self
                    .ids
                    .iter()
                    .find_map(|(name, &other)| (other == id).then(|| lossy(name).into_owned()))
                    .unwrap_or_default();
                return Err(PanGraphXError::Parse(format!(
                    "FASTG node {name} is referenced but has no record"
                )));
            };
            sequences.push(sequence);
        }
        Ok(CoreGraphDTO {
            nodes: Nodes::from_seq(sequences),
            edges: self.edges,
            paths: Vec::new(),
            node_name_map: Some(self.ids.into_iter().map(|(name, id)| (id, name)).collect()),
        })
    }
}

impl<R: Read + Seek> GraphParser<R> for FastgCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        warn!("FASTG files don't contain paths; only nodes and edges are parsed.");
        let mut reader = BufReader::with_capacity(1 << 20, reader);
        let mut state = FastgReader::default();
        let mut line = Vec::new();
        let mut line_no = 0;
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            line_no += 1;
            let trimmed = line.trim_ascii_end();
            match trimmed.first() {
                None => {}
                Some(b'>') => state.header(trimmed, line_no)?,
                Some(_) => state.sequence_line(trimmed, line_no)?,
            }
        }
        state.into_graph()
    }
}

/// Handle of a node strand: `node << 1 | is_reverse`.
#[inline]
fn handle(node: NodeId, orientation: Orientation) -> usize {
    node << 1 | usize::from(orientation == Orientation::Reverse)
}

/// Successor lists of every node strand in CSR form. Each edge `a -> b` is
/// stored under `a` and, mirrored as `b' -> a'`, under `b'`. Rows are sorted
/// and may contain duplicates, which the writer skips.
struct Successors {
    offsets: Vec<usize>,
    targets: Vec<usize>,
}

impl Successors {
    fn new(node_count: usize, edges: &[Edge]) -> Self {
        let pairs = || {
            edges.iter().flat_map(|edge| {
                let from = handle(edge.from_node, edge.from_orient);
                let to = handle(edge.to_node, edge.to_orient);
                [(from, to), (to ^ 1, from ^ 1)]
            })
        };

        let mut offsets = vec![0; 2 * node_count + 1];
        for (from, _) in pairs() {
            offsets[from + 1] += 1;
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }

        let mut cursor = offsets.clone();
        let mut targets = vec![0; 2 * edges.len()];
        for (from, to) in pairs() {
            targets[cursor[from]] = to;
            cursor[from] += 1;
        }
        drop(cursor);
        for row in offsets.windows(2) {
            targets[row[0]..row[1]].sort_unstable();
        }
        Self { offsets, targets }
    }

    fn of(&self, handle: usize) -> &[usize] {
        &self.targets[self.offsets[handle]..self.offsets[handle + 1]]
    }
}

/// Node names usable inside record names, falling back to `1..=n` for all
/// nodes if any name is not (so the fallback cannot collide with a kept name).
fn record_names(graph: &CoreGraphDTO) -> Vec<Cow<'_, [u8]>> {
    let names = node_names(graph);
    if names.par_iter().all(|name| is_record_name(name)) {
        return names;
    }
    warn!(
        "Some node names contain characters FASTG reserves (`_`, `:`, `,`, `;`, `'`, `>` or whitespace); nodes are renamed to 1..={}.",
        graph.nodes.len()
    );
    one_based_names(graph)
}

fn push_record_name(nodes: &Nodes, names: &[Cow<'_, [u8]>], strand: usize, buf: &mut Vec<u8>) {
    let id = strand >> 1;
    buf.extend_from_slice(b"EDGE_");
    buf.extend_from_slice(&names[id]);
    buf.extend_from_slice(b"_length_");
    buf.extend_from_slice(
        itoa::Buffer::new()
            .format(nodes[id].sequence.len())
            .as_bytes(),
    );
    buf.extend_from_slice(b"_cov_1");
    if strand & 1 == 1 {
        buf.push(b'\'');
    }
}

fn format_record(
    nodes: &Nodes,
    names: &[Cow<'_, [u8]>],
    successors: &Successors,
    node: &Node,
    orientation: Orientation,
    buf: &mut Vec<u8>,
) {
    let strand = handle(node.id, orientation);
    buf.push(b'>');
    push_record_name(nodes, names, strand, buf);
    let mut previous = None;
    for &target in successors.of(strand) {
        if previous == Some(target) {
            continue;
        }
        buf.push(if previous.is_none() { b':' } else { b',' });
        push_record_name(nodes, names, target, buf);
        previous = Some(target);
    }
    buf.extend_from_slice(b";\n");
    match orientation {
        Orientation::Forward => buf.extend_from_slice(&node.sequence),
        Orientation::Reverse => {
            buf.reserve(node.sequence.len());
            buf.extend(oriented_bases(&node.sequence, Orientation::Reverse, 0));
        }
    }
    buf.push(b'\n');
}

impl GraphSerializer for FastgCodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        if let Some(message) = graph.dangling_reference() {
            return Err(PanGraphXError::Serialize(message));
        }
        if !graph.paths.is_empty() {
            warn!(
                "FASTG files don't support paths; {} paths are dropped.",
                graph.paths.len()
            );
        }
        if graph.edges.par_iter().any(|edge| edge.overlap != 0) {
            warn!("FASTG files don't support overlaps; edge overlaps are dropped.");
        }

        let names = record_names(graph);
        let successors = Successors::new(graph.nodes.len(), &graph.edges);
        write_records(
            writer,
            graph.nodes.as_slice(),
            RECORDS_PER_CHUNK,
            |node, buf| {
                for orientation in [Orientation::Forward, Orientation::Reverse] {
                    format_record(&graph.nodes, &names, &successors, node, orientation, buf);
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[0].sequence, b"ATCGCCCAT");
        assert_eq!(graph.nodes[1].sequence, b"GGATCC");
        assert_eq!(graph.edges, vec![edge(0, F, 1, F)]);
    }

    #[test]
    fn header_without_edge_list() {
        let graph = parse(b">A:B;\nAC\n>B;\nGG\n").unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[1].sequence, b"GG");
        assert_eq!(graph.edges.len(), 1);
    }

    /// A `'` record holds the reverse complement of the node's sequence.
    #[test]
    fn reverse_record_is_reverse_complemented() {
        let graph = parse(b">A':B;\nCGAT\n>B;\nGGAT\n").unwrap();
        assert_eq!(graph.nodes[0].sequence, b"ATCG");
        assert_eq!(graph.edges, vec![edge(0, R, 1, F)]);
    }

    #[test]
    fn both_strands_merge_into_one_node() {
        let graph = parse(b">A:B;\nAACG\n>A';\nCGTT\n>B;\nGG\n>B':A';\nCC\n").unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[0].sequence, b"AACG");
        // `B' -> A'` mirrors `A -> B`.
        assert_eq!(graph.edges, vec![edge(0, F, 1, F)]);
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
        assert_eq!(graph.edges, vec![edge(0, F, 1, R)]);
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
        assert_eq!(graph.nodes.len(), 4);
        assert_eq!(
            graph.edges,
            vec![edge(0, F, 1, F), edge(0, F, 2, R), edge(0, F, 3, F)]
        );
    }

    #[test]
    fn test_fastg_multiline_sequence() {
        let graph = parse(b">A:B;\nAT\nCG\nAA\n>B:;\nGG\n").unwrap();
        assert_eq!(graph.nodes[0].sequence, b"ATCGAA");
        assert_eq!(graph.nodes[1].sequence, b"GG");
    }

    #[test]
    fn crlf_and_empty_lines() {
        let graph = parse(b">A:B;\r\n\r\nATCG\r\n\n>B;\r\nGG").unwrap();
        assert_eq!(graph.nodes[0].sequence, b"ATCG");
        assert_eq!(graph.nodes[1].sequence, b"GG");
    }

    #[test]
    fn duplicate_edges_are_merged() {
        let graph = parse(b">A:B,B;\nAT\n>B:;\nGG\n").unwrap();
        assert_eq!(graph.edges.len(), 1);
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
        CoreGraphDTO {
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
        }
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
        assert_eq!(back.nodes, graph.nodes);
        assert_eq!(back.node_name_map, graph.node_name_map);
        let keys = |g: &CoreGraphDTO| g.edges.iter().map(edge_key).collect::<FxHashSet<_>>();
        assert_eq!(keys(&back), keys(&graph));
        assert_eq!(back.edges.len(), graph.edges.len());
    }

    #[test]
    fn unsafe_names_fall_back_to_numbers() {
        let mut graph = sample_graph();
        graph.node_name_map = Some(HashMap::from([(0, b"a_b".to_vec())]));
        let text = serialize(&graph);
        assert!(text.starts_with(">EDGE_1_length_4_cov_1:EDGE_2_length_2_cov_1;\n"));
        let back = parse(text.as_bytes()).unwrap();
        assert_eq!(name(&back, 0), "1");
        assert_eq!(back.nodes, graph.nodes);
    }

    #[test]
    fn dangling_edge_is_a_serialize_error() {
        let mut graph = sample_graph();
        graph.edges.push(edge(0, F, 9, F));
        assert!(matches!(
            FastgCodec.serialize(&graph, &mut Vec::new()),
            Err(PanGraphXError::Serialize(_))
        ));
    }
}
