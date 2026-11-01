//! FASTG reader: headers and sequence lines, both strands merged into one node.

use crate::core::core_types::{Edge, NodeId, NodeName, Nodes, Orientation, Sequence};
use crate::core::graph_dto::{CoreGraphDTO, GraphParts};
use crate::core::graph_utils::oriented_bases;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::common::text::{line_error, lossy};
use log::warn;
use memchr::memchr;
use rustc_hash::{FxHashMap, FxHashSet};
use std::io::{BufRead, BufReader, Read, Seek};

/// Bytes allowed in a sequence line: IUPAC nucleotide codes in either case.
pub(super) const SEQUENCE_BYTES: [bool; 256] = {
    let mut table = [false; 256];
    let codes = b"ACGTUNRYKMSWBDHVacgtunrykmswbdhv";
    let mut i = 0;
    while i < codes.len() {
        table[codes[i] as usize] = true;
        i += 1;
    }
    table
};

/// Strips the `SPAdes` wrapper `<prefix>_<id>_length_<len>_cov_<cov>` down to
/// `<id>`, the part Bandage uses as the node name. Other names are unchanged.
pub(super) fn canonical_name(name: &[u8]) -> &[u8] {
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
pub(super) fn split_strand(name: &[u8]) -> (&[u8], Orientation) {
    name.strip_suffix(b"'")
        .map_or((name, Orientation::Forward), |stripped| {
            (stripped, Orientation::Reverse)
        })
}

/// Parses one name of a header (the record itself or an edge-list entry).
pub(super) fn parse_name(raw: &[u8], line_no: usize) -> PanResult<(&[u8], Orientation)> {
    let (name, orientation) = split_strand(raw);
    if name.is_empty() {
        return Err(line_error("FASTG", line_no, "empty node name"));
    }
    if !name
        .iter()
        .all(|&b| b.is_ascii_graphic() && !matches!(b, b':' | b',' | b';' | b'\''))
    {
        return Err(line_error(
            "FASTG",
            line_no,
            &format!("invalid node name {:?}", lossy(raw)),
        ));
    }
    Ok((canonical_name(name), orientation))
}

/// Canonical key of an oriented edge: `a -> b` and its mirror `b' -> a'` are
/// the same edge and share a key.
pub(super) fn edge_key(edge: &Edge) -> (NodeId, bool, NodeId, bool) {
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
pub(super) struct FastgReader {
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
            .ok_or_else(|| line_error("FASTG", line_no, "header must end with ';'"))?;
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
            return Err(line_error(
                "FASTG",
                line_no,
                "sequence before the first header",
            ));
        }
        if let Some(&b) = line.iter().find(|&&b| !SEQUENCE_BYTES[usize::from(b)]) {
            return Err(line_error(
                "FASTG",
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
                "FASTG",
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
        CoreGraphDTO::new(GraphParts {
            nodes: Nodes::from_seq(sequences),
            edges: self.edges,
            paths: Vec::new(),
            node_name_map: Some(self.ids.into_iter().map(|(name, id)| (id, name)).collect()),
        })
    }
}

/// Reads a FASTG file; it has no paths.
pub(super) fn parse<R: Read + Seek>(reader: &mut R) -> PanResult<CoreGraphDTO> {
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
